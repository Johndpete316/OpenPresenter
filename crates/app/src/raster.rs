//! Raster worker: follows the core's snapshots directly (not via the UI thread), turns
//! the live slide into an output-sized frame, keeps a small cache, and pre-renders the
//! neighbours in play order so a slide change is just a texture swap.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select};
use pp6core::{ShowState, prev_enabled};
use render::{Fonts, SlideFrame, render_slide};

use crate::UserEvent;

/// Cache key: which slide of which loaded document, at which output size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameKey {
    /// Identity of the loaded document (`Arc` pointer): a reload is a new key.
    pub doc: usize,
    pub flat: usize,
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
    /// Show nothing (black): no live slide, or it couldn't be rendered.
    Blank {
        requested_at: Option<Instant>,
    },
    /// Text / background layer visibility (Clear Text / Clear Background).
    Layers {
        text: bool,
        background: bool,
        requested_at: Option<Instant>,
    },
    Resize {
        width: u32,
        height: u32,
    },
    Redraw,
    /// Background media to show (and to pre-load next).
    Media {
        current: Option<PathBuf>,
        next: Option<PathBuf>,
    },
    /// mpv has a new frame (posted from mpv's thread).
    MediaFrame,
    AudioDevice(Option<String>),
    /// Read back what's on screen and save it as PNG (verification tool).
    Capture(PathBuf),
    Stop,
}

/// The current output's command channel; `None` while the output is off.
pub type OutputLink = Arc<Mutex<Option<Sender<RenderCmd>>>>;

pub enum RasterCmd {
    /// Output size changed, or a new output window exists: re-send everything.
    Size {
        width: u32,
        height: u32,
    },
    Stop,
}

const CACHE: usize = 6;

pub struct Spawned {
    pub tx: Sender<RasterCmd>,
    pub thread: JoinHandle<()>,
}

pub fn spawn(
    snapshots: Receiver<Arc<ShowState>>,
    font_dirs: Vec<PathBuf>,
    output: OutputLink,
    events: winit::event_loop::EventLoopProxy<UserEvent>,
    fonts_for_others: Sender<Fonts>,
) -> std::io::Result<Spawned> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let thread = std::thread::Builder::new()
        .name("raster".into())
        .spawn(move || {
            let started = Instant::now();
            let mut fonts = Fonts::system();
            for d in &font_dirs {
                fonts.add_dir(d);
            }
            let _ = fonts_for_others.send(fonts.fork());
            let _ = events.send_event(UserEvent::FontsReady {
                elapsed: started.elapsed(),
                faces: fonts.face_count(),
            });
            let mut w = Worker {
                fonts,
                cache: VecDeque::new(),
                output,
                events,
                size: None,
                sent: Sent::default(),
            };
            w.run(&snapshots, &rx);
        })?;
    Ok(Spawned { tx, thread })
}

/// What the output currently has, to send only changes.
#[derive(Default, Clone)]
struct Sent {
    live: Option<(usize, usize)>,
    layers: Option<(bool, bool)>,
    media: Option<(Option<PathBuf>, Option<PathBuf>)>,
}

/// A rendered frame, or the operator message for a slide that can't be drawn.
type Rendered = Result<Arc<SlideFrame>, String>;

struct Worker {
    fonts: Fonts,
    /// Failures are cached too, so pre-rendering doesn't retry and re-report them.
    cache: VecDeque<(FrameKey, Rendered)>,
    output: OutputLink,
    events: winit::event_loop::EventLoopProxy<UserEvent>,
    size: Option<(u32, u32)>,
    sent: Sent,
}

impl Worker {
    fn run(&mut self, snapshots: &Receiver<Arc<ShowState>>, rx: &Receiver<RasterCmd>) {
        let mut latest: Option<Arc<ShowState>> = None;
        loop {
            select! {
                recv(snapshots) -> s => {
                    let Ok(mut s) = s else { return };
                    while let Ok(newer) = snapshots.try_recv() {
                        s = newer;
                    }
                    self.apply(&s, false);
                    latest = Some(s);
                }
                recv(rx) -> cmd => match cmd {
                    Ok(RasterCmd::Size { width, height }) => {
                        self.size = Some((width, height));
                        if let Some(s) = &latest {
                            self.apply(s, true);
                        }
                    }
                    Ok(RasterCmd::Stop) | Err(_) => return,
                }
            }
        }
    }

    fn send(&self, cmd: RenderCmd) {
        if let Ok(guard) = self.output.lock()
            && let Some(tx) = guard.as_ref()
        {
            let _ = tx.send(cmd);
        }
    }

    /// Bring the output in line with `s`. `force` re-sends everything (new window/size).
    /// The live slide goes first so background media never delays it.
    fn apply(&mut self, s: &ShowState, force: bool) {
        if self.size.is_none() {
            return;
        }
        if force {
            self.sent = Sent::default();
        }
        self.apply_live(s, force);
        let media = (s.background_media.clone(), s.background_next.clone());
        if self.sent.media.as_ref() != Some(&media) {
            self.send(RenderCmd::Media {
                current: media.0.clone(),
                next: media.1.clone(),
            });
            self.sent.media = Some(media);
        }
    }

    fn apply_live(&mut self, s: &ShowState, force: bool) {
        let Some(size) = self.size else { return };
        // A forced re-send (new window / size) isn't an operator action: no latency stamp.
        let requested_at = if force { None } else { s.command_at };
        let layers = (s.text_visible, s.background_visible);
        if self.sent.layers != Some(layers) {
            self.sent.layers = Some(layers);
            self.send(RenderCmd::Layers {
                text: layers.0,
                background: layers.1,
                requested_at,
            });
        }
        let live = s
            .live
            .as_ref()
            .map(|l| (Arc::as_ptr(&l.doc) as usize, l.slide.flat));
        let prev = std::mem::replace(&mut self.sent.live, live);
        if prev == live && !force {
            return;
        }
        let Some(l) = &s.live else {
            // Went from live to nothing (or a fresh window): show black.
            self.send(RenderCmd::Blank { requested_at });
            return;
        };
        let index = l.slide.flat;
        match self.frame(&l.doc, index, size) {
            (key, Ok(frame)) => {
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
            (_, Err(message)) => {
                let _ = self
                    .events
                    .send_event(UserEvent::RenderFailed { index, message });
                self.send(RenderCmd::Blank { requested_at });
            }
        }
        // Next first (possibly the next playlist item's first slide): it's the likely next
        // press. Failures stay quiet until shown.
        if let Some((_, doc, _, flat)) = s.next_slide()
            && let (key, Ok(frame)) = self.frame(&doc, flat, size)
        {
            self.send(RenderCmd::Preload { key, frame });
        }
        if let Some(r) = l
            .position
            .and_then(|p| prev_enabled(&l.order, p))
            .and_then(|p| l.order.get(p))
            && let (key, Ok(frame)) = self.frame(&l.doc, r.flat, size)
        {
            self.send(RenderCmd::Preload { key, frame });
        }
    }

    fn frame(
        &mut self,
        doc: &Arc<pro6::Document>,
        flat: usize,
        (width, height): (u32, u32),
    ) -> (FrameKey, Rendered) {
        let key = FrameKey {
            doc: Arc::as_ptr(doc) as usize,
            flat,
            width,
            height,
        };
        if let Some(pos) = self.cache.iter().position(|(k, _)| *k == key)
            && let Some(hit) = self.cache.remove(pos)
        {
            self.cache.push_back(hit.clone());
            return hit;
        }
        let result = match doc.slides().nth(flat) {
            None => Err(format!("Slide {} doesn't exist.", flat + 1)),
            Some(slide) => match render_slide(&mut self.fonts, doc, slide, width, height) {
                Ok(frame) => {
                    for w in &frame.warnings {
                        tracing::info!(slide = flat + 1, warning = %w.message, "render warning");
                    }
                    Ok(Arc::new(frame))
                }
                Err(e) => Err(e.user_message()),
            },
        };
        self.cache.push_back((key, result.clone()));
        while self.cache.len() > CACHE {
            self.cache.pop_front();
        }
        (key, result)
    }
}

pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
