//! Raster worker: turns slides into output-sized frames off every show-critical
//! thread, keeps a small cache, and pre-renders the neighbours of the current slide
//! so a slide change is just a texture swap.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use render::{Fonts, SlideFrame, render_slide};

use crate::UserEvent;

/// Cache key: which slide at which output size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameKey {
    pub index: usize,
    pub width: u32,
    pub height: u32,
}

/// Commands to the output render thread.
pub enum RenderCmd {
    /// Show this frame now. `requested_at` is when the operator asked (for latency).
    Show {
        key: FrameKey,
        frame: Arc<SlideFrame>,
        requested_at: Option<Instant>,
    },
    /// Upload ahead of time so a later `Show` is instant.
    Preload {
        key: FrameKey,
        frame: Arc<SlideFrame>,
    },
    /// Show nothing (black), e.g. the slide couldn't be rendered.
    Blank {
        requested_at: Option<Instant>,
    },
    Resize {
        width: u32,
        height: u32,
    },
    Redraw,
    /// Read back what's on screen and save it as PNG (verification tool).
    Capture(std::path::PathBuf),
    Stop,
}

/// The current output's command channel; `None` while the output is off.
pub type OutputLink = Arc<Mutex<Option<Sender<RenderCmd>>>>;

pub enum RasterCmd {
    Show {
        index: usize,
        requested_at: Option<Instant>,
    },
    /// Output size changed (or a new output window exists): re-send the current slide.
    Size {
        width: u32,
        height: u32,
    },
    Stop,
}

const CACHE: usize = 6;

pub fn spawn(
    doc: Arc<pro6::Document>,
    font_dirs: Vec<PathBuf>,
    output: OutputLink,
    events: winit::event_loop::EventLoopProxy<UserEvent>,
) -> std::io::Result<(Sender<RasterCmd>, JoinHandle<()>)> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let handle = std::thread::Builder::new()
        .name("raster".into())
        .spawn(move || run(&doc, &font_dirs, &output, &events, &rx))?;
    Ok((tx, handle))
}

fn run(
    doc: &pro6::Document,
    font_dirs: &[PathBuf],
    output: &OutputLink,
    events: &winit::event_loop::EventLoopProxy<UserEvent>,
    rx: &Receiver<RasterCmd>,
) {
    let started = Instant::now();
    let mut fonts = Fonts::system();
    for d in font_dirs {
        fonts.add_dir(d);
    }
    let _ = events.send_event(UserEvent::FontsReady {
        elapsed: started.elapsed(),
        faces: fonts.face_count(),
    });

    let mut w = Worker {
        doc,
        slides: doc.slides().collect(),
        fonts,
        cache: VecDeque::new(),
        output,
        events,
    };
    let mut size: Option<(u32, u32)> = None;
    let mut current: Option<usize> = None;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            RasterCmd::Show {
                index,
                requested_at,
            } => {
                current = Some(index);
                if let Some(sz) = size {
                    w.show(index, sz, requested_at);
                }
            }
            RasterCmd::Size { width, height } => {
                size = Some((width, height));
                if let Some(i) = current {
                    w.show(i, (width, height), None);
                }
            }
            RasterCmd::Stop => break,
        }
    }
}

/// A rendered frame, or the operator message for a slide that can't be drawn.
type Rendered = Result<Arc<SlideFrame>, String>;

struct Worker<'a> {
    doc: &'a pro6::Document,
    slides: Vec<&'a pro6::Slide>,
    fonts: Fonts,
    /// Failures are cached too, so pre-rendering doesn't retry and re-report them.
    cache: VecDeque<(FrameKey, Rendered)>,
    output: &'a OutputLink,
    events: &'a winit::event_loop::EventLoopProxy<UserEvent>,
}

impl Worker<'_> {
    fn send(&self, cmd: RenderCmd) {
        if let Ok(guard) = self.output.lock()
            && let Some(tx) = guard.as_ref()
        {
            let _ = tx.send(cmd);
        }
    }

    /// Show `index` now, then pre-render its neighbours.
    fn show(&mut self, index: usize, size: (u32, u32), requested_at: Option<Instant>) {
        match self.frame(index, size) {
            Some((key, Ok(frame))) => {
                tracing::debug!(
                    slide = index + 1,
                    render_ms = ms(frame.render_time),
                    "frame ready"
                );
                self.send(RenderCmd::Show {
                    key,
                    frame,
                    requested_at,
                });
            }
            Some((_, Err(message))) => {
                let _ = self
                    .events
                    .send_event(UserEvent::RenderFailed { index, message });
                self.send(RenderCmd::Blank { requested_at });
            }
            None => self.send(RenderCmd::Blank { requested_at }),
        }
        // Next first: it's the likely next press. Failures stay quiet until shown.
        for n in [index.checked_add(1), index.checked_sub(1)]
            .into_iter()
            .flatten()
        {
            if n < self.slides.len()
                && let Some((key, Ok(frame))) = self.frame(n, size)
            {
                self.send(RenderCmd::Preload { key, frame });
            }
        }
    }

    /// Cached or freshly rendered frame. `None` only if `index` is out of range.
    fn frame(&mut self, index: usize, (width, height): (u32, u32)) -> Option<(FrameKey, Rendered)> {
        let key = FrameKey {
            index,
            width,
            height,
        };
        if let Some(pos) = self.cache.iter().position(|(k, _)| *k == key) {
            let hit = self.cache.remove(pos)?;
            self.cache.push_back(hit.clone());
            return Some(hit);
        }
        let slide = self.slides.get(index)?;
        let result = match render_slide(&mut self.fonts, self.doc, slide, width, height) {
            Ok(frame) => {
                for w in &frame.warnings {
                    tracing::info!(slide = index + 1, warning = %w.message, "render warning");
                }
                Ok(Arc::new(frame))
            }
            Err(e) => Err(e.user_message()),
        };
        self.cache.push_back((key, result.clone()));
        while self.cache.len() > CACHE {
            self.cache.pop_front();
        }
        Some((key, result))
    }
}

pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
