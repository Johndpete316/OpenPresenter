//! The operator window: egui via egui-winit + egui_glow, on the event-loop thread.
#![allow(unsafe_code)] // glutin/glow setup is unsafe; confined to this module.

use std::num::NonZeroU32;
use std::sync::Arc;

use glow::HasContext as _;
use glutin::config::{ConfigTemplateBuilder, GlConfig as _};
use glutin::context::{
    ContextApi, ContextAttributesBuilder, GlProfile, NotCurrentGlContext as _,
    PossiblyCurrentContext, PossiblyCurrentGlContext as _, Version,
};
use glutin::display::{GetGlDisplay as _, GlDisplay as _};
use glutin::surface::{GlSurface as _, Surface, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow as _};
use raw_window_handle::HasWindowHandle as _;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::Window;

use crate::UserEvent;
use crate::error::OutputError;

pub struct OperatorWindow {
    pub window: Window,
    ctx: PossiblyCurrentContext,
    surface: Surface<WindowSurface>,
    pub egui: egui_glow::EguiGlow,
    gl: Arc<glow::Context>,
}

impl OperatorWindow {
    pub fn create(
        el: &ActiveEventLoop,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Result<OperatorWindow, OutputError> {
        let attrs = Window::default_attributes()
            .with_title("PP6")
            .with_inner_size(winit::dpi::LogicalSize::new(1400.0, 900.0));
        let (window, config) = DisplayBuilder::new()
            .with_window_attributes(Some(attrs.clone()))
            .build(el, ConfigTemplateBuilder::new(), |configs| {
                #[allow(clippy::expect_used)]
                // glutin only calls the picker with at least one config
                configs
                    .min_by_key(|c| c.num_samples())
                    .expect("glutin offered no GL configs")
            })
            .map_err(|e| OutputError::Window(e.to_string()))?;
        let window = match window {
            Some(w) => w,
            None => glutin_winit::finalize_window(el, attrs, &config)
                .map_err(|e| OutputError::Window(e.to_string()))?,
        };
        let display = config.display();
        let raw = window
            .window_handle()
            .map_err(|e| OutputError::Window(e.to_string()))?
            .as_raw();
        let ctx_attrs = ContextAttributesBuilder::new()
            .with_profile(GlProfile::Core)
            .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
            .build(Some(raw));
        let not_current = unsafe { display.create_context(&config, &ctx_attrs) }
            .map_err(|e| OutputError::Gl(e.to_string()))?;
        let surface_attrs = window
            .build_surface_attributes(Default::default())
            .map_err(|e| OutputError::Gl(e.to_string()))?;
        let surface = unsafe { display.create_window_surface(&config, &surface_attrs) }
            .map_err(|e| OutputError::Gl(e.to_string()))?;
        let ctx = not_current
            .make_current(&surface)
            .map_err(|e| OutputError::Gl(e.to_string()))?;
        // Don't block the event loop on vsync: key handling must never wait for a UI frame.
        let _ = surface.set_swap_interval(&ctx, SwapInterval::DontWait);
        let gl = Arc::new(unsafe {
            glow::Context::from_loader_function_cstr(|s| display.get_proc_address(s).cast())
        });
        let egui = egui_glow::EguiGlow::new(el, gl.clone(), None, None, true);
        egui.egui_ctx.set_request_repaint_callback(move |info| {
            let _ = proxy.send_event(UserEvent::Repaint(info.delay));
        });
        Ok(OperatorWindow {
            window,
            ctx,
            surface,
            egui,
            gl,
        })
    }

    pub fn resize(&self, width: u32, height: u32) {
        if let (Some(w), Some(h)) = (NonZeroU32::new(width), NonZeroU32::new(height)) {
            self.surface.resize(&self.ctx, w, h);
        }
    }

    /// Run one UI frame and present it. With `capture`, the frame is also saved as PNG.
    pub fn frame(&mut self, run_ui: impl FnMut(&mut egui::Ui), capture: Option<&std::path::Path>) {
        // Creating another GL context on this thread (the output window, on Windows/WGL)
        // can leave no context current; make ours current before every frame.
        if let Err(e) = self.ctx.make_current(&self.surface) {
            tracing::error!(error = %e, "operator window: couldn't make GL context current");
            return;
        }
        self.egui.run(&self.window, run_ui);
        unsafe {
            self.gl.clear_color(0.1, 0.1, 0.1, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        self.egui.paint(&self.window);
        if let Some(path) = capture {
            let size = self.window.inner_size();
            let (w, h) = (size.width, size.height);
            let mut px = vec![0u8; w as usize * h as usize * 4];
            unsafe {
                self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
                self.gl.read_pixels(
                    0,
                    0,
                    w as i32,
                    h as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::Slice(Some(&mut px)),
                );
            }
            let row = w as usize * 4;
            let flipped: Vec<u8> = px.chunks_exact(row).rev().flatten().copied().collect();
            match image::RgbaImage::from_raw(w, h, flipped).map(|i| i.save(path)) {
                Some(Ok(())) => println!("captured operator window to {}", path.display()),
                other => eprintln!("operator capture failed: {other:?}"),
            }
        }
        if let Err(e) = self.surface.swap_buffers(&self.ctx) {
            tracing::warn!(error = %e, "operator window swap failed");
        }
    }

    pub fn destroy(&mut self) {
        self.egui.destroy();
    }
}
