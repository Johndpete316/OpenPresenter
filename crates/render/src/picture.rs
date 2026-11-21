//! Images on slides: decoding (cached for every render thread), resizing, and the
//! non-destructive adjustments.
//!
//! Decoding happens on whichever render thread first needs a file (the output,
//! editor canvas and thumbnail workers, never the UI thread). The decoded picture
//! is kept downscaled to at most [`MAX_DECODED`] on its long side, and resized
//! copies are kept per drawn size, so moving a slider or switching slides only
//! redoes the cheap per-pixel adjustment.
//!
//! Adjustments are computed on the CPU here, in the same code path for the output,
//! the editor, thumbnails, the phone remote and `pp6tool render` (WYSIWYG; see
//! docs/EDITOR-PLAN.md E3 status).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use doc::{Adjust, MediaRef};
use image::RgbaImage;
use sha2::{Digest, Sha256};

/// Long side of the decoded picture kept in memory.
pub const MAX_DECODED: u32 = 4096;
/// Memory kept for decoded and resized pictures (least recently used go first).
const BUDGET_BYTES: usize = 512 << 20;

#[derive(Clone, PartialEq, Eq, Hash)]
struct FileKey {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct SizedKey {
    file: FileKey,
    width: u32,
    height: u32,
    flip_h: bool,
    flip_v: bool,
}

struct Decoded {
    image: Arc<RgbaImage>,
    sha256: String,
}

#[derive(Default)]
struct Cache {
    decoded: HashMap<FileKey, (Arc<Decoded>, u64)>,
    sized: HashMap<SizedKey, (Arc<RgbaImage>, u64)>,
    bytes: usize,
    tick: u64,
}

impl Cache {
    fn trim(&mut self) {
        while self.bytes > BUDGET_BYTES {
            let oldest_sized = self
                .sized
                .iter()
                .min_by_key(|(_, v)| v.1)
                .map(|(k, v)| (k.clone(), v.1));
            let oldest_dec = self
                .decoded
                .iter()
                .min_by_key(|(_, v)| v.1)
                .map(|(k, v)| (k.clone(), v.1));
            match (oldest_sized, oldest_dec) {
                (Some((k, a)), Some((_, b))) if a <= b => self.drop_sized(&k),
                (_, Some((k, _))) => {
                    if let Some((d, _)) = self.decoded.remove(&k) {
                        self.bytes = self.bytes.saturating_sub(d.image.as_raw().len());
                    }
                }
                (Some((k, _)), None) => self.drop_sized(&k),
                (None, None) => break,
            }
        }
    }

    fn drop_sized(&mut self, k: &SizedKey) {
        if let Some((i, _)) = self.sized.remove(k) {
            self.bytes = self.bytes.saturating_sub(i.as_raw().len());
        }
    }
}

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(Mutex::default)
}

fn lock() -> std::sync::MutexGuard<'static, Cache> {
    cache().lock().unwrap_or_else(|e| e.into_inner())
}

/// Why an image can't be drawn as stored.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ImageProblem {
    Missing(PathBuf),
    Unreadable(String),
}

impl ImageProblem {
    pub(crate) fn message(&self, m: &MediaRef) -> String {
        match self {
            ImageProblem::Missing(p) => format!(
                "image \"{}\" wasn't found ({}); it isn't shown",
                m.file_name(),
                p.display()
            ),
            ImageProblem::Unreadable(e) => {
                format!(
                    "image \"{}\" can't be read ({e}); it isn't shown",
                    m.file_name()
                )
            }
        }
    }
}

fn file_key(path: &Path) -> Result<FileKey, ImageProblem> {
    let meta = std::fs::metadata(path).map_err(|_| ImageProblem::Missing(path.to_path_buf()))?;
    Ok(FileKey {
        path: path.to_path_buf(),
        modified: meta.modified().ok(),
        len: meta.len(),
    })
}

fn decode(key: &FileKey) -> Result<Arc<Decoded>, ImageProblem> {
    {
        let mut c = lock();
        c.tick += 1;
        let t = c.tick;
        if let Some(e) = c.decoded.get_mut(key) {
            e.1 = t;
            return Ok(e.0.clone());
        }
    }
    let _span = tracing::debug_span!("decode_image", path = %key.path.display()).entered();
    let bytes = std::fs::read(&key.path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ImageProblem::Missing(key.path.clone()),
        _ => ImageProblem::Unreadable(e.to_string()),
    })?;
    let sha256 = hex(&Sha256::digest(&bytes));
    let img = image::load_from_memory(&bytes)
        .map_err(|e| ImageProblem::Unreadable(e.to_string()))?
        .to_rgba8();
    let img = if img.width().max(img.height()) > MAX_DECODED {
        let s = f64::from(MAX_DECODED) / f64::from(img.width().max(img.height()));
        let (w, h) = (
            ((f64::from(img.width()) * s).round() as u32).max(1),
            ((f64::from(img.height()) * s).round() as u32).max(1),
        );
        image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let d = Arc::new(Decoded {
        image: Arc::new(img),
        sha256,
    });
    let mut c = lock();
    c.tick += 1;
    let t = c.tick;
    c.bytes += d.image.as_raw().len();
    c.decoded.insert(key.clone(), (d.clone(), t));
    c.trim();
    Ok(d)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// A picture resized to `width × height` (straight RGBA), with notes for the
/// operator (e.g. the file changed since it was added).
pub(crate) struct Sized {
    pub image: Arc<RgbaImage>,
    pub notes: Vec<String>,
}

pub(crate) fn sized(
    doc_path: &Path,
    media: &MediaRef,
    width: u32,
    height: u32,
    flip_h: bool,
    flip_v: bool,
) -> Result<Sized, ImageProblem> {
    let path = media.resolve(doc_path);
    let file = file_key(&path)?;
    let decoded = decode(&file)?;
    let mut notes = Vec::new();
    if let Some(expected) = &media.sha256
        && !expected.eq_ignore_ascii_case(&decoded.sha256)
    {
        notes.push(format!(
            "image \"{}\" has changed since it was added to this presentation",
            media.file_name()
        ));
    }
    let key = SizedKey {
        file,
        width,
        height,
        flip_h,
        flip_v,
    };
    {
        let mut c = lock();
        c.tick += 1;
        let t = c.tick;
        if let Some(e) = c.sized.get_mut(&key) {
            e.1 = t;
            return Ok(Sized {
                image: e.0.clone(),
                notes,
            });
        }
    }
    let src = &decoded.image;
    let mut img = if src.width() == width && src.height() == height {
        (**src).clone()
    } else {
        image::imageops::resize(&**src, width, height, image::imageops::FilterType::Triangle)
    };
    if flip_h {
        image::imageops::flip_horizontal_in_place(&mut img);
    }
    if flip_v {
        image::imageops::flip_vertical_in_place(&mut img);
    }
    let img = Arc::new(img);
    let mut c = lock();
    c.tick += 1;
    let t = c.tick;
    c.bytes += img.as_raw().len();
    c.sized.insert(key, (img.clone(), t));
    c.trim();
    Ok(Sized { image: img, notes })
}

/// Natural pixel size of an image file (for inserting it at its own shape).
pub fn image_size(path: &Path) -> Result<(u32, u32), String> {
    image::image_dimensions(path).map_err(|e| e.to_string())
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Straight RGBA `src` → premultiplied RGBA with `adjust` and `opacity` applied.
/// The vignette is computed over the `src` rectangle.
///
/// Brightness and contrast go through a 256-entry table, the vignette through a
/// table over the squared distance, and rows are split across threads: a full-HD
/// picture takes a few milliseconds, so slider drags stay smooth.
pub fn adjusted_premultiplied(src: &RgbaImage, adjust: &Adjust, opacity: f64) -> Vec<u8> {
    let (w, h) = (src.width() as usize, src.height() as usize);
    let mut out = vec![0u8; w * h * 4];
    if w == 0 || h == 0 {
        return out;
    }
    let op = opacity.clamp(0.0, 1.0) as f32;
    let b = adjust.brightness as f32;
    let k = 1.0 + adjust.contrast as f32;
    let mut tone = [0f32; 256];
    for (i, t) in tone.iter_mut().enumerate() {
        let mut c = i as f32 / 255.0;
        c = if b >= 0.0 {
            c + (1.0 - c) * b * 0.8
        } else {
            c * (1.0 + b * 0.8)
        };
        *t = (c - 0.5) * k + 0.5;
    }
    let p = Params {
        tone,
        mix: adjust.saturation != 0.0 || adjust.temperature != 0.0 || adjust.tint != 0.0,
        sat: 1.0 + adjust.saturation as f32,
        temp: adjust.temperature as f32 * 0.15,
        tint: adjust.tint as f32 * 0.12,
        vignette: (adjust.vignette > 0.0).then(|| vignette_table(adjust)),
        op,
    };
    // Squared normalised distance per column; rows add theirs.
    let u2: Vec<f32> = (0..w)
        .map(|x| {
            let u = (x as f32 + 0.5) / w as f32 * 2.0 - 1.0;
            u * u
        })
        .collect();
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(8)
        .min(h.div_ceil(64).max(1));
    let rows_per = h.div_ceil(threads);
    let raw = src.as_raw();
    std::thread::scope(|scope| {
        for (chunk_i, dst) in out.chunks_mut(rows_per * w * 4).enumerate() {
            let (p, u2) = (&p, &u2);
            let y0 = chunk_i * rows_per;
            let src = &raw[y0 * w * 4..(y0 * w * 4 + dst.len())];
            let mut work = move || adjust_rows(src, dst, w, h, y0, p, u2);
            if threads == 1 {
                work();
            } else {
                scope.spawn(work);
            }
        }
    });
    out
}

struct Params {
    tone: [f32; 256],
    /// Saturation, temperature or tint is set.
    mix: bool,
    sat: f32,
    temp: f32,
    tint: f32,
    /// Darkening factor by squared distance (0 = centre … 2 = corner).
    vignette: Option<Vec<f32>>,
    op: f32,
}

const VIGNETTE_STEPS: usize = 1024;

fn vignette_table(adjust: &Adjust) -> Vec<f32> {
    let amount = adjust.vignette as f32;
    let start = 1.0 - adjust.vignette_size as f32;
    let end = start + 0.6;
    (0..VIGNETTE_STEPS)
        .map(|i| {
            let d2 = i as f32 / (VIGNETTE_STEPS - 1) as f32 * 2.0;
            let d = d2.sqrt() / std::f32::consts::SQRT_2;
            1.0 - amount * smoothstep(start, end, d)
        })
        .collect()
}

fn adjust_rows(src: &[u8], dst: &mut [u8], w: usize, h: usize, y0: usize, p: &Params, u2: &[f32]) {
    for (row, (row_in, row_out)) in src
        .chunks_exact(w * 4)
        .zip(dst.chunks_exact_mut(w * 4))
        .enumerate()
    {
        let y = y0 + row;
        let v = (y as f32 + 0.5) / h as f32 * 2.0 - 1.0;
        let v2 = v * v;
        for ((px, o), u2) in row_in
            .as_chunks::<4>()
            .0
            .iter()
            .zip(row_out.as_chunks_mut::<4>().0)
            .zip(u2)
        {
            let a = f32::from(px[3]) / 255.0 * p.op;
            if a <= 0.0 {
                continue;
            }
            let mut c = [
                p.tone[px[0] as usize],
                p.tone[px[1] as usize],
                p.tone[px[2] as usize],
            ];
            if p.mix {
                let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                for ch in &mut c {
                    *ch = l + (*ch - l) * p.sat;
                }
                c[0] += p.temp + p.tint * 0.5;
                c[1] -= p.tint;
                c[2] += -p.temp + p.tint * 0.5;
            }
            if let Some(t) = &p.vignette {
                let i = ((u2 + v2) * 0.5 * (VIGNETTE_STEPS - 1) as f32) as usize;
                let f = t[i.min(VIGNETTE_STEPS - 1)];
                for ch in &mut c {
                    *ch *= f;
                }
            }
            let s = a * 255.0;
            let q = |v: f32| (v.clamp(0.0, 1.0) * s + 0.5) as u8;
            *o = [q(c[0]), q(c[1]), q(c[2]), (s + 0.5) as u8];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(v: u8) -> RgbaImage {
        RgbaImage::from_pixel(10, 10, image::Rgba([v, v, v, 255]))
    }

    #[test]
    fn no_adjustment_is_the_picture_premultiplied() {
        let img = RgbaImage::from_pixel(2, 2, image::Rgba([200, 100, 50, 128]));
        let out = adjusted_premultiplied(&img, &Adjust::default(), 1.0);
        assert_eq!(&out[..4], &[100, 50, 25, 128]);
    }

    #[test]
    fn brightness_contrast_saturation_move_the_right_way() {
        let base = adjusted_premultiplied(&grey(100), &Adjust::default(), 1.0)[0];
        let bright = Adjust {
            brightness: 0.5,
            ..Adjust::default()
        };
        assert!(adjusted_premultiplied(&grey(100), &bright, 1.0)[0] > base);
        let dark = Adjust {
            brightness: -0.5,
            ..Adjust::default()
        };
        assert!(adjusted_premultiplied(&grey(100), &dark, 1.0)[0] < base);
        let contrast = Adjust {
            contrast: 0.5,
            ..Adjust::default()
        };
        assert!(adjusted_premultiplied(&grey(60), &contrast, 1.0)[0] < 60);
        let red = RgbaImage::from_pixel(1, 1, image::Rgba([200, 50, 50, 255]));
        let grey_out = adjusted_premultiplied(
            &red,
            &Adjust {
                saturation: -1.0,
                ..Adjust::default()
            },
            1.0,
        );
        assert_eq!(grey_out[0], grey_out[1], "fully desaturated is grey");
        let warm = adjusted_premultiplied(
            &grey(128),
            &Adjust {
                temperature: 1.0,
                ..Adjust::default()
            },
            1.0,
        );
        assert!(warm[0] > warm[2], "warm: more red than blue");
    }

    #[test]
    fn vignette_darkens_corners_not_the_centre() {
        let img = RgbaImage::from_pixel(41, 41, image::Rgba([200, 200, 200, 255]));
        let a = Adjust {
            vignette: 1.0,
            vignette_size: 0.5,
            ..Adjust::default()
        };
        let out = adjusted_premultiplied(&img, &a, 1.0);
        let at = |x: usize, y: usize| out[(y * 41 + x) * 4];
        assert_eq!(at(20, 20), 200);
        assert!(at(0, 0) < 60, "corner {}", at(0, 0));
    }

    #[test]
    fn opacity_scales_alpha() {
        let out = adjusted_premultiplied(&grey(200), &Adjust::default(), 0.5);
        assert_eq!(out[3], 128);
        assert_eq!(out[0], 100);
    }
}
