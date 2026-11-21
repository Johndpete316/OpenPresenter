//! The editor canvas image: one slide rendered by `render` (the output's renderer,
//! so the editor is WYSIWYG) at the canvas's on-screen pixel size, on its own
//! thread. Only the newest request is rendered; while it renders, the previous image
//! stays on screen, so dragging never waits on the renderer.

use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use egui::TextureHandle;
use render::{Fonts, render_slide};

use crate::UserEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanvasKey {
    /// Document identity (`Arc` pointer; the request keeps it alive).
    doc: usize,
    flat: usize,
    width: u32,
    height: u32,
}

struct Request {
    key: CanvasKey,
    doc: Arc<doc::Document>,
    flat: usize,
}

struct Done {
    key: CanvasKey,
    image: Result<egui::ColorImage, String>,
    /// Text boxes (UUIDs) whose text is taller than the box.
    overflow: Vec<String>,
}

pub struct CanvasRenderer {
    tx: Sender<Request>,
    rx: Receiver<Done>,
    /// Last request (kept so its document pointer stays unique).
    wanted: Option<(CanvasKey, Arc<doc::Document>)>,
    shown: Option<CanvasKey>,
    texture: Option<TextureHandle>,
    error: Option<String>,
    overflow: Vec<String>,
}

impl CanvasRenderer {
    pub fn spawn(
        fonts: Receiver<Fonts>,
        events: winit::event_loop::EventLoopProxy<UserEvent>,
    ) -> std::io::Result<CanvasRenderer> {
        let (tx, req_rx) = crossbeam_channel::unbounded::<Request>();
        let (done_tx, rx) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("editor-canvas".into())
            .spawn(move || {
                let Ok(mut fonts) = fonts.recv() else { return };
                while let Ok(mut req) = req_rx.recv() {
                    // Skip to the newest request.
                    while let Ok(newer) = req_rx.try_recv() {
                        req = newer;
                    }
                    let mut overflow = Vec::new();
                    let image = req
                        .doc
                        .slides()
                        .nth(req.flat)
                        .ok_or_else(|| "slide missing".to_owned())
                        .and_then(|slide| {
                            render_slide(&mut fonts, &req.doc, slide, req.key.width, req.key.height)
                                .map_err(|e| e.user_message())
                        })
                        .map(|frame| {
                            overflow = frame
                                .warnings
                                .iter()
                                .filter(|w| w.overflow)
                                .map(|w| w.element_uuid.clone())
                                .collect();
                            let img = frame.composite();
                            egui::ColorImage::from_rgba_unmultiplied(
                                [img.width() as usize, img.height() as usize],
                                img.as_raw(),
                            )
                        });
                    if done_tx
                        .send(Done {
                            key: req.key,
                            image,
                            overflow,
                        })
                        .is_err()
                    {
                        return;
                    }
                    let _ = events.send_event(UserEvent::ThumbReady);
                }
            })?;
        Ok(CanvasRenderer {
            tx,
            rx,
            wanted: None,
            shown: None,
            texture: None,
            error: None,
            overflow: Vec::new(),
        })
    }

    /// Ask for slide `flat` of `doc` at `width × height` pixels (no-op if that's
    /// already shown or on its way).
    pub fn request(&mut self, doc: &Arc<doc::Document>, flat: usize, width: u32, height: u32) {
        let key = CanvasKey {
            doc: Arc::as_ptr(doc) as usize,
            flat,
            width: width.clamp(16, 4096),
            height: height.clamp(16, 4096),
        };
        if self.wanted.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        self.wanted = Some((key, doc.clone()));
        let _ = self.tx.send(Request {
            key,
            doc: doc.clone(),
            flat,
        });
    }

    /// Take finished images.
    pub fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(d) = self.rx.try_recv() {
            match d.image {
                Ok(img) => {
                    match &mut self.texture {
                        Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                        None => {
                            self.texture = Some(ctx.load_texture(
                                "editor-canvas",
                                img,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                    }
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
            self.overflow = d.overflow;
            self.shown = Some(d.key);
        }
    }

    /// The latest image (possibly of an earlier request) and, if the latest render
    /// failed, why.
    pub fn image(&self) -> (Option<&TextureHandle>, Option<&str>) {
        (self.texture.as_ref(), self.error.as_deref())
    }

    /// Text boxes (UUIDs) overflowing in the latest image.
    pub fn overflow(&self) -> &[String] {
        &self.overflow
    }
}
