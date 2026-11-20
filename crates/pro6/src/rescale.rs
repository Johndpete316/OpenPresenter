//! Change a presentation's canvas resolution, scaling everything drawn on it.
//!
//! Same aspect ratio: proportional scale. Different aspect: uniform fit, centred
//! (nothing distorted), unless `Mode::Stretch` is asked for. Scales the canvas,
//! element positions/sizes, RTF sizes (`\fs` and other absolute size words, edited
//! in place), shadow blur and offset, and stroke width.

use crate::edit::WinPayloads;
use crate::model::{CanvasSize, DisplayElement, Document, TextElement};
use crate::rtf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Uniform scale to fit, centred (default).
    Fit,
    /// Scale x and y independently to fill the new canvas. Text and effects use
    /// the smaller factor so they never overflow their boxes.
    Stretch,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RescaleReport {
    pub from: Option<CanvasSize>,
    pub to: Option<CanvasSize>,
    pub scale_x: f64,
    pub scale_y: f64,
    /// Factor applied to font sizes and effects.
    pub scale_text: f64,
    pub offset_x: f64,
    pub offset_y: f64,
    pub text_elements: usize,
    /// `(old, new)` half-point sizes seen for `\fs`, deduplicated, sorted.
    pub font_sizes: Vec<(i32, i32)>,
    /// Other RTF size words scaled (`sl`, `li`, ...), with counts.
    pub other_rtf_sizes: Vec<(String, usize)>,
    pub shadows: usize,
    pub strokes: usize,
    pub win_payloads_dropped: usize,
    /// Things that couldn't be scaled (reported, not guessed).
    pub warnings: Vec<String>,
    /// Canvas was already the target size: nothing changed.
    pub unchanged: bool,
}

/// Rescale `doc` in place to `to`.
pub fn rescale(doc: &mut Document, to: CanvasSize, mode: Mode, win: WinPayloads) -> RescaleReport {
    let from = doc.canvas;
    let mut r = RescaleReport {
        from: Some(from),
        to: Some(to),
        ..Default::default()
    };
    if from == to {
        r.unchanged = true;
        r.scale_x = 1.0;
        r.scale_y = 1.0;
        r.scale_text = 1.0;
        return r;
    }
    let sx = f64::from(to.width) / f64::from(from.width);
    let sy = f64::from(to.height) / f64::from(from.height);
    let (scale_x, scale_y, ox, oy) = match mode {
        Mode::Fit => {
            let s = sx.min(sy);
            (
                s,
                s,
                (f64::from(to.width) - f64::from(from.width) * s) / 2.0,
                (f64::from(to.height) - f64::from(from.height) * s) / 2.0,
            )
        }
        Mode::Stretch => (sx, sy, 0.0, 0.0),
    };
    let st = scale_x.min(scale_y);
    r.scale_x = scale_x;
    r.scale_y = scale_y;
    r.scale_text = st;
    r.offset_x = ox;
    r.offset_y = oy;
    doc.canvas = to;

    let path = doc.path.clone();
    let mut fs = std::collections::BTreeSet::new();
    let mut other: std::collections::BTreeMap<String, usize> = Default::default();
    let groups = doc
        .groups
        .as_mut()
        .map(|g| g.items.iter_mut())
        .into_iter()
        .flatten();
    for (gi, g) in groups.enumerate() {
        let slides = g
            .slides
            .as_mut()
            .map(|s| s.items.iter_mut())
            .into_iter()
            .flatten();
        for (si, slide) in slides.enumerate() {
            let elements = slide
                .elements
                .as_mut()
                .map(|e| e.items.iter_mut())
                .into_iter()
                .flatten();
            for el in elements {
                match el {
                    DisplayElement::Text(t) => {
                        r.text_elements += 1;
                        scale_text_element(
                            t,
                            (scale_x, scale_y, st, ox, oy),
                            win,
                            &mut r,
                            &mut fs,
                            &mut other,
                        );
                    }
                    DisplayElement::Other(raw) => r.warnings.push(format!(
                        "group {} slide {}: <{}> element isn't scaled (not modelled yet)",
                        gi + 1,
                        si + 1,
                        raw.name
                    )),
                }
            }
        }
    }
    r.font_sizes = fs.into_iter().collect();
    r.other_rtf_sizes = other.into_iter().collect();
    tracing::info!(file = %path.display(), ?from, ?to, scale_x, scale_y, "rescaled");
    r
}

fn scale_text_element(
    t: &mut TextElement,
    (sx, sy, st, ox, oy): (f64, f64, f64, f64, f64),
    win: WinPayloads,
    r: &mut RescaleReport,
    fs: &mut std::collections::BTreeSet<(i32, i32)>,
    other: &mut std::collections::BTreeMap<String, usize>,
) {
    let p = &mut t.position.value;
    p.x = p.x * sx + ox;
    p.y = p.y * sy + oy;
    p.width *= sx;
    p.height *= sy;

    if let Some(s) = &mut t.shadow {
        match (&mut s.value.blur, &mut s.value.offset) {
            (Some(b), Some((x, y))) => {
                *b *= st;
                *x *= st;
                *y *= st;
                r.shadows += 1;
            }
            _ => r.warnings.push(format!(
                "element {}: shadow {:?} isn't in the expected layout; not scaled",
                t.uuid,
                s.value.fields.join("|")
            )),
        }
    }
    if let Some(w) = t.stroke.as_mut().and_then(|s| s.width.as_mut()) {
        w.value *= st;
        r.strokes += 1;
    }
    if let Some(rtf) = &mut t.rtf {
        match rtf::scale_sizes(&rtf.value.bytes, st) {
            Ok((bytes, changes)) => {
                for c in changes {
                    if c.word == "fs" {
                        fs.insert((c.old, c.new));
                    } else {
                        *other.entry(c.word).or_default() += 1;
                    }
                }
                rtf.value.bytes = bytes;
            }
            Err(e) => r.warnings.push(format!(
                "element {}: RTF text is malformed ({e}); font sizes not scaled",
                t.uuid
            )),
        }
    }
    if win == WinPayloads::Drop && (t.win_flow.is_some() || t.win_font.is_some()) {
        t.win_flow = None;
        t.win_font = None;
        r.win_payloads_dropped += 1;
    }
}
