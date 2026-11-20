//! Lay out and rasterise one slide.
//!
//! Assumptions (docs/PLAN.md Unknowns): one RTF point = one canvas pixel, so
//! `\fs` half-points / 2 = font size in canvas pixels; RTF "auto" colour (no
//! `\cf`) = black. `verticalAlignment`: 0 = middle is **verified** (every real
//! file uses 0 and PP6 shows it centred); 1 = top, 2 = bottom are assumed.
//! Layout happens directly at output scale (font sizes and boxes multiplied by
//! the fit scale), so text stays sharp on any output resolution.

use std::time::Instant;

use cosmic_text::{Align as CtAlign, Attrs, Buffer, Color, Family, Metrics, Shaping, Wrap};
use pro6::rtf::Align;
use pro6::{DisplayElement, Document, Rgba, Slide, TextElement};

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
        background: background(doc, slide),
        text: vec![0; width as usize * height as usize * 4],
        warnings: Vec::new(),
        render_time: Default::default(),
    };
    for el in slide.elements() {
        match el {
            DisplayElement::Text(t) => render_text(fonts, doc, slide, t, &mut frame)?,
            DisplayElement::Other(raw) => frame.warnings.push(RenderWarning {
                element_uuid: pro6::raw::RawElement::attr(raw, "UUID")
                    .map(|a| a.raw_value.clone())
                    .unwrap_or_default(),
                message: format!("<{}> elements aren't drawn yet", raw.name),
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

/// Slide background if the slide draws one, else the document's, else `None`
/// (black, or background media when there is some).
fn background(doc: &Document, slide: &Slide) -> Option<[u8; 4]> {
    let draws = |e: &pro6::Extra| {
        e.element
            .attr("drawingBackgroundColor")
            .is_some_and(|a| a.raw_value == "true")
    };
    let c = if draws(&slide.extra) {
        slide.background_color
    } else if draws(&doc.extra) {
        doc.background_color
    } else {
        None
    };
    c.map(to_rgba8)
}

fn to_rgba8(c: Rgba) -> [u8; 4] {
    let q = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [q(c.r), q(c.g), q(c.b), q(c.a)]
}

fn render_text(
    fonts: &mut Fonts,
    doc: &Document,
    slide: &Slide,
    el: &TextElement,
    frame: &mut SlideFrame,
) -> Result<(), RenderError> {
    let warn = |frame: &mut SlideFrame, message: String| {
        frame.warnings.push(RenderWarning {
            element_uuid: el.uuid.clone(),
            message,
        });
    };
    if el.drawing_shadow {
        warn(frame, "text shadow isn't drawn yet".into());
    }
    if el.drawing_stroke
        && el
            .stroke
            .as_ref()
            .and_then(|s| s.width.as_ref())
            .is_some_and(|w| w.value > 0.0)
    {
        warn(frame, "text outline (stroke) isn't drawn yet".into());
    }
    // 360° (seen in real files) is no rotation.
    if el.rotation.rem_euclid(360.0) != 0.0 {
        warn(
            frame,
            format!("rotation {}° isn't applied yet", el.rotation),
        );
    }

    let p = frame.placement;
    let r = el.position.value;
    let (x0, y0) = p.map(r.x, r.y);
    let box_w = (r.width as f32 * p.scale).max(1.0);
    let box_h = r.height as f32 * p.scale;

    if el.drawing_fill
        && let Some(fill) = el.fill_color
    {
        if el.bezel_radius > 0.0 {
            warn(
                frame,
                "rounded corners on the element fill aren't drawn yet".into(),
            );
        }
        let c = to_rgba8(fill);
        let a = (f64::from(c[3]) * el.opacity).round() as u8;
        fill_rect(frame, x0, y0, box_w, box_h, [c[0], c[1], c[2], a]);
    }

    let Some(rtf) = el.rtf_doc(&doc.path)? else {
        warn(frame, "element has no RTFData; its text isn't drawn".into());
        return Ok(());
    };

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
    for (pi, para) in rtf.paragraphs.iter().enumerate() {
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
            let Some(name) = &run.font else {
                return Err(RenderError::FontNotInTable {
                    index: run.font_index,
                    file: doc.path.clone(),
                    slide_uuid: slide.uuid.clone(),
                    element_uuid: el.uuid.clone(),
                }
                .logged());
            };
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
            let px = run.size_half_pt as f32 / 2.0 * p.scale;
            let metrics = Metrics::new(px, fonts.line_height(&font, px));
            let color = match run.color {
                Some(c) => Color::rgba(c.r, c.g, c.b, (255.0 * el.opacity).round() as u8),
                None => Color::rgba(0, 0, 0, (255.0 * el.opacity).round() as u8),
            };
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
        return Ok(()); // no text
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
                r.height
            ),
        );
    }

    let dy = match el.vertical_alignment {
        1 => 0.0,
        2 => box_h - text_h,
        _ => (box_h - text_h) / 2.0,
    };
    let (ox, oy) = (x0.round() as i32, (y0 + dy).round() as i32);
    let c = p.canvas;
    let (cx0, cy0, cx1, cy1) = (
        c.x as i32,
        c.y as i32,
        (c.x + c.width) as i32,
        (c.y + c.height) as i32,
    );
    let width = frame.width as usize;
    let text = &mut frame.text;
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
                    blend(&mut text[i..i + 4], [color.r(), color.g(), color.b(), a]);
                }
            }
        },
    );
    Ok(())
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

fn fill_rect(frame: &mut SlideFrame, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
    let c = frame.placement.canvas;
    let x0 = (x.round() as i64).max(i64::from(c.x));
    let y0 = (y.round() as i64).max(i64::from(c.y));
    let x1 = ((x + w).round() as i64).min(i64::from(c.x + c.width));
    let y1 = ((y + h).round() as i64).min(i64::from(c.y + c.height));
    for yy in y0..y1 {
        for xx in x0..x1 {
            let i = (yy as usize * frame.width as usize + xx as usize) * 4;
            blend(&mut frame.text[i..i + 4], color);
        }
    }
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
