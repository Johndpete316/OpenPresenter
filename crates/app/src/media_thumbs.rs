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

/// Picture settings as the output's mpv gets them (whole steps of −100 … 100), so a
/// poster matches the output and a slider drag makes at most 201 versions per setting.
type Picture = [i16; 5];

fn picture(a: &doc::VideoAdjust) -> Picture {
    [a.brightness, a.contrast, a.saturation, a.gamma, a.hue]
        .map(|v| (v.clamp(-1.0, 1.0) * 100.0).round() as i16)
}

type Key = (PathBuf, Picture);

struct Done {
    key: Key,
    image: Result<egui::ColorImage, String>,
}

/// The UI side: requests thumbnails for tiles on screen and holds their textures.
pub struct MediaThumbs {
    tx: Sender<Key>,
    rx: Receiver<Done>,
    textures: HashMap<Key, Result<TextureHandle, String>>,
    requested: HashSet<Key>,
    /// The picture settings of the latest finished version of each file, shown while
    /// a different version is being made (no flicker while a slider moves).
    latest: HashMap<PathBuf, Picture>,
}

impl MediaThumbs {
    /// The texture for `path` once it's made (asks for it the first time).
    pub fn get(&mut self, path: &Path) -> Option<&Result<TextureHandle, String>> {
        self.get_adjusted(path, &doc::VideoAdjust::default())
    }

    /// The texture for `path` with a video's picture settings applied, as the output
    /// shows it (asks for it the first time; until it's made, the closest version
    /// already made).
    pub fn get_adjusted(
        &mut self,
        path: &Path,
        adjust: &doc::VideoAdjust,
    ) -> Option<&Result<TextureHandle, String>> {
        let key = (path.to_path_buf(), picture(adjust));
        if !self.requested.contains(&key) {
            self.requested.insert(key.clone());
            let _ = self.tx.send(key.clone());
        }
        if self.textures.contains_key(&key) {
            return self.textures.get(&key);
        }
        let pic = self.latest.get(path)?;
        self.textures.get(&(path.to_path_buf(), *pic))
    }

    /// Take finished thumbnails into textures.
    pub fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(d) = self.rx.try_recv() {
            let (path, pic) = &d.key;
            let tex = d.image.map(|img| {
                ctx.load_texture(
                    format!("media-{}-{pic:?}", path.display()),
                    img,
                    egui::TextureOptions::LINEAR,
                )
            });
            self.latest.insert(path.clone(), *pic);
            self.textures.insert(d.key, tex);
        }
        if self.textures.len() > 3000 {
            self.textures.clear();
            self.requested.clear();
            self.latest.clear();
        }
    }
}

/// Paint a poster into `rect` the way the output places the video: the whole picture
/// with bars (`Fit`), covering and cropped (`Fill`), or stretched.
pub fn paint_poster(painter: &egui::Painter, rect: egui::Rect, tex: &TextureHandle, fit: doc::Fit) {
    use egui::{Pos2, Rect, Vec2};
    let [w, h] = tex.size();
    let full = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    let (dest, uv) = if w == 0 || h == 0 || rect.height() <= 0.0 {
        (rect, full)
    } else {
        let pic = w as f32 / h as f32;
        let area = rect.width() / rect.height();
        match fit {
            doc::Fit::Stretch => (rect, full),
            doc::Fit::Fit => {
                let size = if pic > area {
                    Vec2::new(rect.width(), rect.width() / pic)
                } else {
                    Vec2::new(rect.height() * pic, rect.height())
                };
                (Rect::from_center_size(rect.center(), size), full)
            }
            doc::Fit::Fill => {
                let (uw, vh) = if pic > area {
                    (area / pic, 1.0)
                } else {
                    (1.0, pic / area)
                };
                (
                    rect,
                    Rect::from_center_size(Pos2::new(0.5, 0.5), Vec2::new(uw, vh)),
                )
            }
        }
    };
    painter.image(tex.id(), dest, uv, egui::Color32::WHITE);
}

/// mpv's picture settings applied to an image, the same maths as its renderer: hue
/// turns and saturation scales the colour (BT.709), contrast is a gain and
/// brightness an offset on the result, then gamma.
fn adjust_image(img: &mut egui::ColorImage, pic: Picture) {
    if pic == [0; 5] {
        return;
    }
    let [b, c, s, g, h] = pic.map(|v| f32::from(v) / 100.0);
    let brightness = b;
    let contrast = c + 1.0;
    let sat = s + 1.0;
    let hue = h * std::f32::consts::PI;
    let (hs, hc) = hue.sin_cos();
    let (hc, hs) = (hc * sat, hs * sat);
    // mpv divides by 8^gamma in the exponent: higher gamma is brighter.
    let inv_gamma = 1.0 / 8f32.powf(g);
    let mut lut = [0u8; 256];
    for (i, o) in lut.iter_mut().enumerate() {
        *o = (((i as f32 / 255.0).powf(inv_gamma)) * 255.0).round() as u8;
    }
    const KR: f32 = 0.2126;
    const KB: f32 = 0.0722;
    const KG: f32 = 1.0 - KR - KB;
    for px in &mut img.pixels {
        let [r, gr, bl, a] = px.to_srgba_unmultiplied();
        let (r, gr, bl) = (r as f32 / 255.0, gr as f32 / 255.0, bl as f32 / 255.0);
        let y = KR * r + KG * gr + KB * bl;
        let u = (bl - y) / (2.0 * (1.0 - KB));
        let v = (r - y) / (2.0 * (1.0 - KR));
        // mpv rotates the matrix's [U, V] columns: the signal turns the other way.
        let (u, v) = (hc * u + hs * v, -hs * u + hc * v);
        let r = y + 2.0 * (1.0 - KR) * v;
        let bl = y + 2.0 * (1.0 - KB) * u;
        let gr = (y - KR * r - KB * bl) / KG;
        let out = [r, gr, bl].map(|x| {
            let x = (contrast * x + brightness).clamp(0.0, 1.0);
            lut[(x * 255.0).round() as usize]
        });
        *px = egui::Color32::from_rgba_unmultiplied(out[0], out[1], out[2], a);
    }
}

/// Start the worker. `mpv_ready` is set once the output has loaded libmpv.
pub fn spawn(
    cache_dir: PathBuf,
    mpv_ready: Arc<AtomicBool>,
    events: winit::event_loop::EventLoopProxy<UserEvent>,
) -> std::io::Result<MediaThumbs> {
    let (tx, req_rx) = crossbeam_channel::unbounded::<Key>();
    let (done_tx, rx) = crossbeam_channel::unbounded();
    let started = Instant::now();
    std::thread::Builder::new()
        .name("media-thumbnails".into())
        .spawn(move || {
            let _ = std::fs::create_dir_all(&cache_dir);
            let mut queue: Vec<Key> = Vec::new();
            let mut last: Option<(PathBuf, egui::ColorImage)> = None;
            loop {
                if queue.is_empty() {
                    match req_rx.recv() {
                        Ok(p) => queue.push(p),
                        Err(_) => return,
                    }
                }
                queue.extend(req_rx.try_iter());
                // Newest first: what's on screen now, not what scrolled past.
                let Some(key) = queue.pop() else { continue };
                let (path, pic) = &key;
                // A slider drag asks for many versions of one file: keep its frame.
                let base = match &last {
                    Some((p, img)) if p == path => Ok(img.clone()),
                    _ => make(&cache_dir, path, || {
                        while !mpv_ready.load(Ordering::Relaxed) && started.elapsed() < MPV_WAIT {
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }),
                };
                if let Ok(img) = &base {
                    last = Some((path.clone(), img.clone()));
                }
                let image = base.map(|mut img| {
                    adjust_image(&mut img, *pic);
                    img
                });
                if let Err(e) = &image {
                    tracing::debug!(path = %path.display(), error = %e, "no media thumbnail");
                }
                if done_tx.send(Done { key, image }).is_err() {
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
        latest: HashMap::new(),
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

/// Video file extensions (thumbnails, the editor's video picker).
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "mov", "m4v", "mkv", "avi", "wmv", "webm", "mpg", "mpeg",
];

pub fn is_video(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(c: egui::Color32, pic: Picture) -> [u8; 4] {
        let mut img = egui::ColorImage::new([1, 1], vec![c]);
        adjust_image(&mut img, pic);
        img.pixels[0].to_srgba_unmultiplied()
    }

    #[test]
    fn no_settings_change_nothing() {
        let c = egui::Color32::from_rgb(200, 40, 90);
        assert_eq!(one(c, [0; 5]), [200, 40, 90, 255]);
        // The full colour round trip without settings is (near) lossless too.
        let [r, g, b, _] = one(c, [0, 0, 0, 0, 1]);
        assert!(r.abs_diff(200) < 6 && g.abs_diff(40) < 6 && b.abs_diff(90) < 6);
    }

    #[test]
    fn settings_go_the_way_mpv_does() {
        let c = egui::Color32::from_rgb(200, 40, 90);
        let grey = one(c, [0, 0, -100, 0, 0]);
        assert!(grey[0].abs_diff(grey[1]) <= 1 && grey[1].abs_diff(grey[2]) <= 1);
        assert!(one(c, [30, 0, 0, 0, 0])[1] > 40, "brightness lifts");
        assert_eq!(
            one(c, [0, -100, 0, 0, 0])[..3],
            [0, 0, 0],
            "no contrast is black"
        );
        assert!(one(c, [0, 0, 0, 50, 0])[1] > 40, "gamma up brightens");
        assert!(one(c, [0, 0, 0, -50, 0])[1] < 40, "gamma down darkens");
    }
}
