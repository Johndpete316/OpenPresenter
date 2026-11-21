//! Slide and background images for the remote page.
//!
//! Slides are drawn by the same renderer as the output, as PNGs with a transparent
//! background (the page puts them over the background media's poster frame, so
//! Clear Text / Clear Background are just layers shown or hidden). Encoded images
//! are cached in memory.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use render::{Fonts, render_slide};

/// Makes a poster frame of a background image or video, `width` pixels wide.
pub type PosterFn = Arc<dyn Fn(&Path, u32) -> Result<image::RgbaImage, String> + Send + Sync>;

/// What the server needs to draw images. Fonts usually arrive a moment after start
/// (the system font scan); until then image requests answer 503 and the page retries.
#[derive(Clone, Default)]
pub struct Images {
    pub fonts: Arc<OnceLock<Mutex<Fonts>>>,
    pub poster: Option<PosterFn>,
}

impl std::fmt::Debug for Images {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Images")
            .field("fonts_ready", &self.fonts.get().is_some())
            .field("poster", &self.poster.is_some())
            .finish()
    }
}

/// Requested widths are rounded to a few sizes so the cache stays small.
pub(crate) fn width(requested: Option<u32>) -> u32 {
    requested.unwrap_or(640).clamp(160, 1920).div_ceil(160) * 160
}

#[derive(Default)]
pub(crate) struct Cache {
    map: HashMap<(String, u32), Arc<Vec<u8>>>,
    bytes: usize,
}

const CACHE_BYTES: usize = 64 << 20;

impl Cache {
    pub fn get(&self, key: &str, w: u32) -> Option<Arc<Vec<u8>>> {
        self.map.get(&(key.to_owned(), w)).cloned()
    }

    pub fn put(&mut self, key: String, w: u32, data: Arc<Vec<u8>>) {
        if self.bytes + data.len() > CACHE_BYTES {
            self.map.clear();
            self.bytes = 0;
        }
        self.bytes += data.len();
        self.map.insert((key, w), data);
    }
}

/// Slide `flat` of `doc`, `w` pixels wide at the canvas aspect, as a PNG: slide
/// background colour (if any) and text over transparency.
pub(crate) fn slide_png(
    fonts: &mut Fonts,
    doc: &pro6::Document,
    flat: usize,
    w: u32,
) -> Result<Vec<u8>, String> {
    let slide = doc.slides().nth(flat).ok_or("no such slide")?;
    let c = doc.canvas;
    let h = if c.width == 0 {
        w * 9 / 16
    } else {
        (u64::from(w) * u64::from(c.height) / u64::from(c.width)).clamp(1, 16384) as u32
    };
    let frame = render_slide(fonts, doc, slide, w, h).map_err(|e| e.user_message())?;
    let mut out = vec![0u8; frame.text.len()];
    let bg = frame.background.map(|[r, g, b, a]| {
        let m = |v: u8| ((u16::from(v) * u16::from(a) + 127) / 255) as u8;
        [m(r), m(g), m(b), a]
    });
    let canvas = frame.placement.canvas;
    for (i, (dst, src)) in out
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(frame.text.as_chunks::<4>().0)
        .enumerate()
    {
        let (x, y) = ((i as u32) % w, (i as u32) / w);
        let inside = x >= canvas.x
            && x < canvas.x + canvas.width
            && y >= canvas.y
            && y < canvas.y + canvas.height;
        // Premultiplied "text over background", then back to straight alpha.
        let under = bg.filter(|_| inside).unwrap_or([0; 4]);
        let inv = u16::from(255 - src[3]);
        let mut p = [0u8; 4];
        for k in 0..4 {
            p[k] = src[k].saturating_add(((u16::from(under[k]) * inv + 127) / 255) as u8);
        }
        let a = u16::from(p[3]);
        if a > 0 && a < 255 {
            for v in p.iter_mut().take(3) {
                *v = ((u16::from(*v) * 255 + a / 2) / a).min(255) as u8;
            }
        }
        *dst = p;
    }
    let mut png = Vec::new();
    let enc = image::codecs::png::PngEncoder::new_with_quality(
        &mut png,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    );
    image::ImageEncoder::write_image(enc, &out, w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(png)
}

/// A poster frame as JPEG.
pub(crate) fn poster_jpeg(poster: &PosterFn, path: &Path, w: u32) -> Result<Vec<u8>, String> {
    let img = poster(path, w)?;
    let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
    let mut jpg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 80)
        .encode_image(&rgb)
        .map_err(|e| e.to_string())?;
    Ok(jpg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_round_to_steps() {
        assert_eq!(width(None), 640);
        assert_eq!(width(Some(1)), 160);
        assert_eq!(width(Some(700)), 800);
        assert_eq!(width(Some(99999)), 1920);
    }
}
