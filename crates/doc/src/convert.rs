//! `.pro6` ⇄ native conversion.
//!
//! Import reads everything the native model describes; the `.pro6` itself is kept
//! on the document (`Document::pro6`). Export starts from that original and changes
//! only what differs from a fresh import of it (the "baseline"): untouched parts are
//! written back byte-for-byte, and anything the native model doesn't describe
//! (cues, media, shapes, unknown XML) is kept.
//!
//! Each native group, slide and element records in `src` which original node it
//! came from (itself, or the original of a copy), so moves and copies keep their XML.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use pro6::ops::{Op, new_group, refresh_raw_uuids};
use pro6::{DisplayElement, WinPayloads};

use crate::FORMAT_VERSION;
use crate::compat::Mode;
use crate::model::*;

/// Convert a parsed `.pro6`. Never fails: text that can't be read is kept and
/// marked (`TextBox::problem`), so the slide reports it when drawn.
pub fn from_pro6(src: pro6::Document) -> Document {
    let src = Arc::new(src);
    let mut d = convert(&src);
    d.pro6 = Some(src);
    d
}

fn rgba(c: pro6::Rgba) -> Rgba {
    Rgba {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

const BLACK: Rgba = Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

fn to_p(c: Rgba) -> pro6::Rgba {
    pro6::Rgba {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

fn draws_background(el: &pro6::raw::RawElement) -> bool {
    el.attr("drawingBackgroundColor")
        .is_some_and(|a| a.raw_value == "true")
}

/// Stable keys for the nodes of a `.pro6`: the UUID, made unique by position when
/// it's missing or repeated.
#[derive(Default)]
struct Keys {
    seen: HashMap<String, usize>,
}

impl Keys {
    fn key(&mut self, uuid: &str, fallback: impl FnOnce() -> String) -> String {
        let base = if uuid.is_empty() {
            fallback()
        } else {
            uuid.to_owned()
        };
        let n = self.seen.entry(base.clone()).or_default();
        *n += 1;
        if *n == 1 { base } else { format!("{base}#{n}") }
    }
}

fn raw_uuid(raw: &pro6::raw::RawElement) -> String {
    raw.attr("UUID")
        .map(|a| a.raw_value.clone())
        .unwrap_or_default()
}

/// Native view of a `.pro6` without keeping the source (read-only uses: it can't
/// be saved back as `.pro6`).
pub fn convert_ref(p: &pro6::Document) -> Document {
    convert(p)
}

/// Native view of a `.pro6`, without keeping the source.
pub(crate) fn convert(p: &pro6::Document) -> Document {
    let path = &p.path;
    let timings = p.slide_timings();
    let timeline_cues = p
        .timeline
        .as_ref()
        .map_or(0, |t| t.time_cue_count() + t.media_track_count());
    let cue_report = match &timings {
        Some(t) => CueReport {
            other_cues: t.other_cues,
            with_delay: t.with_delay,
            timeline_cues,
        },
        None => CueReport {
            other_cues: p.slides().map(pro6::Slide::cue_count).sum(),
            with_delay: 0,
            timeline_cues,
        },
    };
    let mut keys = Keys::default();
    let groups =
        p.groups()
            .iter()
            .enumerate()
            .map(|(gi, g)| {
                let gkey = keys.key(&g.uuid, || format!("group#{gi}"));
                let slides =
                    g.slides()
                        .iter()
                        .enumerate()
                        .map(|(si, s)| {
                            let skey = keys.key(&s.uuid, || format!("{gkey}/slide#{si}"));
                            let elements = s
                                .elements()
                                .iter()
                                .enumerate()
                                .map(|(ei, e)| match e {
                                    DisplayElement::Text(t) => {
                                        let k = keys.key(&t.uuid, || format!("{skey}/el#{ei}"));
                                        Element::Text(Box::new(text_box(t, path, k)))
                                    }
                                    DisplayElement::Other(raw) => {
                                        let uuid = raw_uuid(raw);
                                        let k = keys.key(&uuid, || format!("{skey}/el#{ei}"));
                                        match image_box(raw, &uuid) {
                                            Some(mut i) => {
                                                i.src = Some(k);
                                                Element::Image(Box::new(i))
                                            }
                                            None => Element::Pro6(Pro6Element {
                                                uuid,
                                                kind: raw.name.clone(),
                                                src: Some(k),
                                            }),
                                        }
                                    }
                                })
                                .collect();
                            Slide {
                                uuid: s.uuid.clone(),
                                enabled: s.enabled,
                                label: s.label.clone(),
                                notes: s.notes.clone(),
                                hot_key: s.hot_key.clone(),
                                background: s
                                    .background_color
                                    .filter(|_| draws_background(&s.extra.element))
                                    .map(rgba),
                                background_image: None,
                                elements,
                                timer: timings.as_ref().and_then(|t| t.timers.get(&s.uuid)).map(
                                    |t| SlideTimer {
                                        duration: t.duration,
                                        loop_to_beginning: t.loop_to_beginning,
                                    },
                                ),
                                video: None,
                                pro6_incompatible: false,
                                theme: None,
                                src: Some(skey),
                            }
                        })
                        .collect();
                Group {
                    uuid: g.uuid.clone(),
                    name: g.name.clone(),
                    color: g.color.map(rgba),
                    slides,
                    src: Some(gkey),
                }
            })
            .collect();
    let arrangements = p
        .arrangements()
        .into_iter()
        .map(|a| Arrangement {
            uuid: a.uuid,
            name: a.name,
            color: a.color.map(rgba),
            groups: a.group_ids,
        })
        .collect();
    Document {
        path: p.path.clone(),
        format_version: FORMAT_VERSION,
        mode: Mode::Pro6,
        canvas: CanvasSize {
            width: p.canvas.width,
            height: p.canvas.height,
        },
        background: p
            .background_color
            .filter(|_| draws_background(&p.extra.element))
            .map(rgba),
        selected_arrangement: p.selected_arrangement_id.clone(),
        groups,
        arrangements,
        cue_report,
        theme: None,
        pro6: None,
    }
}

/// Decode `%XX` escapes (PP6 stores image paths URL-encoded).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The values PP6 writes for an image drawn plainly (seen in real files). Anything
/// else (scaling, offsets, fill, shadow, stroke, rounded corners) isn't understood
/// yet, so such elements are kept as raw `.pro6` and not drawn.
fn plain_image_attrs(raw: &pro6::raw::RawElement) -> bool {
    let is = |name: &str, ok: &[&str]| {
        raw.attr(name)
            .is_none_or(|a| ok.contains(&a.raw_value.as_str()))
    };
    is("drawingFill", &["false"])
        && is("drawingShadow", &["false"])
        && is("drawingStroke", &["false"])
        && is("bezelRadius", &["0"])
        && is("scaleBehavior", &["0"])
        && is("scaleSize", &["{1, 1}"])
        && is("imageOffset", &["{0, 0}"])
        && is("flippedHorizontally", &["0", "1"])
        && is("flippedVertically", &["0", "1"])
}

fn raw_attr(raw: &pro6::raw::RawElement, name: &str) -> Option<String> {
    raw.attr(name)
        .and_then(|a| a.value().ok())
        .map(|v| v.into_owned())
}

fn raw_position(raw: &pro6::raw::RawElement) -> Option<(usize, pro6::model::Rect3D)> {
    raw.elements().find_map(|(i, c)| {
        (c.name == "RVRect3D" && c.ivar_name() == Some("position"))
            .then(|| c.text().ok().and_then(|t| pro6::model::Rect3D::parse(&t)))
            .flatten()
            .map(|r| (i, r))
    })
}

/// An `RVImageElement` the native model can describe, or `None`.
fn image_box(raw: &pro6::raw::RawElement, uuid: &str) -> Option<ImageBox> {
    if raw.name != "RVImageElement" || !plain_image_attrs(raw) {
        return None;
    }
    let source = raw_attr(raw, "source")?;
    if source.is_empty() {
        return None;
    }
    let mut path = percent_decode(&source);
    if let Some(p) = path.strip_prefix("file://") {
        path = p.to_owned();
    }
    let (_, r) = raw_position(raw)?;
    let num = |name: &str, default: f64| {
        raw_attr(raw, name)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(default)
    };
    let flag = |name: &str| raw_attr(raw, name).is_some_and(|v| v == "1" || v == "true");
    Some(ImageBox {
        uuid: uuid.to_owned(),
        name: raw_attr(raw, "displayName").unwrap_or_default(),
        frame: Rect {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        },
        rotation: num("rotation", 0.0),
        opacity: num("opacity", 1.0),
        locked: flag("locked"),
        hidden: false,
        flip_h: flag("flippedHorizontally"),
        flip_v: flag("flippedVertically"),
        media: MediaRef { path, sha256: None },
        adjust: Adjust::default(),
        src: None,
    })
}

/// Write an image's changed placement into its original `RVImageElement`.
fn export_image(i: &ImageBox, base: &ImageBox, raw: &mut pro6::raw::RawElement) {
    use pro6::write::fmt_f64;
    if i.name != base.name {
        raw.set_attr("displayName", &i.name);
    }
    if i.rotation != base.rotation {
        raw.set_attr("rotation", &fmt_f64(i.rotation));
    }
    if i.opacity != base.opacity {
        raw.set_attr("opacity", &fmt_f64(i.opacity));
    }
    if i.locked != base.locked {
        raw.set_attr("locked", if i.locked { "true" } else { "false" });
    }
    if i.flip_h != base.flip_h {
        raw.set_attr("flippedHorizontally", if i.flip_h { "1" } else { "0" });
    }
    if i.flip_v != base.flip_v {
        raw.set_attr("flippedVertically", if i.flip_v { "1" } else { "0" });
    }
    if i.frame != base.frame
        && let Some((idx, r)) = raw_position(raw)
        && let Some(pro6::raw::RawNode::Element(c)) = raw.children.get_mut(idx)
    {
        c.set_text(&format!(
            "{{{} {} {} {} {}}}",
            fmt_f64(i.frame.x),
            fmt_f64(i.frame.y),
            fmt_f64(r.z),
            fmt_f64(i.frame.width),
            fmt_f64(i.frame.height)
        ));
    }
}

fn align(a: pro6::rtf::Align) -> Align {
    match a {
        pro6::rtf::Align::Left => Align::Left,
        pro6::rtf::Align::Center => Align::Center,
        pro6::rtf::Align::Right => Align::Right,
        pro6::rtf::Align::Justified => Align::Justified,
    }
}

fn to_p_align(a: Align) -> pro6::rtf::Align {
    match a {
        Align::Left => pro6::rtf::Align::Left,
        Align::Center => pro6::rtf::Align::Center,
        Align::Right => pro6::rtf::Align::Right,
        Align::Justified => pro6::rtf::Align::Justified,
    }
}

fn valign(v: i64) -> VAlign {
    // 0 = middle is verified; 1 = top, 2 = bottom are assumed (render crate docs).
    match v {
        1 => VAlign::Top,
        2 => VAlign::Bottom,
        _ => VAlign::Middle,
    }
}

fn to_p_valign(v: VAlign) -> i64 {
    match v {
        VAlign::Middle => 0,
        VAlign::Top => 1,
        VAlign::Bottom => 2,
    }
}

/// Paragraphs from `PlainText` (unstyled), for boxes whose RTF can't be used.
fn plain_paragraphs(t: &pro6::TextElement) -> Vec<Paragraph> {
    let text = t
        .plain_text
        .as_ref()
        .map(|p| p.value.text())
        .unwrap_or_default()
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .map(|line| Paragraph {
            align: Align::Left,
            line_spacing: 1.0,
            runs: vec![Run {
                text: line.to_owned(),
                font: String::new(),
                size: 0.0,
                color: None,
                bold: false,
                italic: false,
                underline: false,
                letter_spacing: 0.0,
            }],
        })
        .collect()
}

fn text_box(t: &pro6::TextElement, path: &Path, key: String) -> TextBox {
    let r = t.position.value;
    let (mut paragraphs, problem) = match t.rtf_doc(path) {
        Ok(Some(rtf)) => {
            let mut problem = None;
            let paragraphs = rtf
                .paragraphs
                .iter()
                .map(|p| Paragraph {
                    align: align(p.align),
                    line_spacing: 1.0,
                    runs: p
                        .runs
                        .iter()
                        .map(|run| Run {
                            text: run.text.clone(),
                            font: match &run.font {
                                Some(f) => f.clone(),
                                None => {
                                    problem.get_or_insert(TextProblem::FontNotInTable {
                                        index: run.font_index,
                                    });
                                    String::new()
                                }
                            },
                            size: f64::from(run.size_half_pt) / 2.0,
                            color: run.color.map(|c| [c.r, c.g, c.b]),
                            bold: run.bold,
                            italic: run.italic,
                            underline: run.underline,
                            letter_spacing: 0.0,
                        })
                        .collect(),
                })
                .collect();
            (paragraphs, problem)
        }
        Ok(None) => (plain_paragraphs(t), Some(TextProblem::NoStyledText)),
        Err(e) => (
            plain_paragraphs(t),
            Some(TextProblem::Unreadable {
                message: e.user_message(),
            }),
        ),
    };
    // PP6's "Double-click to edit" placeholder isn't shown: it's empty text in the
    // style it was given.
    if t.is_placeholder(path) {
        let first = paragraphs.iter().find_map(|p| {
            p.runs.first().map(|r| Paragraph {
                align: p.align,
                line_spacing: 1.0,
                runs: vec![Run {
                    text: String::new(),
                    ..r.clone()
                }],
            })
        });
        paragraphs = first.into_iter().collect();
    }
    TextBox {
        uuid: t.uuid.clone(),
        name: t.display_name.clone(),
        frame: Rect {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        },
        rotation: t.rotation,
        opacity: t.opacity,
        locked: t.locked,
        hidden: false,
        valign: valign(t.vertical_alignment),
        fill: t.fill_color.filter(|_| t.drawing_fill).map(rgba),
        corner_radius: t.bezel_radius,
        outline: t
            .stroke
            .as_ref()
            .filter(|_| t.drawing_stroke)
            .and_then(|s| {
                let width = s.width.as_ref()?.value;
                (width > 0.0).then(|| Outline {
                    width,
                    color: s.color.as_ref().map_or(BLACK, |c| rgba(c.value)),
                })
            }),
        shadow: t
            .shadow
            .as_ref()
            .filter(|_| t.drawing_shadow)
            // Assumed layout `blur|colour|{x, y}` with y up (Cocoa); unverified.
            .map(|s| Shadow {
                blur: s.value.blur.unwrap_or(0.0),
                color: s.value.color.map_or(BLACK, rgba),
                offset: s.value.offset.map_or((0.0, 0.0), |(x, y)| (x, -y)),
            }),
        paragraphs,
        problem,
        src: Some(key),
    }
}

// ---- export ---------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("the presentation wasn't made from a .pro6 file")]
    NoSource,
    #[error("{what} {uuid} has no .pro6 original to build on")]
    NoOriginal { what: &'static str, uuid: String },
    #[error("{what} can't be written to .pro6 yet")]
    Unsupported { what: &'static str },
    #[error("text box {uuid}: {source}")]
    Text {
        uuid: String,
        source: pro6::EditError,
    },
    #[error("arrangement {uuid}: {source}")]
    Arrangement {
        uuid: String,
        source: pro6::EditError,
    },
}

impl ExportError {
    pub fn user_message(&self) -> String {
        match self {
            ExportError::NoSource => {
                "This presentation can't be saved as a ProPresenter 6 file yet: it wasn't made from one. Nothing was saved.".to_owned()
            }
            ExportError::NoOriginal { what, .. } => format!(
                "A {what} in this presentation can't be written to a ProPresenter 6 file yet. Nothing was saved."
            ),
            ExportError::Unsupported { what } => format!(
                "{what} can't be written to a ProPresenter 6 file yet. Nothing was saved."
            ),
            ExportError::Text { source, .. } => format!(
                "A text box couldn't be written ({}). Nothing was saved.",
                source.user_message()
            ),
            ExportError::Arrangement { source, .. } => format!(
                "An arrangement couldn't be written ({}). Nothing was saved.",
                source.user_message()
            ),
        }
    }
}

/// What to do with slides marked `pro6_incompatible` when exporting a native
/// document (asked each time; John, 2026-11-21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportChoice {
    /// Leave those slides out.
    SkipIncompatible,
    /// Write them without the parts `.pro6` can't hold.
    DropUnsupported,
}

/// What an export left out (shown to the user).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExportReport {
    /// `"<group> / slide N"` for each slide left out or simplified.
    pub left_out: Vec<String>,
}

/// Export a native document to `.pro6`, applying `choice` to incompatible slides.
pub fn export_pro6(
    doc: &Document,
    choice: ExportChoice,
) -> Result<(pro6::Document, ExportReport), ExportError> {
    let mut report = ExportReport::default();
    for g in &doc.groups {
        for (i, s) in g.slides.iter().enumerate() {
            if s.pro6_incompatible {
                report
                    .left_out
                    .push(format!("{} / slide {}", g.name, i + 1));
            }
        }
    }
    let out = if choice == ExportChoice::SkipIncompatible && !report.left_out.is_empty() {
        let mut d = doc.clone();
        for g in &mut d.groups {
            g.slides.retain(|s| !s.pro6_incompatible);
        }
        to_pro6(&d)?
    } else {
        to_pro6(doc)?
    };
    Ok((out, report))
}

struct Index<'a> {
    groups: HashMap<&'a str, (&'a pro6::SlideGroup, &'a Group)>,
    slides: HashMap<&'a str, (&'a pro6::Slide, &'a Slide)>,
    elements: HashMap<&'a str, (&'a DisplayElement, &'a Element)>,
}

impl<'a> Index<'a> {
    fn new(p: &'a pro6::Document, base: &'a Document) -> Index<'a> {
        let mut ix = Index {
            groups: HashMap::new(),
            slides: HashMap::new(),
            elements: HashMap::new(),
        };
        let src = |s: &'a Option<String>| s.as_deref().unwrap_or_default();
        for (pg, g) in p.groups().iter().zip(&base.groups) {
            ix.groups.insert(src(&g.src), (pg, g));
            for (ps, s) in pg.slides().iter().zip(&g.slides) {
                ix.slides.insert(src(&s.src), (ps, s));
                for (pe, e) in ps.elements().iter().zip(&s.elements) {
                    ix.elements.insert(e.src().unwrap_or_default(), (pe, e));
                }
            }
        }
        ix
    }
}

fn empty_array(ivar: &str) -> pro6::extra::Extra {
    pro6::extra::Extra {
        element: pro6::raw::RawElement::new("array", &[("rvXMLIvarName", ivar)]),
        typed_slots: Vec::new(),
    }
}

fn set_background(color: &mut Option<pro6::Rgba>, el: &mut pro6::raw::RawElement, v: Option<Rgba>) {
    match v {
        Some(c) => {
            *color = Some(to_p(c));
            el.set_attr("drawingBackgroundColor", "true");
        }
        None => el.set_attr("drawingBackgroundColor", "false"),
    }
}

/// Export to `.pro6`, keeping everything untouched byte-identical. Slides marked
/// incompatible are written without their native-only parts (see [`export_pro6`]).
pub fn to_pro6(doc: &Document) -> Result<pro6::Document, ExportError> {
    let origin = doc.pro6.as_deref().ok_or(ExportError::NoSource)?;
    let base = convert(origin);
    let ix = Index::new(origin, &base);
    let mut out = origin.clone();

    out.canvas = pro6::CanvasSize {
        width: doc.canvas.width,
        height: doc.canvas.height,
    };
    if doc.background != base.background {
        set_background(
            &mut out.background_color,
            &mut out.extra.element,
            doc.background,
        );
    }

    let mut groups = Vec::with_capacity(doc.groups.len());
    for g in &doc.groups {
        let mut pg = match g.src.as_deref().and_then(|k| ix.groups.get(k)) {
            Some((o, _)) => {
                let mut c = (*o).clone();
                if let Some(s) = c.slides.as_mut() {
                    s.items.clear();
                }
                c
            }
            None => new_group(&g.name, g.color.map(to_p)),
        };
        pg.uuid.clone_from(&g.uuid);
        pg.name.clone_from(&g.name);
        pg.color = g.color.map(to_p);
        if !g.slides.is_empty() && pg.slides.is_none() {
            pg.slides = Some(pro6::TypedArray {
                items: Vec::new(),
                extra: empty_array("slides"),
            });
        }
        if let Some(arr) = pg.slides.as_mut() {
            for s in &g.slides {
                arr.items.push(export_slide(s, &ix, &doc.path)?);
            }
        }
        groups.push(pg);
    }
    match out.groups.as_mut() {
        Some(arr) => arr.items = groups,
        None if !groups.is_empty() => {
            out.groups = Some(pro6::TypedArray {
                items: groups,
                extra: empty_array("groups"),
            });
        }
        None => {}
    }

    export_arrangements(doc, &base, &mut out)?;
    out.selected_arrangement_id
        .clone_from(&doc.selected_arrangement);
    Ok(out)
}

fn export_slide(s: &Slide, ix: &Index<'_>, path: &Path) -> Result<pro6::Slide, ExportError> {
    let Some((po, base)) = s.src.as_deref().and_then(|k| ix.slides.get(k)) else {
        return Err(ExportError::NoOriginal {
            what: "slide",
            uuid: s.uuid.clone(),
        });
    };
    let mut ps = (*po).clone();
    if ps.uuid != s.uuid {
        // A copy: everything below it with an ID gets a new one.
        refresh_raw_uuids(&mut ps.extra.element);
        ps.uuid.clone_from(&s.uuid);
    }
    ps.enabled = s.enabled;
    ps.label.clone_from(&s.label);
    ps.notes.clone_from(&s.notes);
    ps.hot_key.clone_from(&s.hot_key);
    if s.background != base.background {
        set_background(
            &mut ps.background_color,
            &mut ps.extra.element,
            s.background,
        );
    }
    let mut items = Vec::with_capacity(s.elements.len());
    for e in &s.elements {
        // Elements made here with nothing to copy (images, text boxes in a
        // presentation without one) and hidden elements are native-only: their
        // slide is flagged, and an export writes it without them.
        if e.src().is_none() || e.hidden() {
            continue;
        }
        items.push(export_element(e, ix, path)?);
    }
    match ps.elements.as_mut() {
        Some(arr) => arr.items = items,
        None if !items.is_empty() => {
            ps.elements = Some(pro6::TypedArray {
                items,
                extra: empty_array("displayElements"),
            });
        }
        None => {}
    }
    Ok(ps)
}

fn export_element(e: &Element, ix: &Index<'_>, path: &Path) -> Result<DisplayElement, ExportError> {
    let Some((po, base)) = e.src().and_then(|k| ix.elements.get(k)) else {
        return Err(ExportError::NoOriginal {
            what: "slide element",
            uuid: e.uuid().to_owned(),
        });
    };
    match (e, po, base) {
        (Element::Pro6(o), DisplayElement::Other(raw), _) => {
            let mut raw = raw.clone();
            if raw_uuid(&raw) != o.uuid {
                refresh_raw_uuids(&mut raw);
                raw.set_attr("UUID", &o.uuid);
            }
            Ok(DisplayElement::Other(raw))
        }
        (Element::Image(i), DisplayElement::Other(raw), Element::Image(bi)) => {
            let mut raw = raw.clone();
            if raw_uuid(&raw) != i.uuid {
                refresh_raw_uuids(&mut raw);
                raw.set_attr("UUID", &i.uuid);
            }
            export_image(i, bi, &mut raw);
            Ok(DisplayElement::Other(raw))
        }
        (Element::Text(t), DisplayElement::Text(pt), Element::Text(bt)) => {
            let mut pt = pt.clone();
            if pt.uuid != t.uuid {
                refresh_raw_uuids(&mut pt.extra.element);
                pt.uuid.clone_from(&t.uuid);
            }
            export_text(t, bt, &mut pt, path)?;
            Ok(DisplayElement::Text(pt))
        }
        _ => Err(ExportError::NoOriginal {
            what: "slide element",
            uuid: e.uuid().to_owned(),
        }),
    }
}

fn export_text(
    t: &TextBox,
    base: &TextBox,
    pt: &mut pro6::TextElement,
    path: &Path,
) -> Result<(), ExportError> {
    pt.display_name.clone_from(&t.name);
    let r = &mut pt.position.value;
    r.x = t.frame.x;
    r.y = t.frame.y;
    r.width = t.frame.width;
    r.height = t.frame.height;
    pt.rotation = t.rotation;
    pt.opacity = t.opacity;
    pt.locked = t.locked;
    pt.bezel_radius = t.corner_radius;
    if t.valign != base.valign {
        pt.vertical_alignment = to_p_valign(t.valign);
    }
    if t.fill != base.fill {
        match t.fill {
            Some(c) => {
                pt.drawing_fill = true;
                pt.fill_color = Some(to_p(c));
            }
            None => pt.drawing_fill = false,
        }
    }
    // Outline and shadow changes can't be written to .pro6 yet (unverified); they
    // only exist in native documents, whose export leaves them out. Turning one
    // off is just the drawing flag.
    if t.outline.is_none() && base.outline.is_some() {
        pt.drawing_stroke = false;
    }
    if t.shadow.is_none() && base.shadow.is_some() {
        pt.drawing_shadow = false;
    }
    if t.paragraphs != base.paragraphs {
        let text_err = |source| ExportError::Text {
            uuid: t.uuid.clone(),
            source,
        };
        if t.runs().any(|r| r.font.is_empty()) {
            return Err(text_err(pro6::EditError::NoStyle {
                uuid: t.uuid.clone(),
            }));
        }
        let paragraphs = t
            .paragraphs
            .iter()
            .map(|p| pro6::rtf::Paragraph {
                align: to_p_align(p.align),
                runs: p
                    .runs
                    .iter()
                    .map(|r| pro6::rtf::Run {
                        text: r.text.clone(),
                        font: Some(r.font.clone()),
                        font_index: 0,
                        size_half_pt: (r.size * 2.0).round().max(1.0) as u32,
                        color: r.color.map(|[r, g, b]| pro6::rtf::Rgb { r, g, b }),
                        bold: r.bold,
                        italic: r.italic,
                        underline: r.underline,
                    })
                    .collect(),
            })
            .collect();
        pt.set_rich_text(paragraphs, WinPayloads::Drop, path)
            .map_err(text_err)?;
    }
    Ok(())
}

fn export_arrangements(
    doc: &Document,
    base: &Document,
    out: &mut pro6::Document,
) -> Result<(), ExportError> {
    let err = |uuid: &str| {
        let uuid = uuid.to_owned();
        move |source| ExportError::Arrangement { uuid, source }
    };
    let find = |list: &[Arrangement], uuid: &str| -> Option<usize> {
        list.iter().position(|a| a.uuid.eq_ignore_ascii_case(uuid))
    };
    for b in &base.arrangements {
        if find(&doc.arrangements, &b.uuid).is_none() {
            Op::RemoveArrangement {
                uuid: b.uuid.clone(),
            }
            .apply(out)
            .map_err(err(&b.uuid))?;
        }
    }
    // Group indices in the exported document (same order as the native groups).
    let indices = |a: &Arrangement| -> Vec<usize> {
        a.groups
            .iter()
            .filter_map(|id| {
                doc.groups
                    .iter()
                    .position(|g| g.uuid.eq_ignore_ascii_case(id))
            })
            .collect()
    };
    for a in &doc.arrangements {
        match find(&base.arrangements, &a.uuid).map(|i| &base.arrangements[i]) {
            Some(b) if b == a => {}
            Some(b) => {
                if b.name != a.name {
                    Op::RenameArrangement {
                        uuid: a.uuid.clone(),
                        name: a.name.clone(),
                    }
                    .apply(out)
                    .map_err(err(&a.uuid))?;
                }
                if b.groups != a.groups {
                    Op::SetArrangementGroups {
                        uuid: a.uuid.clone(),
                        groups: indices(a),
                    }
                    .apply(out)
                    .map_err(err(&a.uuid))?;
                }
                if b.color != a.color {
                    return Err(ExportError::Unsupported {
                        what: "A changed arrangement colour",
                    });
                }
            }
            None => Op::AddArrangement {
                uuid: a.uuid.clone(),
                name: a.name.clone(),
                groups: indices(a),
            }
            .apply(out)
            .map_err(err(&a.uuid))?,
        }
    }
    Ok(())
}

/// Copy of `s` with new UUIDs for it and its elements (`src` keeps pointing at the
/// originals, so the export copies their XML).
pub(crate) fn fresh_slide(s: &Slide) -> Slide {
    let mut c = s.clone();
    c.uuid = pro6::ops::new_uuid();
    for e in &mut c.elements {
        e.set_uuid(pro6::ops::new_uuid());
    }
    c
}
