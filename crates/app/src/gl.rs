//! Output render thread: owns the GL context for one output window, keeps slide
//! frames as textures, plays background media (mpv) into a framebuffer, and
//! presents with vsync. Independent of any UI frame timing.
//!
//! Draw order: black → background media (canvas area) → slide background colour
//! (if the slide sets one) → slide text. Background and text layers can each be
//! hidden (Clear Background / Clear Text).
#![allow(unsafe_code)] // OpenGL calls are inherently unsafe; confined to this module.

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use glow::HasContext as _;
use glutin::context::{NotCurrentContext, NotCurrentGlContext as _, PossiblyCurrentContext};
use glutin::display::{Display, GlDisplay as _};
use glutin::surface::{GlSurface as _, Surface, SwapInterval, WindowSurface};
use render::{PixelRect, SlideFrame};
use winit::event_loop::EventLoopProxy;

use crate::UserEvent;
use crate::raster::{FrameKey, RenderCmd};

/// Everything the render thread needs; built on the event-loop thread.
pub struct GlParts {
    pub context: NotCurrentContext,
    pub surface: Surface<WindowSurface>,
    pub display: Display,
    pub width: u32,
    pub height: u32,
    /// This thread's own command channel (mpv's "new frame" callback posts to it).
    pub self_tx: Sender<RenderCmd>,
    pub audio_device: Option<String>,
}

struct Texture {
    key: FrameKey,
    tex: glow::Texture,
    frame: Arc<SlideFrame>,
}

const TEXTURE_CACHE: usize = 6;

const VS: &str = r"#version 330 core
out vec2 uv;
void main() {
    // Triangle strip over the viewport: (0,0) (2,0) (0,2) (2,2) - 1.
    vec2 p = vec2(float((gl_VertexID & 1) << 1), float(gl_VertexID & 2));
    uv = vec2(p.x * 0.5, 1.0 - p.y * 0.5); // texture row 0 is the top
    gl_Position = vec4(p - 1.0, 0.0, 1.0);
}";

const FS: &str = r"#version 330 core
uniform sampler2D tex;
in vec2 uv;
out vec4 color;
void main() { color = texture(tex, uv); } // premultiplied alpha";

/// Run until `Stop` or the channel closes. Errors are reported to the event loop.
pub fn run(parts: GlParts, rx: &Receiver<RenderCmd>, events: &EventLoopProxy<UserEvent>) {
    if let Err(e) = run_inner(parts, rx, events) {
        tracing::error!(error = %e, "output render thread failed");
        let _ = events.send_event(UserEvent::OutputFailed(e));
    }
}

fn run_inner(
    parts: GlParts,
    rx: &Receiver<RenderCmd>,
    events: &EventLoopProxy<UserEvent>,
) -> Result<(), String> {
    let GlParts {
        context,
        surface,
        display,
        mut width,
        mut height,
        self_tx,
        audio_device,
    } = parts;
    let ctx: PossiblyCurrentContext = context
        .make_current(&surface)
        .map_err(|e| format!("make current: {e}"))?;
    if let Err(e) = surface.set_swap_interval(&ctx, SwapInterval::Wait(NonZeroU32::MIN)) {
        tracing::warn!(error = %e, "vsync unavailable; frames may tear");
    }
    let gl =
        unsafe { glow::Context::from_loader_function_cstr(|s| display.get_proc_address(s).cast()) };
    let mut r = Renderer::new(&gl)?;
    let mut media = MediaLayer::new(display.clone(), self_tx, events.clone(), audio_device);
    tracing::info!(
        version = unsafe { gl.get_parameter_string(glow::VERSION) },
        renderer = unsafe { gl.get_parameter_string(glow::RENDERER) },
        width,
        height,
        "output GL ready"
    );

    let present = |r: &Renderer, media: &MediaLayer, w: u32, h: u32| -> Result<(), String> {
        r.draw(&gl, media, w, h);
        surface
            .swap_buffers(&ctx)
            .map_err(|e| format!("swap buffers: {e}"))
    };
    present(&r, &media, width, height)?;

    // A command taken off the queue while coalescing media frames, handled next.
    let mut pending: Option<RenderCmd> = None;
    loop {
        // Once idle after startup, set up the media players (≈100–200 ms of GL work)
        // so the first background trigger during a service never stalls a slide.
        let cmd = if let Some(c) = pending.take() {
            c
        } else if media.wants_warm_up() {
            match rx.recv_timeout(std::time::Duration::from_millis(300)) {
                Ok(c) => c,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    media.warm_up();
                    // libmpv is loaded now: the app can do its other mpv start-up work.
                    let _ = events.send_event(UserEvent::MediaWarm);
                    continue;
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(c) => c,
                Err(_) => break,
            }
        };
        match cmd {
            RenderCmd::Show {
                key,
                frame,
                layers,
                requested_at,
            } => {
                let upload_started = Instant::now();
                r.ensure(&gl, key, frame)?;
                r.current = Some(key);
                r.layers = layers;
                let upload = upload_started.elapsed();
                media.render(&gl, r.media_rect(width, height), true);
                present(&r, &media, width, height)?;
                presented(events, Some(key.flat), requested_at, upload);
            }
            RenderCmd::Preload { key, frame } => r.ensure(&gl, key, frame)?,
            RenderCmd::Layers {
                text,
                background,
                requested_at,
            } => {
                r.layers = (text, background);
                present(&r, &media, width, height)?;
                presented(events, None, requested_at, Default::default());
            }
            RenderCmd::Blank {
                layers,
                requested_at,
            } => {
                r.current = None;
                r.layers = layers;
                media.render(&gl, r.media_rect(width, height), true);
                present(&r, &media, width, height)?;
                presented(events, None, requested_at, Default::default());
            }
            RenderCmd::Media { current, next } => {
                media.apply(current, next);
                media.render(&gl, r.media_rect(width, height), true);
                present(&r, &media, width, height)?;
            }
            RenderCmd::MediaFrame => {
                // Coalesce a burst of frame notifications into one render.
                loop {
                    match rx.try_recv() {
                        Ok(RenderCmd::MediaFrame) => {}
                        // Not a frame: keep it for the next iteration (never drop a command).
                        Ok(other) => {
                            pending = Some(other);
                            break;
                        }
                        Err(_) => break,
                    }
                }
                if media.render(&gl, r.media_rect(width, height), false) {
                    present(&r, &media, width, height)?;
                }
            }
            RenderCmd::AudioDevice(d) => media.set_audio_device(d),
            RenderCmd::Resize {
                width: w,
                height: h,
            } => {
                if let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) {
                    surface.resize(&ctx, nw, nh);
                    (width, height) = (w, h);
                    media.render(&gl, r.media_rect(width, height), true);
                    present(&r, &media, width, height)?;
                }
            }
            RenderCmd::Redraw => present(&r, &media, width, height)?,
            RenderCmd::Capture(path) => {
                r.draw(&gl, &media, width, height);
                let result = capture(&gl, width, height, &path);
                surface
                    .swap_buffers(&ctx)
                    .map_err(|e| format!("swap buffers: {e}"))?;
                let _ = events.send_event(UserEvent::Captured { path, result });
            }
            RenderCmd::Stop => break,
        }
    }
    // mpv render contexts must be freed while this GL context is current.
    media.destroy(&gl);
    r.destroy(&gl);
    Ok(())
}

/// Read the back buffer (just drawn) and save it top-row-first as PNG.
fn capture(gl: &glow::Context, w: u32, h: u32, path: &std::path::Path) -> Result<(), String> {
    let mut px = vec![0u8; w as usize * h as usize * 4];
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
        gl.read_pixels(
            0,
            0,
            w as i32,
            h as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut px)),
        );
    }
    // GL rows are bottom-up.
    let row = w as usize * 4;
    let flipped: Vec<u8> = px.chunks_exact(row).rev().flatten().copied().collect();
    let img = image::RgbaImage::from_raw(w, h, flipped).ok_or("capture buffer size mismatch")?;
    img.save(path).map_err(|e| e.to_string())
}

fn presented(
    events: &EventLoopProxy<UserEvent>,
    index: Option<usize>,
    requested_at: Option<Instant>,
    upload: std::time::Duration,
) {
    let _ = events.send_event(UserEvent::Presented {
        index,
        latency: requested_at.map(|t| t.elapsed()),
        upload,
        at: Instant::now(),
    });
}

/// Output-space rectangle in GL coordinates (origin bottom-left).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GlRect {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
}

struct Renderer {
    program: glow::Program,
    vao: glow::VertexArray,
    textures: Vec<Texture>,
    current: Option<FrameKey>,
    /// (text visible, background visible)
    layers: (bool, bool),
}

impl Renderer {
    fn new(gl: &glow::Context) -> Result<Self, String> {
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = Vec::new();
            for (kind, src) in [(glow::VERTEX_SHADER, VS), (glow::FRAGMENT_SHADER, FS)] {
                let s = gl.create_shader(kind)?;
                gl.shader_source(s, src);
                gl.compile_shader(s);
                if !gl.get_shader_compile_status(s) {
                    return Err(format!("shader compile: {}", gl.get_shader_info_log(s)));
                }
                gl.attach_shader(program, s);
                shaders.push(s);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(format!("shader link: {}", gl.get_program_info_log(program)));
            }
            for s in shaders {
                gl.detach_shader(program, s);
                gl.delete_shader(s);
            }
            let vao = gl.create_vertex_array()?;
            Ok(Renderer {
                program,
                vao,
                textures: Vec::new(),
                current: None,
                layers: (true, true),
            })
        }
    }

    fn current_frame(&self) -> Option<&Arc<SlideFrame>> {
        let k = self.current?;
        self.textures.iter().find(|t| t.key == k).map(|t| &t.frame)
    }

    /// Where the current frame sits on a `sw × sh` surface (centred 1:1).
    fn frame_origin(f: &SlideFrame, sw: u32, sh: u32) -> (i32, i32) {
        (
            (sw as i32 - f.width as i32) / 2,
            (sh as i32 - f.height as i32) / 2,
        )
    }

    /// The canvas area in GL coordinates (whole surface if nothing is live).
    fn canvas_rect(f: Option<&SlideFrame>, sw: u32, sh: u32) -> GlRect {
        match f {
            Some(f) => {
                let (ox, oy) = Self::frame_origin(f, sw, sh);
                let c: PixelRect = f.placement.canvas;
                GlRect {
                    x: ox + c.x as i32,
                    y: oy + (f.height - c.y - c.height) as i32,
                    w: c.width,
                    h: c.height,
                }
            }
            None => GlRect {
                x: 0,
                y: 0,
                w: sw,
                h: sh,
            },
        }
    }

    fn media_rect(&self, sw: u32, sh: u32) -> GlRect {
        Self::canvas_rect(self.current_frame().map(|f| &**f), sw, sh)
    }

    /// Upload `frame` unless a texture for `key` already exists.
    fn ensure(
        &mut self,
        gl: &glow::Context,
        key: FrameKey,
        frame: Arc<SlideFrame>,
    ) -> Result<(), String> {
        if let Some(pos) = self.textures.iter().position(|t| t.key == key) {
            let t = self.textures.remove(pos);
            self.textures.push(t);
            return Ok(());
        }
        let tex = unsafe {
            let tex = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            // Drawn 1:1, so nearest keeps pixels exactly as rendered.
            set_tex_params(gl, glow::NEAREST);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                frame.width as i32,
                frame.height as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&frame.text)),
            );
            tex
        };
        self.textures.push(Texture { key, tex, frame });
        // Evict least recently used, but never the one on screen.
        while self.textures.len() > TEXTURE_CACHE {
            let Some(pos) = self
                .textures
                .iter()
                .position(|t| Some(t.key) != self.current)
            else {
                break;
            };
            let old = self.textures.remove(pos);
            unsafe { gl.delete_texture(old.tex) };
        }
        Ok(())
    }

    fn blit(&self, gl: &glow::Context, tex: glow::Texture, rect: GlRect, blend: bool) {
        unsafe {
            gl.viewport(rect.x, rect.y, rect.w as i32, rect.h as i32);
            if blend {
                gl.enable(glow::BLEND);
                gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            } else {
                gl.disable(glow::BLEND);
            }
            gl.use_program(Some(self.program));
            gl.bind_vertex_array(Some(self.vao));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.disable(glow::BLEND);
        }
    }

    /// Black → media → slide background colour → text. A frame rendered for a
    /// different size is centred 1:1, never stretched.
    fn draw(&self, gl: &glow::Context, media: &MediaLayer, sw: u32, sh: u32) {
        unsafe {
            // mpv may leave its own state bound.
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.disable(glow::SCISSOR_TEST);
            gl.viewport(0, 0, sw as i32, sh as i32);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        let frame = self.current_frame();
        let (text_on, bg_on) = self.layers;
        if bg_on && let Some(tex) = media.texture() {
            self.blit(gl, tex, self.media_rect(sw, sh), false);
        }
        let Some(f) = frame else { return };
        if bg_on && let Some(bg) = f.background {
            let c = Self::canvas_rect(Some(f), sw, sh);
            let [r, g, b, a] = bg.map(|v| f32::from(v) / 255.0);
            unsafe {
                gl.enable(glow::SCISSOR_TEST);
                gl.scissor(c.x, c.y, c.w as i32, c.h as i32);
                gl.clear_color(r * a, g * a, b * a, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                gl.disable(glow::SCISSOR_TEST);
            }
        }
        if text_on
            && let Some(t) = self
                .current
                .and_then(|k| self.textures.iter().find(|t| t.key == k))
        {
            let (ox, oy) = Self::frame_origin(f, sw, sh);
            self.blit(
                gl,
                t.tex,
                GlRect {
                    x: ox,
                    y: oy,
                    w: f.width,
                    h: f.height,
                },
                true,
            );
        }
    }

    fn destroy(&mut self, gl: &glow::Context) {
        unsafe {
            for t in self.textures.drain(..) {
                gl.delete_texture(t.tex);
            }
            gl.delete_vertex_array(self.vao);
            gl.delete_program(self.program);
        }
    }
}

unsafe fn set_tex_params(gl: &glow::Context, filter: u32) {
    unsafe {
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter as i32);
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
    }
}

struct Fbo {
    tex: glow::Texture,
    fb: glow::Framebuffer,
    w: u32,
    h: u32,
}

/// Background media: an active player shown on screen and a standby player that
/// pre-loads the next background (paused) so switching is instant.
struct MediaLayer {
    display: Display,
    self_tx: Sender<RenderCmd>,
    events: EventLoopProxy<UserEvent>,
    audio_device: Option<String>,
    players: Vec<media::Player>,
    paths: [Option<PathBuf>; 2],
    active: usize,
    target: Option<Fbo>,
    has_frame: bool,
    unavailable_reported: bool,
    warm_up_tried: bool,
}

impl MediaLayer {
    fn new(
        display: Display,
        self_tx: Sender<RenderCmd>,
        events: EventLoopProxy<UserEvent>,
        audio_device: Option<String>,
    ) -> Self {
        MediaLayer {
            display,
            self_tx,
            events,
            audio_device,
            players: Vec::new(),
            paths: [None, None],
            active: 0,
            target: None,
            has_frame: false,
            unavailable_reported: false,
            warm_up_tried: false,
        }
    }

    fn report(&self, e: &media::MediaError) {
        tracing::warn!(error = %e, "background media");
        let _ = self
            .events
            .send_event(UserEvent::MediaFailed(e.user_message()));
    }

    /// Create both players on first use (needs this thread's GL context current).
    fn ensure_players(&mut self) -> bool {
        if self.players.len() == 2 {
            return true;
        }
        if self.unavailable_reported {
            return false;
        }
        let started = Instant::now();
        for _ in 0..2 {
            let ev = self.events.clone();
            let player = media::Player::new(self.audio_device.as_deref(), move |e| {
                if let media::MediaEvent::Failed(err) = e {
                    let _ = ev.send_event(UserEvent::MediaFailed(err.user_message()));
                }
            })
            .and_then(|mut p| {
                tracing::info!(
                    elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                    "mpv created"
                );
                let d = self.display.clone();
                let tx = self.self_tx.clone();
                p.attach_gl(
                    move |name| d.get_proc_address(name),
                    move || {
                        let _ = tx.send(RenderCmd::MediaFrame);
                    },
                )?;
                Ok(p)
            });
            match player {
                Ok(p) => {
                    self.players.push(p);
                    tracing::info!(
                        elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                        "media player ready"
                    );
                }
                Err(e) => {
                    self.report(&e);
                    self.players.clear();
                    self.unavailable_reported = true;
                    return false;
                }
            }
        }
        true
    }

    /// Whether idle warm-up should still happen (libmpv present, players not made yet).
    fn wants_warm_up(&self) -> bool {
        self.players.is_empty() && !self.unavailable_reported && !self.warm_up_tried
    }

    fn warm_up(&mut self) {
        self.warm_up_tried = true;
        if media::available().is_ok() {
            self.ensure_players();
        }
    }

    fn apply(&mut self, current: Option<PathBuf>, next: Option<PathBuf>) {
        if current.is_none() && next.is_none() && self.players.is_empty() {
            return;
        }
        if !self.ensure_players() {
            return;
        }
        let a = self.active;
        let sb = 1 - a;
        tracing::debug!(?current, ?next, active = ?self.paths[a], standby = ?self.paths[sb], "media apply");
        let result = (|| -> Result<(), media::MediaError> {
            match &current {
                None => {
                    if self.paths[a].is_some() {
                        self.players[a].stop()?;
                    }
                    self.paths[a] = None;
                    self.has_frame = false;
                }
                Some(p) if self.paths[a].as_ref() == Some(p) => self.players[a].play()?,
                Some(p) if self.paths[sb].as_ref() == Some(p) => {
                    // Pre-loaded: swap in instantly.
                    self.active = sb;
                    self.players[sb].play()?;
                    self.players[a].stop()?;
                    self.paths[a] = None;
                    self.has_frame = false;
                    tracing::info!(path = %p.display(), "background switched (pre-loaded)");
                }
                Some(p) => {
                    self.players[a].load(p, false)?;
                    self.paths[a] = Some(p.clone());
                    self.has_frame = false;
                }
            }
            let sb = 1 - self.active;
            if next.as_ref() != self.paths[sb].as_ref()
                && next.as_ref() != self.paths[self.active].as_ref()
            {
                match &next {
                    Some(n) => self.players[sb].load(n, true)?,
                    None if self.paths[sb].is_some() => self.players[sb].stop()?,
                    None => {}
                }
                self.paths[sb] = next.clone();
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.report(&e);
        }
    }

    fn set_audio_device(&mut self, d: Option<String>) {
        for p in &self.players {
            if let Err(e) = p.set_audio_device(d.as_deref()) {
                self.report(&e);
            }
        }
        self.audio_device = d;
    }

    /// Render the active player into the framebuffer if it has a new frame (or
    /// `force`). Returns whether anything was drawn.
    fn render(&mut self, gl: &glow::Context, rect: GlRect, force: bool) -> bool {
        if self.players.is_empty() {
            return false;
        }
        // Every render context must be updated after its callback fires.
        let mut active_new = false;
        for (i, p) in self.players.iter().enumerate() {
            let upd = p.needs_render();
            if i == self.active {
                active_new = upd;
            }
        }
        if self.paths[self.active].is_none() || !(active_new || force) {
            return false;
        }
        let Some(fb) = self.ensure_target(gl, rect.w, rect.h) else {
            return false;
        };
        match self.players[self.active].render(fb.0, rect.w, rect.h, false) {
            Ok(()) => {
                self.has_frame = true;
                true
            }
            Err(e) => {
                self.report(&e);
                false
            }
        }
    }

    /// (framebuffer id, ...) sized `w × h`.
    fn ensure_target(&mut self, gl: &glow::Context, w: u32, h: u32) -> Option<(u32,)> {
        if self.target.as_ref().is_some_and(|t| t.w == w && t.h == h) {
            return self.target.as_ref().map(|t| (t.fb.0.get(),));
        }
        if let Some(old) = self.target.take() {
            unsafe {
                gl.delete_framebuffer(old.fb);
                gl.delete_texture(old.tex);
            }
        }
        unsafe {
            let tex = gl.create_texture().ok()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            set_tex_params(gl, glow::LINEAR);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                w as i32,
                h as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            let fb = gl.create_framebuffer().ok()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fb));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(tex),
                0,
            );
            let ok = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            if !ok {
                gl.delete_framebuffer(fb);
                gl.delete_texture(tex);
                self.report(&media::MediaError::Render("framebuffer incomplete".into()));
                return None;
            }
            self.target = Some(Fbo { tex, fb, w, h });
            Some((fb.0.get(),))
        }
    }

    fn texture(&self) -> Option<glow::Texture> {
        (self.has_frame && self.paths[self.active].is_some())
            .then(|| self.target.as_ref().map(|t| t.tex))
            .flatten()
    }

    fn destroy(&mut self, gl: &glow::Context) {
        self.players.clear(); // frees mpv render contexts (GL context is current)
        if let Some(t) = self.target.take() {
            unsafe {
                gl.delete_framebuffer(t.fb);
                gl.delete_texture(t.tex);
            }
        }
    }
}
