//! Raster worker: follows the core's snapshots directly (not via the UI thread), turns
//! the live slide into an output-sized frame, keeps a small cache, and pre-renders the
//! neighbours in play order so a slide change is just a texture swap.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select};
use opcore::{ShowState, prev_enabled};
use render::{Fonts, SlideFrame, render_slide};

use crate::UserEvent;

/// Cache key: which slide of which loaded document, at which output size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameKey {
    /// Identity of the loaded document (`Arc` pointer): a reload is a new key.
    pub doc: usize,
    pub flat: usize,
    /// The size the frame was rendered at (the output's, or larger when stretched).
    pub width: u32,
    pub height: u32,
    /// Draw the frame over the whole output, whatever its shape (Settings → Output →
    /// Slides: Stretch). Otherwise it's drawn 1:1, centred.
    pub stretch: bool,
}

/// Commands to the output render thread.
pub enum RenderCmd {
    /// Show this frame now. `requested_at` is when the operator asked (for latency).
    /// `layers` = (text, background) visibility, applied in the same present so a
    /// cleared slide can never flash back before the new one is up.
    Show {
        key: FrameKey,
        frame: Arc<SlideFrame>,
        layers: (bool, bool),
        requested_at: Option<Instant>,
    },
    /// Upload ahead of time so a later `Show` is instant.
    Preload {
        key: FrameKey,
        frame: Arc<SlideFrame>,
    },
    /// Show nothing (black): no live slide, or it couldn't be rendered.
    Blank {
        layers: (bool, bool),
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
    /// Background media to show (and to pre-load next). `cue`: how it plays when it's
    /// a slide's video (`None` = loop at full volume); `restart` changes to start it
    /// again from the beginning.
    Media {
        current: Option<PathBuf>,
        next: Option<PathBuf>,
        cue: Option<doc::SlideVideo>,
        paused: bool,
        restart: u64,
    },
    /// mpv has a new frame (posted from mpv's thread).
    MediaFrame,
    AudioDevice(Option<String>),
    MediaScaling(media::Scaling),
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
    /// Slides stretch to the output's shape (`true`) or keep their own (`false`).
    Stretch(bool),
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
    fonts_for_others: Vec<Sender<Fonts>>,
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
            for tx in &fonts_for_others {
                let _ = tx.send(fonts.fork());
            }
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
                stretch: false,
                sent: Sent::default(),
                hold: Hold::default(),
            };
            w.run(&snapshots, &rx);
        })?;
    Ok(Spawned { tx, thread })
}

/// What the output currently has, to send only changes.
#[derive(Default, Clone)]
struct Sent {
    /// The output was blanked (see `Command::Blank`).
    blank: bool,
    live: Option<(usize, usize)>,
    layers: Option<(bool, bool)>,
    media: Option<MediaSent>,
}

type MediaSent = (
    Option<PathBuf>,
    Option<PathBuf>,
    Option<doc::SlideVideo>,
    bool,
    u64,
);

/// A rendered frame, or the operator message for a slide that can't be drawn.
type Rendered = Result<Arc<SlideFrame>, String>;

struct Worker {
    fonts: Fonts,
    /// Failures are cached too, so pre-rendering doesn't retry and re-report them.
    cache: VecDeque<(FrameKey, Rendered)>,
    output: OutputLink,
    events: winit::event_loop::EventLoopProxy<UserEvent>,
    size: Option<(u32, u32)>,
    stretch: bool,
    sent: Sent,
    hold: Hold,
}

/// What the output should show for a show state: freeze and blank sit between the
/// operator's state and the screen.
#[derive(Default)]
struct Hold {
    /// What the output shows (`None` while blanked).
    shown: Option<Arc<ShowState>>,
    /// While frozen: the state the output holds.
    held: Option<Arc<ShowState>>,
}

enum View {
    Blank,
    Show(Arc<ShowState>),
}

impl Hold {
    /// Blanked: black and silent. Frozen: what the output showed when the freeze
    /// began (or what was live, when frozen while blanked). Otherwise `s`.
    fn view(&mut self, s: &Arc<ShowState>) -> View {
        if !s.frozen {
            self.held = None;
        } else if self.held.is_none() {
            self.held = Some(self.shown.clone().unwrap_or_else(|| s.clone()));
        }
        if s.blanked {
            self.shown = None;
            return View::Blank;
        }
        let shown = self.held.clone().unwrap_or_else(|| s.clone());
        self.shown = Some(shown.clone());
        View::Show(shown)
    }
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
                    Ok(RasterCmd::Stretch(on)) => {
                        if self.stretch != on {
                            self.stretch = on;
                            if let Some(s) = &latest {
                                self.apply(s, true);
                            }
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
    /// Blanked: black and silent. Frozen: the output keeps what it showed when the
    /// freeze began; unfreezing or unblanking shows what's live by then.
    fn apply(&mut self, s: &Arc<ShowState>, force: bool) {
        if self.size.is_none() {
            return;
        }
        if force {
            self.sent = Sent::default();
        }
        match self.hold.view(s) {
            View::Blank => self.apply_blank(s, force),
            View::Show(shown) => {
                self.sent.blank = false;
                self.apply_state(&shown, force);
            }
        }
    }

    /// Black, no media (once; a forced re-send does it again).
    fn apply_blank(&mut self, s: &ShowState, force: bool) {
        if self.sent.blank && !force {
            return;
        }
        let requested_at = if force { None } else { s.command_at };
        let layers = (false, false);
        self.send(RenderCmd::Blank {
            layers,
            requested_at,
        });
        let media: MediaSent = (None, None, None, false, s.media_restart);
        self.send(RenderCmd::Media {
            current: None,
            next: None,
            cue: None,
            paused: false,
            restart: media.4,
        });
        self.sent = Sent {
            blank: true,
            live: None,
            layers: Some(layers),
            media: Some(media),
        };
    }

    fn apply_state(&mut self, s: &ShowState, force: bool) {
        self.apply_live(s, force);
        let media: MediaSent = (
            s.background_media.clone(),
            s.background_next.clone(),
            s.background_cue.as_ref().map(|c| c.video.clone()),
            s.media_paused,
            s.media_restart,
        );
        if self.sent.media.as_ref() != Some(&media) {
            self.send(RenderCmd::Media {
                current: media.0.clone(),
                next: media.1.clone(),
                cue: media.2.clone(),
                paused: media.3,
                restart: media.4,
            });
            self.sent.media = Some(media);
        }
    }

    fn apply_live(&mut self, s: &ShowState, force: bool) {
        let Some(size) = self.size else { return };
        // A forced re-send (new window / size) isn't an operator action: no latency stamp.
        let requested_at = if force { None } else { s.command_at };
        let layers = (s.text_visible, s.background_visible);
        let live = s
            .live
            .as_ref()
            .map(|l| (Arc::as_ptr(&l.doc) as usize, l.slide.flat));
        let prev = std::mem::replace(&mut self.sent.live, live);
        if prev == live && !force {
            // Same slide: only visibility may have changed (Clear Text, etc.).
            if self.sent.layers != Some(layers) {
                self.sent.layers = Some(layers);
                self.send(RenderCmd::Layers {
                    text: layers.0,
                    background: layers.1,
                    requested_at,
                });
            }
            return;
        }
        // New slide: its visibility travels with it (one present, never the old slide).
        self.sent.layers = Some(layers);
        let Some(l) = &s.live else {
            // Went from live to nothing (or a fresh window): show black.
            self.send(RenderCmd::Blank {
                layers,
                requested_at,
            });
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
                    layers,
                    requested_at,
                });
            }
            (_, Err(message)) => {
                let _ = self
                    .events
                    .send_event(UserEvent::RenderFailed { index, message });
                self.send(RenderCmd::Blank {
                    layers,
                    requested_at,
                });
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
        doc: &Arc<doc::Document>,
        flat: usize,
        output: (u32, u32),
    ) -> (FrameKey, Rendered) {
        let (width, height) = if self.stretch {
            stretch_size(doc.canvas, output)
        } else {
            output
        };
        let key = FrameKey {
            doc: Arc::as_ptr(doc) as usize,
            flat,
            width,
            height,
            stretch: self.stretch,
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

/// The size to render a slide at when it will be stretched over a `w × h` output:
/// the canvas's own shape, scaled until it covers the output on both axes. The GPU
/// then only ever shrinks it (sharper text than enlarging), on one axis.
fn stretch_size(canvas: doc::CanvasSize, (w, h): (u32, u32)) -> (u32, u32) {
    let (cw, ch) = (canvas.width.max(1) as f32, canvas.height.max(1) as f32);
    let scale = (w as f32 / cw).max(h as f32 / ch);
    (
        ((cw * scale).round() as u32).max(w),
        ((ch * scale).round() as u32).max(h),
    )
}

pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stretched_frames_cover_the_output_in_the_canvas_shape() {
        let c = |width, height| doc::CanvasSize { width, height };
        // 4:3 slide on a 16:9 signal: rendered 1920 × 1440, squeezed to 1080 tall.
        assert_eq!(stretch_size(c(1024, 768), (1920, 1080)), (1920, 1440));
        // 16:9 slide on a 4:3 output: rendered wider, squeezed to 1024 wide.
        assert_eq!(stretch_size(c(1920, 1080), (1024, 768)), (1365, 768));
        // Same shape: exactly the output.
        assert_eq!(stretch_size(c(1024, 768), (1024, 768)), (1024, 768));
        assert_eq!(stretch_size(c(1024, 768), (2048, 1536)), (2048, 1536));
    }

    fn state(slide: u64, frozen: bool, blanked: bool) -> Arc<ShowState> {
        Arc::new(ShowState {
            seq: slide,
            frozen,
            blanked,
            ..ShowState::default()
        })
    }

    fn seq(v: View) -> Option<u64> {
        match v {
            View::Blank => None,
            View::Show(s) => Some(s.seq),
        }
    }

    #[test]
    fn freeze_holds_the_screen_and_blank_blacks_it() {
        let mut h = Hold::default();
        assert_eq!(seq(h.view(&state(1, false, false))), Some(1));
        // Frozen on slide 1: moving on doesn't reach the screen.
        assert_eq!(seq(h.view(&state(1, true, false))), Some(1));
        assert_eq!(seq(h.view(&state(4, true, false))), Some(1));
        // Blank wins over freeze; unblanking (still frozen) brings back the hold.
        assert_eq!(seq(h.view(&state(5, true, true))), None);
        assert_eq!(seq(h.view(&state(5, true, false))), Some(1));
        // Unfrozen: what's live now.
        assert_eq!(seq(h.view(&state(6, false, false))), Some(6));
        // Blanked: black whatever goes live; unblanked: what's live by then.
        assert_eq!(seq(h.view(&state(7, false, true))), None);
        assert_eq!(seq(h.view(&state(8, false, true))), None);
        assert_eq!(seq(h.view(&state(9, false, false))), Some(9));
        // Frozen while blanked: holds what was live then.
        assert_eq!(seq(h.view(&state(10, false, true))), None);
        assert_eq!(seq(h.view(&state(10, true, true))), None);
        assert_eq!(seq(h.view(&state(12, true, false))), Some(10));
    }
}
