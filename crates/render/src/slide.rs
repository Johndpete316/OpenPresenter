//! Lay out and rasterise one slide.
//!
//! Assumptions (docs/PLAN.md Unknowns): one RTF point = one canvas pixel (the
//! native run size is in canvas pixels); RTF "auto" colour (no `\cf`) = black. `verticalAlignment`: 0 = middle is **verified** (every real
//! file uses 0 and PP6 shows it centred); 1 = top, 2 = bottom are assumed.
//! Layout happens directly at output scale (font sizes and boxes multiplied by
//! the fit scale), so text stays sharp on any output resolution.

use std::time::Instant;

use cosmic_text::{Align as CtAlign, Attrs, Buffer, Color, Family, Metrics, Shaping, Wrap};
use doc::{Align, Document, Element, Rgba, Slide, TextBox, TextProblem, VAlign};

use crate::error::{RenderError, RenderWarning};
use crate::fonts::{Fonts, ResolvedFont};
use crate::frame::{Placement, SlideFrame, mul};

/// Render `slide` of `doc` for an output of `width × height` pixels.
pub fn render_slide(
    fonts: &mut Fonts,
    doc: &Document,
    slide: &Slide,
    width: u32,
    height: u32,
) -> Result<SlideFrame, RenderError> {
    let _span = tracing::debug_span!("render_slide", slide = %slide.uuid, width, height).entered();
    let started = Instant::now();
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(RenderError::BadTarget { width, height }.logged());
    }
    let placement = Placement::fit(doc.canvas, width, height);
    let mut frame = SlideFrame {
        width,
        height,
        placement,
        background: slide.background.or(doc.background).map(to_rgba8),
        text: vec![0; width as usize * height as usize * 4],
        warnings: Vec::new(),
        render_time: Default::default(),
    };
    for el in &slide.elements {
        match el {
            Element::Text(t) => render_text(fonts, doc, slide, t, &mut frame)?,
            Element::Pro6(raw) => frame.warnings.push(RenderWarning {
                element_uuid: raw.uuid.clone(),
                message: format!("<{}> elements aren't drawn yet", raw.kind),
            }),
        }
    }
    frame.render_time = started.elapsed();
    tracing::debug!(
        elapsed_us = frame.render_time.as_micros() as u64,
        warnings = frame.warnings.len(),
        "rendered"
    );
    Ok(frame)
}

fn to_rgba8(c: Rgba) -> [u8; 4] {
    c.to_rgba8()
}

fn render_text(
    fonts: &mut Fonts,
    doc: &Document,
    slide: &Slide,
    el: &TextBox,
    frame: &mut SlideFrame,
) -> Result<(), RenderError> {
    let warn = |frame: &mut SlideFrame, message: String| {
        frame.warnings.push(RenderWarning {
            element_uuid: el.uuid.clone(),
            message,
        });
    };
    if el.shadow.is_some() {
        warn(frame, "text shadow isn't drawn yet".into());
    }
    if el.outline.is_some() {
        warn(frame, "text outline (stroke) isn't drawn yet".into());
    }

    let p = frame.placement;
    let r = el.frame;
    let (x0, y0) = p.map(r.x, r.y);
    let box_w = (r.width as f32 * p.scale).max(1.0);
    let box_h = r.height as f32 * p.scale;

    let fill = el.fill.map(|f| {
        let c = to_rgba8(f);
        [
            c[0],
            c[1],
            c[2],
            (f64::from(c[3]) * el.opacity).round() as u8,
        ]
    });
    if fill.is_some() && el.corner_radius > 0.0 {
        warn(
            frame,
            "rounded corners on the element fill aren't drawn yet".into(),
        );
    }

    let fail = |e: RenderError| Err(e.logged());
    let layout = match &el.problem {
        None => layout_text(fonts, doc, slide, el, frame, box_w, box_h)?,
        Some(TextProblem::NoStyledText) => {
            warn(frame, "element has no RTFData; its text isn't drawn".into());
            None
        }
        Some(TextProblem::Unreadable { message }) => {
            return fail(RenderError::BadText {
                message: message.clone(),
                file: doc.path.clone(),
                slide_uuid: slide.uuid.clone(),
                element_uuid: el.uuid.clone(),
            });
        }
        Some(TextProblem::FontNotInTable { index }) => {
            return fail(RenderError::FontNotInTable {
                index: *index,
                file: doc.path.clone(),
                slide_uuid: slide.uuid.clone(),
                element_uuid: el.uuid.clone(),
            });
        }
    };
    let text_h = layout.as_ref().map_or(0.0, |(_, h)| *h);
    let dy = match el.valign {
        VAlign::Top => 0.0,
        VAlign::Bottom => box_h - text_h,
        VAlign::Middle => (box_h - text_h) / 2.0,
    };

    // 360° (seen in real files) is no rotation. Degrees clockwise (assumed; unverified
    // against PP6), about the box centre.
    let rotation = el.rotation.rem_euclid(360.0);
    if rotation == 0.0 {
        let c = p.canvas;
        let mut layer = Layer {
            width: frame.width as usize,
            clip: (
                c.x as i32,
                c.y as i32,
                (c.x + c.width) as i32,
                (c.y + c.height) as i32,
            ),
            buf: &mut frame.text,
        };
        if let Some(color) = fill {
            layer.fill(x0, y0, box_w, box_h, color);
        }
        if let Some((mut buffer, _)) = layout {
            layer.draw(
                fonts,
                &mut buffer,
                x0.round() as i32,
                (y0 + dy).round() as i32,
            );
        }
        return Ok(());
    }

    // Rotated: draw the box upright into its own layer, then rotate it onto the frame.
    const MARGIN: f32 = 4.0;
    let top = dy.min(0.0).floor() - MARGIN;
    let bottom = box_h.max(dy + text_h).ceil() + MARGIN;
    let left = -MARGIN;
    let right = box_w.ceil() + MARGIN;
    let (tw, th) = ((right - left) as usize, (bottom - top) as usize);
    let mut tmp = vec![0u8; tw * th * 4];
    {
        let mut layer = Layer {
            width: tw,
            clip: (0, 0, tw as i32, th as i32),
            buf: &mut tmp,
        };
        if let Some(color) = fill {
            layer.fill(-left, -top, box_w, box_h, color);
        }
        if let Some((mut buffer, _)) = layout {
            layer.draw(
                fonts,
                &mut buffer,
                (-left).round() as i32,
                (dy - top).round() as i32,
            );
        }
    }
    composite_rotated(
        frame,
        &tmp,
        (tw, th),
        (box_w / 2.0 - left, box_h / 2.0 - top),
        (x0 + box_w / 2.0, y0 + box_h / 2.0),
        rotation as f32,
    );
    Ok(())
}

/// Lay out a text box's runs at output scale. `None` when there's no text to draw.
fn layout_text(
    fonts: &mut Fonts,
    doc: &Document,
    slide: &Slide,
    el: &TextBox,
    frame: &mut SlideFrame,
    box_w: f32,
    box_h: f32,
) -> Result<Option<(Buffer, f32)>, RenderError> {
    let warn = |frame: &mut SlideFrame, message: String| {
        frame.warnings.push(RenderWarning {
            element_uuid: el.uuid.clone(),
            message,
        });
    };
    // Empty text (including PP6's hidden "Double-click to edit" placeholder) draws nothing.
    if el.runs().all(|r| r.text.is_empty()) {
        return Ok(None);
    }
    let p = frame.placement;

    // Resolve every run's font first: a missing font is an error, never a substitute.
    let missing = |font: String| RenderError::MissingFont {
        font,
        file: doc.path.clone(),
        slide_uuid: slide.uuid.clone(),
        element_uuid: el.uuid.clone(),
    };
    let mut styles: Vec<(ResolvedFont, Metrics, Color)> = Vec::new();
    let mut spans: Vec<(String, usize)> = Vec::new();
    let mut aligns: Vec<Option<CtAlign>> = Vec::new();
    let mut underline = false;
    for (pi, para) in el.paragraphs.iter().enumerate() {
        if pi > 0 {
            // Paragraph break carries the previous style (sets the empty line's height).
            if let Some(&(_, s)) = spans.last() {
                spans.push(("\n".into(), s));
            }
        }
        let align = match para.align {
            Align::Left => CtAlign::Left,
            Align::Center => CtAlign::Center,
            Align::Right => CtAlign::Right,
            Align::Justified => CtAlign::Justified,
        };
        let soft_breaks: usize = para.runs.iter().map(|r| r.text.matches('\n').count()).sum();
        aligns.extend(std::iter::repeat_n(Some(align), soft_breaks + 1));
        for run in &para.runs {
            let name = &run.font;
            let font = fonts
                .resolve(name, run.bold, run.italic)
                .ok_or_else(|| missing(name.clone()).logged())?;
            if font.inexact {
                warn(
                    frame,
                    format!(
                        "'{}' has no {} face; the nearest face of the same font is used",
                        font.family,
                        style_name(run.bold, run.italic)
                    ),
                );
            }
            underline |= run.underline;
            let px = run.size as f32 * p.scale;
            let metrics = Metrics::new(px, fonts.line_height(&font, px));
            let [cr, cg, cb] = run.color.unwrap_or([0, 0, 0]);
            let color = Color::rgba(cr, cg, cb, (255.0 * el.opacity).round() as u8);
            let idx = match styles
                .iter()
                .position(|s| s.0 == font && s.1 == metrics && s.2 == color)
            {
                Some(i) => i,
                None => {
                    styles.push((font, metrics, color));
                    styles.len() - 1
                }
            };
            spans.push((run.text.clone(), idx));
        }
    }
    trim_line_ends(&mut spans);
    if underline {
        warn(frame, "underline isn't drawn yet".into());
    }
    let Some(first) = styles.first() else {
        return Ok(None); // no text
    };

    let attrs: Vec<Attrs<'_>> = styles
        .iter()
        .map(|(f, m, c)| {
            Attrs::new()
                .family(Family::Name(&f.family))
                .weight(f.weight)
                .style(f.style)
                .metrics(*m)
                .color(*c)
        })
        .collect();
    let mut buffer = Buffer::new(&mut fonts.system, first.1);
    buffer.set_size(Some(box_w), None);
    buffer.set_wrap(Wrap::WordOrGlyph);
    buffer.set_rich_text(
        spans.iter().map(|(t, i)| (t.as_str(), attrs[*i].clone())),
        &attrs[0],
        Shaping::Advanced,
        None,
    );
    for (line, align) in buffer.lines.iter_mut().zip(&aligns) {
        line.set_align(*align);
    }
    buffer.shape_until_scroll(&mut fonts.system, false);

    // Per-glyph fallback (character missing from the chosen font) is reported.
    let wanted: Vec<String> = styles.iter().map(|s| s.0.family.to_lowercase()).collect();
    let mut fallback = 0usize;
    let mut text_h = 0f32;
    for run in buffer.layout_runs() {
        text_h = text_h.max(run.line_top + run.line_height);
        for g in run.glyphs {
            if !fonts
                .face_families(g.font_id)
                .iter()
                .any(|f| wanted.contains(&f.to_lowercase()))
            {
                fallback += 1;
            }
        }
    }
    if fallback > 0 {
        warn(
            frame,
            format!(
                "{fallback} character(s) aren't in the slide's font and are drawn from another font"
            ),
        );
    }
    if text_h > box_h + 0.5 {
        warn(
            frame,
            format!(
                "text is taller than its box ({:.0} vs {:.0} canvas px)",
                text_h / p.scale,
                el.frame.height
            ),
        );
    }

    Ok(Some((buffer, text_h)))
}

/// A premultiplied RGBA pixel buffer with a clip rectangle `(x0, y0, x1, y1)`.
struct Layer<'a> {
    buf: &'a mut [u8],
    width: usize,
    clip: (i32, i32, i32, i32),
}

impl Layer<'_> {
    fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        let (cx0, cy0, cx1, cy1) = self.clip;
        let xa = (x.round() as i32).max(cx0);
        let ya = (y.round() as i32).max(cy0);
        let xb = ((x + w).round() as i32).min(cx1);
        let yb = ((y + h).round() as i32).min(cy1);
        for yy in ya..yb {
            for xx in xa..xb {
                let i = (yy as usize * self.width + xx as usize) * 4;
                blend(&mut self.buf[i..i + 4], color);
            }
        }
    }

    /// Draw laid-out text with its top-left at `(ox, oy)`.
    fn draw(&mut self, fonts: &mut Fonts, buffer: &mut Buffer, ox: i32, oy: i32) {
        let (cx0, cy0, cx1, cy1) = self.clip;
        let width = self.width;
        let buf = &mut *self.buf;
        buffer.draw(
            &mut fonts.system,
            &mut fonts.swash,
            Color::rgba(0, 0, 0, 255),
            |x, y, w, h, color| {
                let a = color.a();
                if a == 0 {
                    return;
                }
                for yy in (oy + y).max(cy0)..(oy + y + h as i32).min(cy1) {
                    for xx in (ox + x).max(cx0)..(ox + x + w as i32).min(cx1) {
                        let i = (yy as usize * width + xx as usize) * 4;
                        blend(&mut buf[i..i + 4], [color.r(), color.g(), color.b(), a]);
                    }
                }
            },
        );
    }
}

/// Composite a premultiplied layer onto the frame, rotated `degrees` clockwise so
/// that the layer point `pivot_src` lands on frame point `pivot_dst`. Bilinear
/// sampling, clipped to the canvas.
fn composite_rotated(
    frame: &mut SlideFrame,
    src: &[u8],
    (sw, sh): (usize, usize),
    pivot_src: (f32, f32),
    pivot_dst: (f32, f32),
    degrees: f32,
) {
    let (sin, cos) = degrees.to_radians().sin_cos();
    // Frame-space bounds of the rotated layer.
    let corners = [
        (0.0, 0.0),
        (sw as f32, 0.0),
        (0.0, sh as f32),
        (sw as f32, sh as f32),
    ];
    let (mut bx0, mut by0, mut bx1, mut by1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in corners {
        let (dx, dy) = (x - pivot_src.0, y - pivot_src.1);
        let fx = pivot_dst.0 + dx * cos - dy * sin;
        let fy = pivot_dst.1 + dx * sin + dy * cos;
        bx0 = bx0.min(fx);
        by0 = by0.min(fy);
        bx1 = bx1.max(fx);
        by1 = by1.max(fy);
    }
    let c = frame.placement.canvas;
    let xa = (bx0.floor() as i64).max(i64::from(c.x));
    let ya = (by0.floor() as i64).max(i64::from(c.y));
    let xb = (bx1.ceil() as i64).min(i64::from(c.x + c.width));
    let yb = (by1.ceil() as i64).min(i64::from(c.y + c.height));
    let at = |x: i64, y: i64| -> [f32; 4] {
        if x < 0 || y < 0 || x >= sw as i64 || y >= sh as i64 {
            return [0.0; 4];
        }
        let i = (y as usize * sw + x as usize) * 4;
        [
            f32::from(src[i]),
            f32::from(src[i + 1]),
            f32::from(src[i + 2]),
            f32::from(src[i + 3]),
        ]
    };
    let width = frame.width as usize;
    for yy in ya..yb {
        for xx in xa..xb {
            // Inverse-rotate the pixel centre into layer space.
            let (dx, dy) = (xx as f32 + 0.5 - pivot_dst.0, yy as f32 + 0.5 - pivot_dst.1);
            let lx = pivot_src.0 + dx * cos + dy * sin - 0.5;
            let ly = pivot_src.1 - dx * sin + dy * cos - 0.5;
            let (fx, fy) = (lx.floor(), ly.floor());
            let (tx, ty) = (lx - fx, ly - fy);
            let (ix, iy) = (fx as i64, fy as i64);
            let (p00, p10, p01, p11) = (
                at(ix, iy),
                at(ix + 1, iy),
                at(ix, iy + 1),
                at(ix + 1, iy + 1),
            );
            let mut px = [0u8; 4];
            for k in 0..4 {
                let top = p00[k] + (p10[k] - p00[k]) * tx;
                let bot = p01[k] + (p11[k] - p01[k]) * tx;
                px[k] = (top + (bot - top) * ty).round().clamp(0.0, 255.0) as u8;
            }
            if px[3] == 0 {
                continue;
            }
            let i = (yy as usize * width + xx as usize) * 4;
            let inv = 255 - px[3];
            let d = &mut frame.text[i..i + 4];
            for k in 0..4 {
                d[k] = px[k].saturating_add(mul(d[k], inv));
            }
        }
    }
}

/// Drop spaces/tabs at the end of each line (layout only). PP6 (WPF) ignores them
/// when aligning, so "sing \n" centres like "sing\n"; real songs have many.
fn trim_line_ends(spans: &mut [(String, usize)]) {
    let mut at_line_end = true;
    for (text, _) in spans.iter_mut().rev() {
        let mut kept: Vec<char> = Vec::with_capacity(text.len());
        for c in text.chars().rev() {
            match c {
                ' ' | '\t' if at_line_end => {}
                '\n' => {
                    at_line_end = true;
                    kept.push(c);
                }
                _ => {
                    at_line_end = false;
                    kept.push(c);
                }
            }
        }
        if kept.len() != text.chars().count() {
            *text = kept.into_iter().rev().collect();
        }
    }
}

fn style_name(bold: bool, italic: bool) -> &'static str {
    match (bold, italic) {
        (true, true) => "bold italic",
        (true, false) => "bold",
        (false, true) => "italic",
        (false, false) => "regular",
    }
}

/// Source-over blend of a straight-alpha colour onto a premultiplied pixel.
fn blend(dst: &mut [u8], [r, g, b, a]: [u8; 4]) {
    let inv = 255 - a;
    dst[0] = mul(r, a).saturating_add(mul(dst[0], inv));
    dst[1] = mul(g, a).saturating_add(mul(dst[1], inv));
    dst[2] = mul(b, a).saturating_add(mul(dst[2], inv));
    dst[3] = a.saturating_add(mul(dst[3], inv));
}

#[cfg(test)]
mod tests {
    use super::trim_line_ends;

    #[test]
    fn trailing_spaces_are_trimmed_across_runs_and_lines() {
        let mut s = vec![
            ("sing ".to_string(), 0),
            (" \nlove; ".to_string(), 1),
            ("\n".to_string(), 1),
            ("a  b ".to_string(), 0),
        ];
        trim_line_ends(&mut s);
        let t: Vec<&str> = s.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(t, ["sing", "\nlove;", "\n", "a  b"]);
    }
}
