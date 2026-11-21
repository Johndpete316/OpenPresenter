//! Thumbnails for the Video/Image panel, made on a background thread and cached on
//! disk. Images are decoded directly; videos (and images the decoder can't read) get
//! a frame from a short-lived headless mpv.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

use crossbeam_channel::{Receiver, Sender};
use egui::TextureHandle;

use crate::UserEvent;

/// Thumbnails fit in this box (2× the tile, sharp on high-DPI screens).
const BOX_W: u32 = 256;
const BOX_H: u32 = 144;
/// Bump when thumbnails are made differently so old disk copies are ignored.
const VERSION: u32 = 1;
/// Videos wait for the output to finish its own mpv start-up (or this long).
const MPV_WAIT: Duration = Duration::from_secs(8);

struct Done {
    path: PathBuf,
    image: Result<egui::ColorImage, String>,
}

/// The UI side: requests thumbnails for tiles on screen and holds their textures.
pub struct MediaThumbs {
    tx: Sender<PathBuf>,
    rx: Receiver<Done>,
    textures: HashMap<PathBuf, Result<TextureHandle, String>>,
    requested: HashSet<PathBuf>,
}

impl MediaThumbs {
    /// The texture for `path` once it's made (asks for it the first time).
    pub fn get(&mut self, path: &Path) -> Option<&Result<TextureHandle, String>> {
        if !self.requested.contains(path) {
            self.requested.insert(path.to_path_buf());
            let _ = self.tx.send(path.to_path_buf());
        }
        self.textures.get(path)
    }

    /// Take finished thumbnails into textures.
    pub fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(d) = self.rx.try_recv() {
            let tex = d.image.map(|img| {
                ctx.load_texture(
                    format!("media-{}", d.path.display()),
                    img,
                    egui::TextureOptions::LINEAR,
                )
            });
            self.textures.insert(d.path, tex);
        }
        if self.textures.len() > 3000 {
            self.textures.clear();
            self.requested.clear();
        }
    }
}

/// Start the worker. `mpv_ready` is set once the output has loaded libmpv.
pub fn spawn(
    cache_dir: PathBuf,
    mpv_ready: Arc<AtomicBool>,
    events: winit::event_loop::EventLoopProxy<UserEvent>,
) -> std::io::Result<MediaThumbs> {
    let (tx, req_rx) = crossbeam_channel::unbounded::<PathBuf>();
    let (done_tx, rx) = crossbeam_channel::unbounded();
    let started = Instant::now();
    std::thread::Builder::new()
        .name("media-thumbnails".into())
        .spawn(move || {
            let _ = std::fs::create_dir_all(&cache_dir);
            let mut queue: Vec<PathBuf> = Vec::new();
            loop {
                if queue.is_empty() {
                    match req_rx.recv() {
                        Ok(p) => queue.push(p),
                        Err(_) => return,
                    }
                }
                queue.extend(req_rx.try_iter());
                // Newest first: what's on screen now, not what scrolled past.
                let Some(path) = queue.pop() else { continue };
                let image = make(&cache_dir, &path, || {
                    while !mpv_ready.load(Ordering::Relaxed) && started.elapsed() < MPV_WAIT {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                });
                if let Err(e) = &image {
                    tracing::debug!(path = %path.display(), error = %e, "no media thumbnail");
                }
                if done_tx.send(Done { path, image }).is_err() {
                    return;
                }
                let _ = events.send_event(UserEvent::ThumbReady);
            }
        })?;
    Ok(MediaThumbs {
        tx,
        rx,
        textures: HashMap::new(),
        requested: HashSet::new(),
    })
}

fn disk_name(path: &Path, box_w: u32, box_h: u32) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    let mut h = DefaultHasher::new();
    (VERSION, path, mtime, meta.len(), box_w, box_h).hash(&mut h);
    Some(format!("m{:016x}.png", h.finish()))
}

fn to_color(img: &image::RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    )
}

fn make(cache: &Path, path: &Path, wait_for_mpv: impl Fn()) -> Result<egui::ColorImage, String> {
    make_rgba(cache, path, BOX_W, BOX_H, wait_for_mpv).map(|img| to_color(&img))
}

/// A poster frame for the phone remote, `width` wide (16:9 box), cached with the
/// panel's thumbnails.
pub fn poster(
    cache: &Path,
    path: &Path,
    width: u32,
    mpv_ready: &AtomicBool,
    started: Instant,
) -> Result<image::RgbaImage, String> {
    let _ = std::fs::create_dir_all(cache);
    make_rgba(cache, path, width, width * 9 / 16, || {
        while !mpv_ready.load(Ordering::Relaxed) && started.elapsed() < MPV_WAIT {
            std::thread::sleep(Duration::from_millis(100));
        }
    })
}

fn make_rgba(
    cache: &Path,
    path: &Path,
    box_w: u32,
    box_h: u32,
    wait_for_mpv: impl Fn(),
) -> Result<image::RgbaImage, String> {
    let disk = disk_name(path, box_w, box_h)
        .map(|n| cache.join(n))
        .ok_or("file not found")?;
    if let Ok(img) = image::open(&disk) {
        return Ok(img.to_rgba8());
    }
    let img = match image::open(path) {
        Ok(img) => img.thumbnail(box_w, box_h).to_rgba8(),
        Err(decode_err) => {
            wait_for_mpv();
            media::thumbnail(path, &disk, box_w).map_err(|e| {
                if is_video(path) {
                    e.user_message()
                } else {
                    decode_err.to_string()
                }
            })?;
            let img = image::open(&disk).map_err(|e| e.to_string())?;
            // mpv scaled to the width; a tall video still has to fit the box.
            img.thumbnail(box_w, box_h).to_rgba8()
        }
    };
    if let Err(e) = img.save(&disk) {
        tracing::debug!(path = %disk.display(), error = %e, "couldn't cache media thumbnail");
    }
    Ok(img)
}

fn is_video(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["mp4", "mov", "m4v", "mkv", "avi", "wmv", "webm"]
            .iter()
            .any(|x| e.eq_ignore_ascii_case(x))
    })
}
