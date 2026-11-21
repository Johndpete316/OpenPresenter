//! Text box layout with cosmic-text: shared by drawing (`slide.rs`) and by the
//! editor's caret, selection and hit testing ([`layout_box`]), so the caret sits
//! exactly on the text the output shows.

use std::path::Path;

use cosmic_text::{Align as CtAlign, Attrs, Buffer, Color, Cursor, Family, Metrics, Shaping, Wrap};
use doc::rich::Pos;
use doc::{Align, TextBox, TextProblem, VAlign};

use crate::error::RenderError;
use crate::fonts::{Fonts, ResolvedFont};

/// Who a layout error is about.
pub(crate) struct Ctx<'a> {
    pub file: &'a Path,
    pub slide: &'a str,
    pub element: &'a str,
}

#[derive(Clone, PartialEq)]
struct Style {
    font: ResolvedFont,
    metrics: Metrics,
    color: Color,
    underline: bool,
    /// Letter spacing in em.
    tracking: f32,
}

/// One buffer line: a paragraph, or the part of one between line breaks.
struct LineMap {
    para: usize,
    /// Byte offset of the line's text in the paragraph's text.
    start: usize,
    /// Length before trailing spaces were dropped for layout.
    orig_len: usize,
}

pub(crate) struct TextLayout {
    pub buffer: Buffer,
    /// Per style (glyph `metadata`): underline and colour.
    pub underline: Vec<bool>,
    pub colors: Vec<Color>,
    lines: Vec<LineMap>,
    /// Height of the laid-out text, output pixels.
    pub text_h: f32,
    /// Messages for the slide's warnings (inexact faces, fallback glyphs).
    pub notes: Vec<String>,
}

/// Lay out `el`'s text at `scale` output pixels per canvas pixel, `opacity` applied to
/// the colours. `None` when there's no styled text at all. Empty text gives a layout
/// with no glyphs when `allow_empty` (the editor needs a caret line).
pub(crate) fn layout(
    fonts: &mut Fonts,
    el: &TextBox,
    scale: f32,
    allow_empty: bool,
    ctx: &Ctx<'_>,
) -> Result<Option<TextLayout>, RenderError> {
    match &el.problem {
        None => {}
        Some(TextProblem::NoStyledText) => return Ok(None),
        Some(TextProblem::Unreadable { message }) => {
            return Err(RenderError::BadText {
                message: message.clone(),
                file: ctx.file.to_path_buf(),
                slide_uuid: ctx.slide.to_owned(),
                element_uuid: ctx.element.to_owned(),
            }
            .logged());
        }
        Some(TextProblem::FontNotInTable { index }) => {
            return Err(RenderError::FontNotInTable {
                index: *index,
                file: ctx.file.to_path_buf(),
                slide_uuid: ctx.slide.to_owned(),
                element_uuid: ctx.element.to_owned(),
            }
            .logged());
        }
    }
    let empty = el.runs().all(|r| r.text.is_empty());
    if empty && !allow_empty {
        return Ok(None);
    }
    let box_w = (el.frame.width as f32 * scale).max(1.0);
    let alpha = (255.0 * el.opacity.clamp(0.0, 1.0)).round() as u8;
    let mut notes = Vec::new();

    // Resolve every run's font first: a missing font is an error, never a substitute.
    let mut styles: Vec<Style> = Vec::new();
    let mut spans: Vec<(String, usize)> = Vec::new();
    let mut aligns: Vec<Option<CtAlign>> = Vec::new();
    let mut lines: Vec<LineMap> = Vec::new();
    // Line breaks before the first styled run (empty leading paragraphs).
    let mut pending_breaks = 0usize;
    for (pi, para) in el.paragraphs.iter().enumerate() {
        if pi > 0 {
            // Paragraph break carries the previous style (sets the empty line's height).
            match spans.last() {
                Some(&(_, s)) => spans.push(("\n".into(), s)),
                None => pending_breaks += 1,
            }
        }
        let align = match para.align {
            Align::Left => CtAlign::Left,
            Align::Center => CtAlign::Center,
            Align::Right => CtAlign::Right,
            Align::Justified => CtAlign::Justified,
        };
        let text = doc::rich::text(para);
        let mut start = 0;
        for seg in text.split('\n') {
            lines.push(LineMap {
                para: pi,
                start,
                orig_len: seg.len(),
            });
            start += seg.len() + 1;
            aligns.push(Some(align));
        }
        for run in &para.runs {
            let font = fonts
                .resolve(&run.font, run.bold, run.italic)
                .ok_or_else(|| {
                    RenderError::MissingFont {
                        font: run.font.clone(),
                        file: ctx.file.to_path_buf(),
                        slide_uuid: ctx.slide.to_owned(),
                        element_uuid: ctx.element.to_owned(),
                    }
                    .logged()
                })?;
            if font.inexact {
                notes.push(format!(
                    "'{}' has no {} face; the nearest face of the same font is used",
                    font.family,
                    style_name(run.bold, run.italic)
                ));
            }
            let px = (run.size as f32 * scale).max(0.5);
            let lh = fonts.line_height(&font, px) * para.line_spacing.max(0.1) as f32;
            let [cr, cg, cb] = run.color.unwrap_or([0, 0, 0]);
            let style = Style {
                font,
                metrics: Metrics::new(px, lh),
                color: Color::rgba(cr, cg, cb, alpha),
                underline: run.underline,
                tracking: if run.size > 0.0 {
                    (run.letter_spacing / run.size) as f32
                } else {
                    0.0
                },
            };
            let idx = match styles.iter().position(|s| *s == style) {
                Some(i) => i,
                None => {
                    styles.push(style);
                    styles.len() - 1
                }
            };
            if pending_breaks > 0 {
                spans.push(("\n".repeat(pending_breaks), idx));
                pending_breaks = 0;
            }
            spans.push((run.text.clone(), idx));
        }
    }
    trim_line_ends(&mut spans);
    let Some(first) = styles.first() else {
        return Ok(None); // no styled run at all
    };
    let first_metrics = first.metrics;
    let attrs: Vec<Attrs<'_>> = styles
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut a = Attrs::new()
                .family(Family::Name(&s.font.family))
                .weight(s.font.weight)
                .style(s.font.style)
                .metrics(s.metrics)
                .color(s.color)
                .metadata(i);
            if s.tracking != 0.0 {
                a = a.letter_spacing(s.tracking);
            }
            a
        })
        .collect();
    let mut buffer = Buffer::new(&mut fonts.system, first_metrics);
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
    let wanted: Vec<String> = styles
        .iter()
        .map(|s| s.font.family.to_lowercase())
        .collect();
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
        notes.push(format!(
            "{fallback} character(s) aren't in the slide's font and are drawn from another font"
        ));
    }
    if styles.iter().any(|s| s.underline) {
        // Drawn (see slide.rs); nothing to report.
    }
    Ok(Some(TextLayout {
        buffer,
        underline: styles.iter().map(|s| s.underline).collect(),
        colors: styles.iter().map(|s| s.color).collect(),
        lines,
        text_h,
        notes,
    }))
}

/// Vertical offset of the text in its box (output pixels).
pub(crate) fn valign_offset(valign: VAlign, box_h: f32, text_h: f32) -> f32 {
    match valign {
        VAlign::Top => 0.0,
        VAlign::Bottom => box_h - text_h,
        VAlign::Middle => (box_h - text_h) / 2.0,
    }
}

impl TextLayout {
    fn kept_len(&self, line: usize) -> usize {
        self.buffer.lines.get(line).map_or(0, |l| l.text().len())
    }

    /// Buffer cursor for a text position.
    fn cursor(&self, pos: Pos) -> Cursor {
        let line = self
            .lines
            .iter()
            .position(|l| l.para == pos.para && pos.offset <= l.start + l.orig_len)
            .or_else(|| self.lines.iter().rposition(|l| l.para <= pos.para))
            .unwrap_or(0);
        let l = &self.lines[line];
        let index = pos.offset.saturating_sub(l.start).min(self.kept_len(line));
        Cursor::new(line, index)
    }

    fn pos(&self, c: Cursor) -> Pos {
        match self.lines.get(c.line) {
            Some(l) => Pos::new(l.para, l.start + c.index.min(self.kept_len(c.line))),
            None => Pos::default(),
        }
    }
}

/// A text box laid out for editing, in canvas pixels relative to the box's
/// top-left corner (unrotated).
pub struct BoxLayout {
    layout: Option<TextLayout>,
    /// Output pixels per canvas pixel the layout was made at.
    scale: f32,
    /// Text top, box-local output pixels.
    dy: f32,
    box_w: f32,
    box_h: f32,
    align: Align,
    first_size: f32,
}

/// Lay out a text box for the editor at `scale` output pixels per canvas pixel (use
/// the canvas's on-screen pixel scale so it matches the image exactly).
pub fn layout_box(
    fonts: &mut Fonts,
    file: &Path,
    slide_uuid: &str,
    el: &TextBox,
    scale: f32,
) -> Result<BoxLayout, RenderError> {
    let scale = scale.max(0.01);
    let ctx = Ctx {
        file,
        slide: slide_uuid,
        element: &el.uuid,
    };
    let layout = layout(fonts, el, scale, true, &ctx)?;
    let box_w = (el.frame.width as f32 * scale).max(1.0);
    let box_h = el.frame.height as f32 * scale;
    let text_h = layout.as_ref().map_or(0.0, |l| l.text_h);
    Ok(BoxLayout {
        dy: valign_offset(el.valign, box_h, text_h),
        layout,
        scale,
        box_w,
        box_h,
        align: el.paragraphs.first().map(|p| p.align).unwrap_or_default(),
        first_size: el.runs().next().map_or(40.0, |r| r.size as f32),
    })
}

impl BoxLayout {
    /// Laid-out text height, canvas pixels.
    pub fn text_height(&self) -> f32 {
        self.layout.as_ref().map_or(0.0, |l| l.text_h) / self.scale
    }

    /// Text taller than its box.
    pub fn overflows(&self) -> bool {
        self.layout
            .as_ref()
            .is_some_and(|l| l.text_h > self.box_h + 0.5)
    }

    /// Text position nearest to box-local canvas point `(x, y)`.
    pub fn hit(&self, x: f32, y: f32) -> Pos {
        let Some(l) = &self.layout else {
            return Pos::default();
        };
        let (px, py) = (x * self.scale, y * self.scale - self.dy);
        let total = l.text_h;
        let py = py.clamp(0.0, (total - 0.5).max(0.0));
        match l.buffer.hit(px, py) {
            Some(c) => l.pos(c),
            None => Pos::default(),
        }
    }

    /// Caret at `pos`: `(x, top, height)` in box-local canvas pixels.
    pub fn caret(&self, pos: Pos) -> (f32, f32, f32) {
        let fallback_h = self.first_size * 1.2;
        let empty_x = match self.align {
            Align::Center => self.box_w / 2.0,
            Align::Right => self.box_w,
            Align::Left | Align::Justified => 0.0,
        } / self.scale;
        let Some(l) = &self.layout else {
            return (empty_x, self.dy / self.scale, fallback_h);
        };
        let c = l.cursor(pos);
        for run in l.buffer.layout_runs() {
            if run.line_i != c.line {
                continue;
            }
            if let Some(x) = run.cursor_position(&c) {
                return (
                    x / self.scale,
                    (run.line_top + self.dy) / self.scale,
                    run.line_height / self.scale,
                );
            }
        }
        // An empty line: its first run, at the alignment's edge.
        let run = l.buffer.layout_runs().find(|r| r.line_i == c.line);
        match run {
            Some(r) => {
                let x = match self.align {
                    Align::Center => self.box_w / 2.0,
                    Align::Right => self.box_w,
                    Align::Left | Align::Justified => 0.0,
                };
                (
                    x / self.scale,
                    (r.line_top + self.dy) / self.scale,
                    r.line_height / self.scale,
                )
            }
            None => (empty_x, self.dy / self.scale, fallback_h),
        }
    }

    /// Selection between `a` and `b` as rectangles `(x, y, w, h)`, box-local canvas pixels.
    pub fn selection(&self, a: Pos, b: Pos) -> Vec<[f32; 4]> {
        let Some(l) = &self.layout else {
            return Vec::new();
        };
        let (a, b) = (a.min(b), a.max(b));
        let (ca, cb) = (l.cursor(a), l.cursor(b));
        let mut out = Vec::new();
        for run in l.buffer.layout_runs() {
            if run.line_i < ca.line || run.line_i > cb.line {
                continue;
            }
            for (x, w) in run.highlight(ca, cb) {
                out.push([
                    x / self.scale,
                    (run.line_top + self.dy) / self.scale,
                    w / self.scale,
                    run.line_height / self.scale,
                ]);
            }
        }
        out
    }
}

/// Drop spaces/tabs at the end of each line (layout only). PP6 (WPF) ignores them
/// when aligning, so "sing \n" centres like "sing\n"; real songs have many.
pub(crate) fn trim_line_ends(spans: &mut [(String, usize)]) {
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
