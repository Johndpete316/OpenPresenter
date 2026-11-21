//! Typed model → raw tree → XML.
//!
//! Each typed node starts from its preserved original element (`Extra`). A typed
//! value that still equals what the original raw text parses to is left untouched,
//! so unchanged files are written back byte-for-byte. Only changed values are
//! regenerated. Typed children are re-inserted at their original positions.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;

use crate::extra::{Extra, ITEM, Leaf, TypedArray, parse_bool, parse_f64, parse_i64};
use crate::model::{
    DisplayElement, Document, Payload, Rect3D, Rgba, Shadow, Slide, SlideGroup, Stroke,
    TextElement, Timeline,
};
use crate::raw::{RawDocument, RawElement, RawNode, write_document};

/// Serialize a document to `.pro6` XML.
pub fn write_string(doc: &Document) -> String {
    write_document(&to_raw(doc))
}

/// Build the raw tree for a document.
pub fn to_raw(doc: &Document) -> RawDocument {
    let mut root = doc.extra.element.clone();
    {
        let mut a = Attrs::new(&mut root);
        a.u32("width", doc.canvas.width);
        a.u32("height", doc.canvas.height);
        a.opt_nonempty(
            "selectedArrangementID",
            doc.selected_arrangement_id.as_deref(),
        );
        a.color("backgroundColor", doc.background_color);
        a.string("category", &doc.category);
        a.string("notes", &doc.notes);
        a.string("CCLISongTitle", &doc.ccli_song_title);
        a.opt_raw("versionNumber", doc.version_number.as_deref());
        a.opt_raw("buildNumber", doc.build_number.as_deref());
        a.opt_raw("os", doc.os.as_deref());
        a.finish();
    }
    let mut typed = Vec::new();
    if let Some(t) = &doc.timeline {
        typed.push(("timeline", timeline(t)));
    }
    if let Some(g) = &doc.groups {
        typed.push(("groups", array(g, group)));
    }
    RawDocument {
        has_bom: doc.shell.has_bom,
        prolog: doc.shell.prolog.clone(),
        root: assemble(root, &doc.extra, typed),
        epilog: doc.shell.epilog.clone(),
    }
}

fn timeline(t: &Timeline) -> RawElement {
    let mut el = t.extra.element.clone();
    let mut a = Attrs::new(&mut el);
    a.f64("duration", t.duration, 0.0);
    a.bool("loop", t.loop_, false);
    a.f64("timeOffset", t.time_offset, 0.0);
    a.i64("selectedMediaTrackIndex", t.selected_media_track_index, -1);
    a.finish();
    assemble(el, &t.extra, Vec::new())
}

fn group(g: &SlideGroup) -> RawElement {
    let mut el = g.extra.element.clone();
    let mut a = Attrs::new(&mut el);
    a.string("name", &g.name);
    a.color("color", g.color);
    a.string("uuid", &g.uuid);
    a.finish();
    let typed = g
        .slides
        .as_ref()
        .map(|s| ("slides", array(s, slide)))
        .into_iter()
        .collect();
    assemble(el, &g.extra, typed)
}

fn slide(s: &Slide) -> RawElement {
    let mut el = s.extra.element.clone();
    let mut a = Attrs::new(&mut el);
    a.string("UUID", &s.uuid);
    a.bool("enabled", s.enabled, true);
    a.string("label", &s.label);
    a.string("notes", &s.notes);
    a.string("hotKey", &s.hot_key);
    a.color("backgroundColor", s.background_color);
    a.color("highlightColor", s.highlight_color);
    a.string("chordChartPath", &s.chord_chart_path);
    a.finish();
    let typed = s
        .elements
        .as_ref()
        .map(|e| {
            let el = array(e, |d| match d {
                DisplayElement::Text(t) => text_element(t),
                DisplayElement::Other(raw) => raw.clone(),
            });
            ("displayElements", el)
        })
        .into_iter()
        .collect();
    assemble(el, &s.extra, typed)
}

fn text_element(t: &TextElement) -> RawElement {
    let mut el = t.extra.element.clone();
    let mut a = Attrs::new(&mut el);
    a.string("UUID", &t.uuid);
    a.string("displayName", &t.display_name);
    a.f64("opacity", t.opacity, 1.0);
    a.color("fillColor", t.fill_color);
    a.bool("locked", t.locked, false);
    a.f64("rotation", t.rotation, 0.0);
    a.f64("bezelRadius", t.bezel_radius, 0.0);
    a.i64("verticalAlignment", t.vertical_alignment, 0);
    a.i64("revealType", t.reveal_type, 0);
    a.bool("adjustsHeightToFit", t.adjusts_height_to_fit, false);
    a.bool("drawingFill", t.drawing_fill, false);
    a.bool("drawingShadow", t.drawing_shadow, false);
    a.bool("drawingStroke", t.drawing_stroke, false);
    a.finish();

    let mut typed = vec![("position", leaf(&t.position, Rect3D::parse, fmt_rect))];
    if let Some(s) = &t.shadow {
        typed.push(("shadow", leaf(s, |s| Some(Shadow::parse(s).0), fmt_shadow)));
    }
    if let Some(s) = &t.stroke {
        typed.push(("stroke", stroke(s)));
    }
    for (key, p) in [
        ("PlainText", &t.plain_text),
        ("RTFData", &t.rtf),
        ("WinFlowData", &t.win_flow),
        ("WinFontData", &t.win_font),
    ] {
        if let Some(p) = p {
            typed.push((key, leaf(p, decode_payload, fmt_payload)));
        }
    }
    assemble(el, &t.extra, typed)
}

fn stroke(s: &Stroke) -> RawElement {
    let mut typed = Vec::new();
    if let Some(c) = &s.color {
        typed.push(("color", leaf(c, Rgba::parse, fmt_rgba)));
    }
    if let Some(w) = &s.width {
        typed.push(("width", leaf(w, parse_f64, |v| fmt_f64(*v))));
    }
    assemble(s.extra.element.clone(), &s.extra, typed)
}

fn array<T>(arr: &TypedArray<T>, item: impl Fn(&T) -> RawElement) -> RawElement {
    let typed = arr.items.iter().map(|i| (ITEM, item(i))).collect();
    assemble(arr.extra.element.clone(), &arr.extra, typed)
}

/// Leaf element: keep the original text if it still parses to `value`.
fn leaf<T: PartialEq>(
    leaf: &Leaf<T>,
    parse: impl Fn(&str) -> Option<T>,
    format: impl Fn(&T) -> String,
) -> RawElement {
    let mut el = leaf.extra.element.clone();
    let original = el.text().ok().and_then(|t| parse(t.trim()));
    if original.as_ref() != Some(&leaf.value) {
        el.set_text(&format(&leaf.value));
    }
    el
}

/// Re-insert typed children into `shell` (whose children are the raw, unclaimed ones)
/// at the positions recorded in `extra.typed_slots`.
///
/// - A slot whose typed child is gone (e.g. removed shadow) is dropped.
/// - Extra array items (added in an editor) go right after the last original item.
/// - New named children go at the end, before trailing whitespace.
fn assemble(
    mut shell: RawElement,
    extra: &Extra,
    typed: Vec<(&'static str, RawElement)>,
) -> RawElement {
    let kept = std::mem::take(&mut shell.children);
    let slots = &extra.typed_slots;
    let total = kept.len() + slots.len();
    let last_item_slot = slots
        .iter()
        .filter(|(_, k)| *k == ITEM)
        .map(|(i, _)| *i)
        .next_back();

    let mut items = Vec::new();
    let mut named: Vec<(&'static str, RawElement)> = Vec::new();
    for (k, el) in typed {
        if k == ITEM {
            items.push(el);
        } else {
            named.push((k, el));
        }
    }
    let mut items = items.into_iter();
    let mut kept = kept.into_iter();
    let mut slot_iter = slots.iter().peekable();
    let mut out = Vec::with_capacity(total + 1);

    for i in 0..total {
        if let Some(&&(_, key)) = slot_iter.peek().filter(|(idx, _)| *idx == i) {
            slot_iter.next();
            if key == ITEM {
                out.extend(items.next().map(RawNode::Element));
                if Some(i) == last_item_slot {
                    out.extend(items.by_ref().map(RawNode::Element));
                }
            } else if let Some(pos) = named.iter().position(|(k, _)| *k == key) {
                out.push(RawNode::Element(named.remove(pos).1));
            }
        } else if let Some(n) = kept.next() {
            out.push(n);
        }
    }
    out.extend(kept); // defensive: slots/kept mismatch can't drop data

    let mut tail: Vec<RawNode> = items.map(RawNode::Element).collect();
    tail.extend(named.into_iter().map(|(_, e)| RawNode::Element(e)));
    if !tail.is_empty() {
        let at = match out.last() {
            Some(RawNode::Text(t)) if t.trim().is_empty() => out.len() - 1,
            _ => out.len(),
        };
        out.splice(at..at, tail);
    }
    if !out.is_empty() {
        shell.self_closing = false;
    }
    shell.children = out;
    shell
}

/// Attribute sync: write a typed value only if it differs from the parsed original.
struct Attrs<'a> {
    el: &'a mut RawElement,
    dirty: bool,
}

impl<'a> Attrs<'a> {
    fn new(el: &'a mut RawElement) -> Self {
        Attrs { el, dirty: false }
    }

    fn current(&self, name: &str) -> Option<String> {
        self.el.attr(name).map(|a| {
            a.value()
                .map_or_else(|_| a.raw_value.clone(), |v| v.into_owned())
        })
    }

    /// Current value, treating missing and blank as absent (matches `attr_parse`).
    fn current_parsed<T>(&self, name: &str, parse: fn(&str) -> Option<T>) -> Option<T> {
        self.current(name)
            .filter(|v| !v.trim().is_empty())
            .and_then(|v| parse(v.trim()))
    }

    fn set(&mut self, name: &str, value: &str) {
        let raw_value = quick_xml::escape::escape(value).into_owned();
        match self.el.attrs.iter_mut().find(|a| a.name == name) {
            Some(a) => a.raw_value = raw_value,
            None => self.el.attrs.push(crate::raw::RawAttr {
                name: name.to_owned(),
                raw_value,
            }),
        }
        self.dirty = true;
    }

    fn remove(&mut self, name: &str) {
        let before = self.el.attrs.len();
        self.el.attrs.retain(|a| a.name != name);
        self.dirty |= self.el.attrs.len() != before;
    }

    fn string(&mut self, name: &str, v: &str) {
        if self.current(name).unwrap_or_default() != v {
            self.set(name, v);
        }
    }

    /// `None` ↔ missing or empty.
    fn opt_nonempty(&mut self, name: &str, v: Option<&str>) {
        if self.current(name).filter(|c| !c.is_empty()).as_deref() != v {
            self.set(name, v.unwrap_or(""));
        }
    }

    /// `None` ↔ missing; `Some("")` ↔ present but empty.
    fn opt_raw(&mut self, name: &str, v: Option<&str>) {
        if self.current(name).as_deref() != v {
            match v {
                Some(v) => self.set(name, v),
                None => self.remove(name),
            }
        }
    }

    fn bool(&mut self, name: &str, v: bool, default: bool) {
        if self.current_parsed(name, parse_bool).unwrap_or(default) != v {
            self.set(name, if v { "true" } else { "false" });
        }
    }

    fn f64(&mut self, name: &str, v: f64, default: f64) {
        if self.current_parsed(name, parse_f64).unwrap_or(default) != v {
            self.set(name, &fmt_f64(v));
        }
    }

    fn i64(&mut self, name: &str, v: i64, default: i64) {
        if self.current_parsed(name, parse_i64).unwrap_or(default) != v {
            self.set(name, &v.to_string());
        }
    }

    fn u32(&mut self, name: &str, v: u32) {
        if self.current_parsed(name, parse_f64) != Some(f64::from(v)) {
            self.set(name, &v.to_string());
        }
    }

    fn color(&mut self, name: &str, v: Option<Rgba>) {
        if self.current_parsed(name, Rgba::parse) != v {
            self.set(name, &v.as_ref().map(fmt_rgba).unwrap_or_default());
        }
    }

    fn finish(self) {
        if self.dirty {
            self.el.regenerate_attrs_src();
        }
    }
}

// ---- value formatting -------------------------------------------------------------

/// Shortest representation that parses back to the same `f64` (`1`, `0.5`, `0.3333333432674408`).
pub fn fmt_f64(v: f64) -> String {
    let s = format!("{v}");
    if s == "-0" { "0".to_owned() } else { s }
}

fn fmt_rgba(c: &Rgba) -> String {
    format!(
        "{} {} {} {}",
        fmt_f64(c.r),
        fmt_f64(c.g),
        fmt_f64(c.b),
        fmt_f64(c.a)
    )
}

fn fmt_rect(r: &Rect3D) -> String {
    format!(
        "{{{} {} {} {} {}}}",
        fmt_f64(r.x),
        fmt_f64(r.y),
        fmt_f64(r.z),
        fmt_f64(r.width),
        fmt_f64(r.height)
    )
}

fn fmt_shadow(s: &Shadow) -> String {
    match (s.blur, s.color, s.offset) {
        (Some(b), Some(c), Some((x, y))) => {
            format!(
                "{}|{}|{{{}, {}}}",
                fmt_f64(b),
                fmt_rgba(&c),
                fmt_f64(x),
                fmt_f64(y)
            )
        }
        // Layout not understood: write the raw fields back as they were.
        _ => s.fields.join("|"),
    }
}

fn decode_payload(s: &str) -> Option<Payload> {
    let compact: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    base64::engine::general_purpose::STANDARD_PAD_INDIFFERENT
        .decode(compact.as_bytes())
        .ok()
        .map(|bytes| Payload { bytes })
}

fn fmt_payload(p: &Payload) -> String {
    base64::engine::general_purpose::STANDARD.encode(&p.bytes)
}

// ---- saving -----------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
#[error("{path}: couldn't {op}: {source}", path = .path.display())]
pub struct WriteError {
    pub path: PathBuf,
    pub op: &'static str,
    #[source]
    pub source: std::io::Error,
}

impl WriteError {
    pub fn user_message(&self) -> String {
        let name = self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let why = match self.source.kind() {
            std::io::ErrorKind::PermissionDenied => {
                "this PC isn't allowed to write there (permission denied)".to_owned()
            }
            std::io::ErrorKind::StorageFull => "the disk is full".to_owned(),
            std::io::ErrorKind::NotFound => "the folder no longer exists".to_owned(),
            _ => format!("the file couldn't be written ({})", self.source),
        };
        format!(
            "'{name}' couldn't be saved: {why}. Your changes are not saved; the original file is unchanged."
        )
    }
}

/// Write `doc` to `path` atomically: write a temporary file next to it, flush it to disk,
/// then rename over the target. A crash mid-save leaves the original intact.
pub fn save(doc: &Document, path: &Path) -> Result<(), WriteError> {
    let _span = tracing::info_span!("save_pro6", path = %path.display()).entered();
    let xml = write_string(doc);
    let tmp = tmp_path(path);
    let err = |op, source| {
        let e = WriteError {
            path: path.to_path_buf(),
            op,
            source,
        };
        tracing::error!(error = %e, "pro6 save failed");
        e
    };
    let result = (|| {
        let mut f = std::fs::File::create(&tmp).map_err(|e| err("create a temporary file", e))?;
        f.write_all(xml.as_bytes())
            .map_err(|e| err("write the file", e))?;
        f.sync_all().map_err(|e| err("flush the file to disk", e))?;
        drop(f);
        std::fs::rename(&tmp, path).map_err(|e| err("replace the original file", e))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    } else {
        tracing::info!(bytes = xml.len(), "saved");
    }
    result
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f64_format_round_trips() {
        for v in [
            0.0,
            1.0,
            0.5,
            0.3333333432674408,
            2.82843,
            -2.82843,
            1808.0,
            -0.0,
        ] {
            assert_eq!(fmt_f64(v).parse::<f64>().unwrap(), v);
        }
        assert_eq!(fmt_f64(1.0), "1");
        assert_eq!(fmt_f64(-0.0), "0");
    }

    #[test]
    fn shadow_and_rect_format() {
        let (s, _) = Shadow::parse("4|0 0 0 1|{2.82843, -2.82843}");
        assert_eq!(fmt_shadow(&s), "4|0 0 0 1|{2.82843, -2.82843}");
        let r = Rect3D::parse("{56 42 0 1808 996}").unwrap();
        assert_eq!(fmt_rect(&r), "{56 42 0 1808 996}");
    }
}
