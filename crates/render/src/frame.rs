//! Canvas → output placement and the rendered frame.

use std::time::Duration;

use pro6::CanvasSize;

use crate::error::RenderWarning;

/// Pixel rectangle in output space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Where the canvas lands on the output: uniform scale, centred, black bars on
/// the sides that don't fit. Never stretches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Output pixels per canvas pixel (same on both axes).
    pub scale: f32,
    pub canvas: PixelRect,
}

impl Placement {
    pub fn fit(canvas: CanvasSize, output_width: u32, output_height: u32) -> Placement {
        let sx = output_width as f32 / canvas.width as f32;
        let sy = output_height as f32 / canvas.height as f32;
        let scale = sx.min(sy);
        let width = ((canvas.width as f32 * scale).round() as u32).min(output_width);
        let height = ((canvas.height as f32 * scale).round() as u32).min(output_height);
        Placement {
            scale,
            canvas: PixelRect {
                x: (output_width - width) / 2,
                y: (output_height - height) / 2,
                width,
                height,
            },
        }
    }

    /// Canvas coordinate → output pixel coordinate.
    pub fn map(&self, x: f64, y: f64) -> (f32, f32) {
        (
            self.canvas.x as f32 + x as f32 * self.scale,
            self.canvas.y as f32 + y as f32 * self.scale,
        )
    }
}

/// A rendered slide at output resolution, as separate layers so text and
/// background can be cleared independently.
#[derive(Debug, Clone)]
pub struct SlideFrame {
    pub width: u32,
    pub height: u32,
    pub placement: Placement,
    /// Slide background colour (straight RGBA) when the slide or document draws one,
    /// inside the canvas rect only. `None` = nothing (black, or background media).
    pub background: Option<[u8; 4]>,
    /// Text layer, `width × height`, premultiplied RGBA, transparent where empty.
    pub text: Vec<u8>,
    pub warnings: Vec<RenderWarning>,
    pub render_time: Duration,
}

impl SlideFrame {
    /// Flatten to an opaque image: black bars, background, then text.
    pub fn composite(&self) -> image::RgbaImage {
        self.composite_layers(true, true)
    }

    /// Flatten with layers switched on/off (mirrors Clear Text / Clear Background).
    pub fn composite_layers(&self, background: bool, text: bool) -> image::RgbaImage {
        let mut img =
            image::RgbaImage::from_pixel(self.width, self.height, image::Rgba([0, 0, 0, 255]));
        if background && let Some([r, g, b, a]) = self.background {
            let bg = [mul(r, a), mul(g, a), mul(b, a), 255];
            let c = self.placement.canvas;
            for y in c.y..c.y + c.height {
                for x in c.x..c.x + c.width {
                    img.put_pixel(x, y, image::Rgba(bg));
                }
            }
        }
        if text {
            for (px, src) in img.pixels_mut().zip(self.text.as_chunks::<4>().0) {
                let inv = 255 - src[3];
                for i in 0..3 {
                    px.0[i] = src[i].saturating_add(mul(px.0[i], inv));
                }
            }
        }
        img
    }
}

/// `a * b / 255`, rounded.
pub(crate) fn mul(a: u8, b: u8) -> u8 {
    ((u16::from(a) * u16::from(b) + 127) / 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_letterboxes_and_pillarboxes() {
        let c4x3 = CanvasSize {
            width: 1024,
            height: 768,
        };
        // 4:3 on 16:9 → pillarbox.
        let p = Placement::fit(c4x3, 1920, 1080);
        assert_eq!(
            p.canvas,
            PixelRect {
                x: 240,
                y: 0,
                width: 1440,
                height: 1080
            }
        );
        assert!((p.scale - 1.40625).abs() < 1e-6);
        // 16:9 on 4:3 → letterbox.
        let p = Placement::fit(
            CanvasSize {
                width: 1920,
                height: 1080,
            },
            1024,
            768,
        );
        assert_eq!(
            p.canvas,
            PixelRect {
                x: 0,
                y: 96,
                width: 1024,
                height: 576
            }
        );
        // Same aspect → exact.
        let p = Placement::fit(c4x3, 1024, 768);
        assert_eq!(
            (p.scale, p.canvas),
            (
                1.0,
                PixelRect {
                    x: 0,
                    y: 0,
                    width: 1024,
                    height: 768
                }
            )
        );
        assert_eq!(p.map(10.0, 20.0), (10.0, 20.0));
    }
}
