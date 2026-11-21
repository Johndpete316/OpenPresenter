//! Slide thumbnails, generated on a background thread with the same renderer as the
//! output (so tiles show real fonts and positions), cached in memory and on disk.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use crossbeam_channel::{Receiver, Sender};
use render::{Fonts, render_slide};

use crate::UserEvent;

/// Bump when rendering changes so stale disk thumbnails are ignored.
const RENDER_VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThumbKey {
    /// Loaded document identity (`Arc` pointer).
    pub doc: usize,
    pub flat: usize,
    pub width: u32,
    pub height: u32,
    /// Disk-cached (saved document) vs rendered fresh (editor copy, with overflow).
    pub disk: bool,
}

pub struct ThumbRequest {
    pub key: ThumbKey,
    pub doc: Arc<doc::Document>,
    /// Use the disk cache (only for saved documents: it's keyed by the file).
    pub disk: bool,
}

pub struct ThumbResult {
    pub key: ThumbKey,
    pub image: Result<egui::ColorImage, String>,
    /// Some text box overflows (known only for fresh renders).
    pub overflow: bool,
}

pub fn spawn(
    fonts: Receiver<Fonts>,
    cache_dir: Option<PathBuf>,
    events: winit::event_loop::EventLoopProxy<UserEvent>,
) -> std::io::Result<(Sender<ThumbRequest>, Receiver<ThumbResult>)> {
    let (req_tx, req_rx) = crossbeam_channel::unbounded::<ThumbRequest>();
    let (res_tx, res_rx) = crossbeam_channel::unbounded();
    std::thread::Builder::new()
        .name("thumbnails".into())
        .spawn(move || {
            let Ok(mut fonts) = fonts.recv() else { return };
            if let Some(d) = &cache_dir {
                let _ = std::fs::create_dir_all(d);
            }
            while let Ok(req) = req_rx.recv() {
                let (image, overflow) =
                    thumbnail(&mut fonts, cache_dir.as_deref().filter(|_| req.disk), &req);
                if res_tx
                    .send(ThumbResult {
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
    Ok((req_tx, res_rx))
}

fn disk_key(req: &ThumbRequest) -> Option<String> {
    let meta = std::fs::metadata(&req.doc.path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    let mut h = DefaultHasher::new();
    (
        RENDER_VERSION,
        &req.doc.path,
        mtime,
        meta.len(),
        req.key.flat,
        req.key.width,
        req.key.height,
    )
        .hash(&mut h);
    Some(format!("{:016x}.png", h.finish()))
}

fn thumbnail(
    fonts: &mut Fonts,
    cache_dir: Option<&std::path::Path>,
    req: &ThumbRequest,
) -> (Result<egui::ColorImage, String>, bool) {
    let disk = cache_dir.zip(disk_key(req)).map(|(d, k)| d.join(k));
    if let Some(p) = &disk
        && let Ok(img) = image::open(p)
    {
        let rgba = img.to_rgba8();
        return (
            Ok(egui::ColorImage::from_rgba_unmultiplied(
                [rgba.width() as usize, rgba.height() as usize],
                rgba.as_raw(),
            )),
            false,
        );
    }
    let Some(slide) = req.doc.slides().nth(req.key.flat) else {
        return (Err("slide missing".into()), false);
    };
    let frame = match render_slide(fonts, &req.doc, slide, req.key.width, req.key.height) {
        Ok(f) => f,
        Err(e) => return (Err(e.user_message()), false),
    };
    let overflow = frame.warnings.iter().any(|w| w.overflow);
    let img = frame.composite();
    if let Some(p) = &disk
        && let Err(e) = img.save(p)
    {
        tracing::debug!(path = %p.display(), error = %e, "couldn't cache thumbnail");
    }
    (
        Ok(egui::ColorImage::from_rgba_unmultiplied(
            [img.width() as usize, img.height() as usize],
            img.as_raw(),
        )),
        overflow,
    )
}
