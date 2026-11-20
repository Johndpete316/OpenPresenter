//! Output window lifecycle. The window lives on the event-loop thread (winit
//! requires it); its GL context and surface are handed to a dedicated render
//! thread. Disabling the output destroys the window and releases the monitor.
#![allow(unsafe_code)] // glutin context/surface creation is unsafe; confined to `create`.

use std::thread::JoinHandle;

use crossbeam_channel::Sender;
use glutin::config::{ConfigTemplateBuilder, GlConfig as _};
use glutin::context::{ContextApi, ContextAttributesBuilder, GlProfile, Version};
use glutin::display::{GetGlDisplay as _, GlDisplay as _};
use glutin_winit::{DisplayBuilder, GlWindow as _};
use raw_window_handle::HasWindowHandle as _;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window, WindowId};

use crate::UserEvent;
use crate::error::OutputError;
use crate::gl::{self, GlParts};
use crate::raster::RenderCmd;

/// Which display to put the output on.
#[derive(Debug, Clone)]
pub enum MonitorChoice {
    /// Display name as the OS reports it (falls back to `index` if given).
    Name { name: String, index: Option<usize> },
    /// Index into the list of connected displays.
    Index(usize),
    /// A display that is neither `avoid` (the operator window's display) nor the
    /// primary one. Never covers the operator's screen by surprise: if there is no
    /// such display, the output stays off with an error.
    Secondary { avoid: Option<MonitorHandle> },
}

pub struct OutputWindow {
    pub window: Window,
    pub tx: Sender<RenderCmd>,
    thread: Option<JoinHandle<()>>,
    pub monitor_name: String,
}

pub fn describe_monitors(event_loop: &ActiveEventLoop) -> Vec<String> {
    let primary = event_loop.primary_monitor();
    event_loop
        .available_monitors()
        .enumerate()
        .map(|(i, m)| describe(i, &m, primary.as_ref() == Some(&m)))
        .collect()
}

fn describe(i: usize, m: &MonitorHandle, primary: bool) -> String {
    let s = m.size();
    let p = m.position();
    format!(
        "{i}: {} {}x{} at ({}, {}){}",
        m.name().unwrap_or_else(|| "unnamed".into()),
        s.width,
        s.height,
        p.x,
        p.y,
        if primary { " [primary]" } else { "" }
    )
}

fn pick_monitor(
    event_loop: &ActiveEventLoop,
    choice: &MonitorChoice,
) -> Result<(usize, MonitorHandle), OutputError> {
    let monitors: Vec<MonitorHandle> = event_loop.available_monitors().collect();
    if monitors.is_empty() {
        return Err(OutputError::NoMonitors);
    }
    let not_found = |requested: usize| OutputError::MonitorNotFound {
        requested,
        available: describe_monitors(event_loop),
    };
    match choice {
        MonitorChoice::Index(i) => monitors
            .get(*i)
            .cloned()
            .map(|m| (*i, m))
            .ok_or_else(|| not_found(*i)),
        MonitorChoice::Name { name, index } => monitors
            .iter()
            .position(|m| m.name().as_deref() == Some(name.as_str()))
            .or(index.filter(|i| *i < monitors.len()))
            .map(|i| (i, monitors[i].clone()))
            .ok_or_else(|| not_found(index.unwrap_or(usize::MAX))),
        MonitorChoice::Secondary { avoid } => {
            let primary = event_loop.primary_monitor();
            monitors
                .iter()
                .position(|m| Some(m) != avoid.as_ref() && Some(m) != primary.as_ref())
                .map(|i| (i, monitors[i].clone()))
                .ok_or(OutputError::NoSecondary)
        }
    }
}

impl OutputWindow {
    /// Open a borderless fullscreen output on the chosen display (or a plain window
    /// when `windowed`, for development), set up GL, and start its render thread.
    pub fn create(
        event_loop: &ActiveEventLoop,
        choice: MonitorChoice,
        windowed: Option<(u32, u32)>,
        audio_device: Option<String>,
        events: EventLoopProxy<UserEvent>,
    ) -> Result<OutputWindow, OutputError> {
        let (index, monitor) = pick_monitor(event_loop, &choice)?;
        let monitor_name = describe(
            index,
            &monitor,
            event_loop.primary_monitor().as_ref() == Some(&monitor),
        );
        let mut attrs = crate::with_icon(Window::default_attributes())
            .with_title("PP6 Output")
            .with_decorations(windowed.is_some());
        attrs = if let Some((w, h)) = windowed {
            attrs
                .with_inner_size(winit::dpi::PhysicalSize::new(w, h))
                .with_position(monitor.position())
        } else {
            attrs.with_fullscreen(Some(Fullscreen::Borderless(Some(monitor))))
        };

        let (window, config) = DisplayBuilder::new()
            .with_window_attributes(Some(attrs.clone()))
            .build(event_loop, ConfigTemplateBuilder::new(), |configs| {
                // No multisampling: frames are drawn 1:1 from pre-rendered pixels.
                #[allow(clippy::expect_used)]
                // glutin only calls the picker with at least one config
                configs
                    .min_by_key(|c| c.num_samples())
                    .expect("glutin offered no GL configs")
            })
            .map_err(|e| OutputError::Window(e.to_string()))?;
        let window = match window {
            Some(w) => w,
            None => glutin_winit::finalize_window(event_loop, attrs, &config)
                .map_err(|e| OutputError::Window(e.to_string()))?,
        };
        window.set_cursor_visible(false);

        let display = config.display();
        let raw = window
            .window_handle()
            .map_err(|e| OutputError::Window(e.to_string()))?
            .as_raw();
        let ctx_attrs = ContextAttributesBuilder::new()
            .with_profile(GlProfile::Core)
            .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
            .build(Some(raw));
        let context = unsafe { display.create_context(&config, &ctx_attrs) }
            .map_err(|e| OutputError::Gl(e.to_string()))?;
        let surface_attrs = window
            .build_surface_attributes(Default::default())
            .map_err(|e| OutputError::Gl(e.to_string()))?;
        let surface = unsafe { display.create_window_surface(&config, &surface_attrs) }
            .map_err(|e| OutputError::Gl(e.to_string()))?;

        let size = window.inner_size();
        let (tx, rx) = crossbeam_channel::unbounded();
        let parts = GlParts {
            context,
            surface,
            display,
            width: size.width.max(1),
            height: size.height.max(1),
            self_tx: tx.clone(),
            audio_device,
        };
        let thread = std::thread::Builder::new()
            .name("output-render".into())
            .spawn(move || gl::run(parts, &rx, &events))
            .map_err(|e| OutputError::Window(format!("couldn't start render thread: {e}")))?;
        tracing::info!(monitor = %monitor_name, width = size.width, height = size.height, windowed = windowed.is_some(), "output window open");
        Ok(OutputWindow {
            window,
            tx,
            thread: Some(thread),
            monitor_name,
        })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn size(&self) -> (u32, u32) {
        let s = self.window.inner_size();
        (s.width.max(1), s.height.max(1))
    }

    /// Stop the render thread (which releases GL), then drop the window so the
    /// display is released.
    pub fn destroy(mut self) {
        let _ = self.tx.send(RenderCmd::Stop);
        if let Some(t) = self.thread.take()
            && let Err(panic) = t.join()
        {
            // The render thread panicked: an unrecoverable fault. Re-raise it here.
            std::panic::resume_unwind(panic);
        }
        tracing::info!(monitor = %self.monitor_name, "output window closed");
    }
}
