//! Output render thread: owns the GL context for one output window, keeps frames
//! as textures, and presents with vsync. Independent of any UI frame timing.
#![allow(unsafe_code)] // OpenGL calls are inherently unsafe; confined to this module.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::Receiver;
use glow::HasContext as _;
use glutin::context::{NotCurrentContext, NotCurrentGlContext as _, PossiblyCurrentContext};
use glutin::display::{Display, GlDisplay as _};
use glutin::surface::{GlSurface as _, Surface, SwapInterval, WindowSurface};
use render::SlideFrame;
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
    uv = vec2(p.x * 0.5, 1.0 - p.y * 0.5); // image row 0 is the top
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
    tracing::info!(
        version = unsafe { gl.get_parameter_string(glow::VERSION) },
        renderer = unsafe { gl.get_parameter_string(glow::RENDERER) },
        width,
        height,
        "output GL ready"
    );

    let swap = |r: &mut Renderer, w: u32, h: u32| -> Result<(), String> {
        r.draw(&gl, w, h);
        surface
            .swap_buffers(&ctx)
            .map_err(|e| format!("swap buffers: {e}"))
    };
    swap(&mut r, width, height)?;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            RenderCmd::Show {
                key,
                frame,
                requested_at,
            } => {
                let upload_started = Instant::now();
                r.ensure(&gl, key, frame)?;
                r.current = Some(key);
                let upload = upload_started.elapsed();
                swap(&mut r, width, height)?;
                presented(events, Some(key.index), requested_at, upload);
            }
            RenderCmd::Preload { key, frame } => r.ensure(&gl, key, frame)?,
            RenderCmd::Blank { requested_at } => {
                r.current = None;
                swap(&mut r, width, height)?;
                presented(events, None, requested_at, Default::default());
            }
            RenderCmd::Resize {
                width: w,
                height: h,
            } => {
                if let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) {
                    surface.resize(&ctx, nw, nh);
                    (width, height) = (w, h);
                    swap(&mut r, width, height)?;
                }
            }
            RenderCmd::Redraw => swap(&mut r, width, height)?,
            RenderCmd::Capture(path) => {
                r.draw(&gl, width, height);
                let result = capture(&gl, width, height, &path);
                surface
                    .swap_buffers(&ctx)
                    .map_err(|e| format!("swap buffers: {e}"))?;
                let _ = events.send_event(UserEvent::Captured { path, result });
            }
            RenderCmd::Stop => break,
        }
    }
    r.destroy(&gl);
    Ok(())
}

/// Read the back buffer (just drawn) and save it top-row-first as PNG.
fn capture(gl: &glow::Context, w: u32, h: u32, path: &std::path::Path) -> Result<(), String> {
    let mut px = vec![0u8; w as usize * h as usize * 4];
    unsafe {
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

struct Renderer {
    program: glow::Program,
    vao: glow::VertexArray,
    textures: Vec<Texture>,
    current: Option<FrameKey>,
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
            })
        }
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
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::NEAREST as i32,
            );
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

    /// Black everywhere; the slide background inside the canvas rect; text on top.
    /// A frame rendered for a different size is centred 1:1, never stretched.
    fn draw(&self, gl: &glow::Context, sw: u32, sh: u32) {
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.viewport(0, 0, sw as i32, sh as i32);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            let Some(t) = self
                .current
                .and_then(|k| self.textures.iter().find(|t| t.key == k))
            else {
                return;
            };
            let f = &t.frame;
            let ox = (sw as i32 - f.width as i32) / 2;
            // GL's origin is bottom-left.
            let oy = (sh as i32 - f.height as i32) / 2;
            let c = f.placement.canvas;
            let [r, g, b, a] = f.background.map(|v| f32::from(v) / 255.0);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(
                ox + c.x as i32,
                oy + (f.height - c.y - c.height) as i32,
                c.width as i32,
                c.height as i32,
            );
            gl.clear_color(r * a, g * a, b * a, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.disable(glow::SCISSOR_TEST);

            gl.viewport(ox, oy, f.width as i32, f.height as i32);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.use_program(Some(self.program));
            gl.bind_vertex_array(Some(self.vao));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(t.tex));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.disable(glow::BLEND);
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
