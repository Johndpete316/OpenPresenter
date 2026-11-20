//! Typed `.pro6` document model built from the lossless raw tree.

use std::path::{Path, PathBuf};

use base64::Engine as _;

use crate::error::{Ctx, ParseError, ParseErrorKind};
use crate::extra::{
    Claimed, Extra, ITEM, Leaf, TypedArray, Warning, attr_bool, attr_f64, attr_i64, attr_parse,
    attr_str, attr_string, keep_all, leaf_text, parse_f64, split, take,
};
use crate::raw::{RawDocument, RawElement, RawNode};

pub const ROOT_ELEMENT: &str = "RVPresentationDocument";

/// Colour as four 0–1 components, from PP6's `"r g b a"` strings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Rgba {
    pub fn parse(s: &str) -> Option<Rgba> {
        let mut it = s.split_whitespace().map(parse_f64);
        let c = Rgba {
            r: it.next()??,
            g: it.next()??,
            b: it.next()??,
            a: it.next()??,
        };
        it.next().is_none().then_some(c)
    }
}

/// Document canvas in pixels, from `RVPresentationDocument@width/@height`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanvasSize {
    pub width: u32,
    pub height: u32,
}

/// What surrounds the root element in the file.
#[derive(Debug, Clone, PartialEq)]
pub struct DocShell {
    pub has_bom: bool,
    pub prolog: Vec<RawNode>,
    pub epilog: Vec<RawNode>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub path: PathBuf,
    pub shell: DocShell,
    pub canvas: CanvasSize,
    /// Empty in the file → `None`.
    pub selected_arrangement_id: Option<String>,
    pub background_color: Option<Rgba>,
    pub category: String,
    pub notes: String,
    pub ccli_song_title: String,
    pub version_number: Option<String>,
    pub build_number: Option<String>,
    pub os: Option<String>,
    pub timeline: Option<Timeline>,
    pub groups: Option<TypedArray<SlideGroup>>,
    /// Non-fatal findings: unexpected structure, assumed formats that didn't match.
    pub warnings: Vec<Warning>,
    pub extra: Extra,
}

impl Document {
    pub fn groups(&self) -> &[SlideGroup] {
        self.groups.as_ref().map_or(&[], |g| &g.items)
    }

    /// `arrangements` array as raw XML (see `Document::arrangements` for the parsed view).
    pub fn arrangements_raw(&self) -> Option<&RawElement> {
        self.extra.find_ivar("arrangements")
    }

    pub fn slides(&self) -> impl Iterator<Item = &Slide> {
        self.groups().iter().flat_map(|g| g.slides())
    }

    pub fn text_elements(&self) -> impl Iterator<Item = &TextElement> {
        self.slides().flat_map(|s| s.text_elements())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub duration: f64,
    pub loop_: bool,
    pub time_offset: f64,
    pub selected_media_track_index: i64,
    /// `timeCues` and `mediaTracks` stay raw here until M8.
    pub extra: Extra,
}

impl Timeline {
    pub fn time_cue_count(&self) -> usize {
        element_count(self.extra.find_ivar("timeCues"))
    }
    pub fn media_track_count(&self) -> usize {
        element_count(self.extra.find_ivar("mediaTracks"))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SlideGroup {
    pub name: String,
    pub color: Option<Rgba>,
    pub uuid: String,
    pub slides: Option<TypedArray<Slide>>,
    pub extra: Extra,
}

impl SlideGroup {
    pub fn slides(&self) -> &[Slide] {
        self.slides.as_ref().map_or(&[], |s| &s.items)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Slide {
    pub uuid: String,
    pub enabled: bool,
    pub label: String,
    pub notes: String,
    pub hot_key: String,
    pub background_color: Option<Rgba>,
    pub highlight_color: Option<Rgba>,
    pub chord_chart_path: String,
    pub elements: Option<TypedArray<DisplayElement>>,
    /// `cues` stays raw here until M8.
    pub extra: Extra,
}

impl Slide {
    pub fn elements(&self) -> &[DisplayElement] {
        self.elements.as_ref().map_or(&[], |e| &e.items)
    }
    pub fn text_elements(&self) -> impl Iterator<Item = &TextElement> {
        self.elements().iter().filter_map(|e| match e {
            DisplayElement::Text(t) => Some(&**t),
            DisplayElement::Other(_) => None,
        })
    }
    pub fn cues_raw(&self) -> Option<&RawElement> {
        self.extra.find_ivar("cues")
    }
    pub fn cue_count(&self) -> usize {
        element_count(self.cues_raw())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DisplayElement {
    Text(Box<TextElement>),
    /// Any element type not modelled yet (media, shapes, ...), kept verbatim.
    Other(RawElement),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextElement {
    /// Element path in the file, for error messages.
    pub xml_path: String,
    pub uuid: String,
    pub display_name: String,
    pub opacity: f64,
    pub fill_color: Option<Rgba>,
    pub locked: bool,
    pub rotation: f64,
    pub bezel_radius: f64,
    pub vertical_alignment: i64,
    pub reveal_type: i64,
    pub adjusts_height_to_fit: bool,
    pub drawing_fill: bool,
    pub drawing_shadow: bool,
    pub drawing_stroke: bool,
    pub position: Leaf<Rect3D>,
    pub shadow: Option<Leaf<Shadow>>,
    pub stroke: Option<Stroke>,
    pub plain_text: Option<Leaf<Payload>>,
    /// Primary text source.
    pub rtf: Option<Leaf<Payload>>,
    pub win_flow: Option<Leaf<Payload>>,
    pub win_font: Option<Leaf<Payload>>,
    pub extra: Extra,
}

impl TextElement {
    /// Parse the `RTFData` payload. `Ok(None)` if the element has no RTF.
    /// `file` is used only for error context.
    pub fn rtf_doc(&self, file: &Path) -> Result<Option<crate::rtf::RtfDoc>, ParseError> {
        let Some(rtf) = &self.rtf else {
            return Ok(None);
        };
        crate::rtf::parse(&rtf.value.bytes).map(Some).map_err(|e| {
            Ctx {
                file,
                path: self.xml_path.clone(),
                uuid: Some(self.uuid.clone()).filter(|u| !u.is_empty()),
            }
            .err(ParseErrorKind::Rtf {
                message: e.to_string(),
            })
        })
    }
}

/// `RVRect3D` text `{x y z w h}`. Field order is assumed (see docs/PLAN.md Unknowns).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect3D {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect3D {
    pub fn parse(s: &str) -> Option<Rect3D> {
        let inner = s.trim().strip_prefix('{')?.strip_suffix('}')?;
        let mut it = inner.split_whitespace().map(parse_f64);
        let r = Rect3D {
            x: it.next()??,
            y: it.next()??,
            z: it.next()??,
            width: it.next()??,
            height: it.next()??,
        };
        it.next().is_none().then_some(r)
    }
}

/// Pipe-delimited shadow, e.g. `4|0 0 0 1|{2.82843, -2.82843}`.
/// Field meanings are **assumed** (blur | colour | offset); `fields` always holds the raw split.
#[derive(Debug, Clone, PartialEq)]
pub struct Shadow {
    pub fields: Vec<String>,
    pub blur: Option<f64>,
    pub color: Option<Rgba>,
    pub offset: Option<(f64, f64)>,
}

impl Shadow {
    /// Best-effort parse; returns whether the assumed 3-field layout matched.
    pub fn parse(s: &str) -> (Shadow, bool) {
        let fields: Vec<String> = s.split('|').map(str::to_owned).collect();
        let blur = fields.first().and_then(|f| parse_f64(f.trim()));
        let color = fields.get(1).and_then(|f| Rgba::parse(f));
        let offset = fields.get(2).and_then(|f| {
            let inner = f.trim().strip_prefix('{')?.strip_suffix('}')?;
            let (x, y) = inner.split_once(',')?;
            Some((parse_f64(x.trim())?, parse_f64(y.trim())?))
        });
        let matched = fields.len() == 3 && blur.is_some() && color.is_some() && offset.is_some();
        (
            Shadow {
                fields,
                blur,
                color,
                offset,
            },
            matched,
        )
    }
}

/// `dictionary rvXMLIvarName="stroke"`.
#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    pub color: Option<Leaf<Rgba>>,
    pub width: Option<Leaf<f64>>,
    pub extra: Extra,
}

/// Decoded base64 `NSString` payload.
#[derive(Debug, Clone, PartialEq)]
pub struct Payload {
    pub bytes: Vec<u8>,
}

impl Payload {
    /// Payload as text (lossy for invalid UTF-8).
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

fn element_count(el: Option<&RawElement>) -> usize {
    el.map_or(0, |e| e.elements().count())
}

// ---- parsing --------------------------------------------------------------------

struct P<'w> {
    warnings: &'w mut Vec<Warning>,
}

impl P<'_> {
    fn warn(&mut self, ctx: &Ctx<'_>, message: impl Into<String>) {
        let message = message.into();
        tracing::debug!(file = %ctx.file.display(), xml_path = %ctx.path, %message, "pro6 warning");
        self.warnings.push(Warning {
            xml_path: ctx.path.clone(),
            message,
        });
    }
}

fn is_ivar(el: &RawElement, name: &str, ivar: &str) -> bool {
    el.name == name && el.ivar_name() == Some(ivar)
}

pub(crate) fn document_from_raw(raw: RawDocument, file: &Path) -> Result<Document, ParseError> {
    let RawDocument {
        has_bom,
        prolog,
        root,
        epilog,
    } = raw;
    let ctx = Ctx::root(file).child(ROOT_ELEMENT);
    if root.name != ROOT_ELEMENT {
        return Err(Ctx::root(file)
            .child(&root.name)
            .err(ParseErrorKind::NotPro6Root {
                found: root.name.clone(),
            }));
    }
    let mut warnings = Vec::new();
    let mut p = P {
        warnings: &mut warnings,
    };

    let dim = |name: &'static str| -> Result<u32, ParseError> {
        let v = attr_parse(
            &root,
            &ctx,
            name,
            "a positive whole number of pixels",
            |s| {
                parse_f64(s)
                    .filter(|v| *v >= 1.0 && v.fract() == 0.0 && *v <= f64::from(u32::MAX))
                    .map(|v| v as u32)
            },
        )?;
        v.ok_or_else(|| ctx.err(ParseErrorKind::MissingAttr { attr: name }))
    };
    let canvas = CanvasSize {
        width: dim("width")?,
        height: dim("height")?,
    };

    let selected_arrangement_id =
        attr_str(&root, &ctx, "selectedArrangementID")?.filter(|s| !s.is_empty());
    let background_color = attr_parse(
        &root,
        &ctx,
        "backgroundColor",
        "an \"r g b a\" colour",
        Rgba::parse,
    )?;
    let category = attr_string(&root, &ctx, "category")?;
    let notes = attr_string(&root, &ctx, "notes")?;
    let ccli_song_title = attr_string(&root, &ctx, "CCLISongTitle")?;
    let version_number = attr_str(&root, &ctx, "versionNumber")?;
    let build_number = attr_str(&root, &ctx, "buildNumber")?;
    let os = attr_str(&root, &ctx, "os")?;

    let (extra, mut claimed) = split(root, &ctx, |c| {
        if c.name == "RVTimeline" {
            Some("timeline")
        } else if is_ivar(c, "array", "groups") {
            Some("groups")
        } else {
            None
        }
    });
    let timeline = take(&mut claimed, "timeline")
        .map(timeline_from)
        .transpose()?;
    let groups = match take(&mut claimed, "groups") {
        Some(c) => Some(array_from(c, &mut p, "RVSlideGrouping", group_from)?),
        None => {
            p.warn(&ctx, "no `groups` array; presentation has no slides");
            None
        }
    };
    for unknown in extra.raw_children() {
        if !matches!(unknown.ivar_name(), Some("arrangements")) {
            p.warn(
                &ctx,
                format!("unmodelled child <{}> kept as raw XML", unknown.name),
            );
        }
    }

    Ok(Document {
        path: file.to_path_buf(),
        shell: DocShell {
            has_bom,
            prolog,
            epilog,
        },
        canvas,
        selected_arrangement_id,
        background_color,
        category,
        notes,
        ccli_song_title,
        version_number,
        build_number,
        os,
        timeline,
        groups,
        warnings,
        extra,
    })
}

/// Parse an `<array>` whose items should be `item_name`. Other items stay raw in the array's
/// `extra` (with a warning) so nothing is lost.
fn array_from<'a, T>(
    c: Claimed<'a>,
    p: &mut P<'_>,
    item_name: &'static str,
    item: fn(Claimed<'a>, &mut P<'_>) -> Result<T, ParseError>,
) -> Result<TypedArray<T>, ParseError> {
    let (extra, claimed) = split(c.el, &c.ctx, |e| (e.name == item_name).then_some(ITEM));
    for other in extra.raw_children() {
        p.warn(
            &c.ctx,
            format!(
                "unexpected <{}> in array (expected <{item_name}>), kept as raw XML",
                other.name
            ),
        );
    }
    let items = claimed
        .into_iter()
        .map(|c| item(c, p))
        .collect::<Result<_, _>>()?;
    Ok(TypedArray { items, extra })
}

fn timeline_from(c: Claimed<'_>) -> Result<Timeline, ParseError> {
    let (el, ctx) = (&c.el, &c.ctx);
    Ok(Timeline {
        duration: attr_f64(el, ctx, "duration", 0.0)?,
        loop_: attr_bool(el, ctx, "loop", false)?,
        time_offset: attr_f64(el, ctx, "timeOffset", 0.0)?,
        selected_media_track_index: attr_i64(el, ctx, "selectedMediaTrackIndex", -1)?,
        extra: keep_all(c.el),
    })
}

fn group_from(c: Claimed<'_>, p: &mut P<'_>) -> Result<SlideGroup, ParseError> {
    let (el, ctx) = (&c.el, &c.ctx);
    let name = attr_string(el, ctx, "name")?;
    let color = attr_parse(el, ctx, "color", "an \"r g b a\" colour", Rgba::parse)?;
    let uuid = attr_string(el, ctx, "uuid")?;
    if uuid.is_empty() {
        p.warn(ctx, format!("slide group '{name}' has no uuid"));
    }
    let (extra, mut claimed) = split(c.el, &c.ctx, |e| {
        is_ivar(e, "array", "slides").then_some("slides")
    });
    let slides = take(&mut claimed, "slides")
        .map(|s| array_from(s, p, "RVDisplaySlide", slide_from))
        .transpose()?;
    Ok(SlideGroup {
        name,
        color,
        uuid,
        slides,
        extra,
    })
}

fn slide_from(c: Claimed<'_>, p: &mut P<'_>) -> Result<Slide, ParseError> {
    let (el, ctx) = (&c.el, &c.ctx);
    let colour = "an \"r g b a\" colour";
    let uuid = attr_string(el, ctx, "UUID")?;
    let enabled = attr_bool(el, ctx, "enabled", true)?;
    let label = attr_string(el, ctx, "label")?;
    let notes = attr_string(el, ctx, "notes")?;
    let hot_key = attr_string(el, ctx, "hotKey")?;
    let background_color = attr_parse(el, ctx, "backgroundColor", colour, Rgba::parse)?;
    let highlight_color = attr_parse(el, ctx, "highlightColor", colour, Rgba::parse)?;
    let chord_chart_path = attr_string(el, ctx, "chordChartPath")?;
    let (extra, mut claimed) = split(c.el, &c.ctx, |e| {
        is_ivar(e, "array", "displayElements").then_some("displayElements")
    });
    let elements = match take(&mut claimed, "displayElements") {
        Some(arr) => {
            let (aextra, items) = split(arr.el, &arr.ctx, |_| Some(ITEM));
            let items = items
                .into_iter()
                .map(|i| {
                    if i.el.name == "RVTextElement" {
                        text_element_from(i, p).map(|t| DisplayElement::Text(Box::new(t)))
                    } else {
                        p.warn(
                            &i.ctx,
                            format!(
                                "display element <{}> not modelled yet, kept as raw XML",
                                i.el.name
                            ),
                        );
                        Ok(DisplayElement::Other(i.el))
                    }
                })
                .collect::<Result<_, _>>()?;
            Some(TypedArray {
                items,
                extra: aextra,
            })
        }
        None => None,
    };
    Ok(Slide {
        uuid,
        enabled,
        label,
        notes,
        hot_key,
        background_color,
        highlight_color,
        chord_chart_path,
        elements,
        extra,
    })
}

fn text_element_from(c: Claimed<'_>, p: &mut P<'_>) -> Result<TextElement, ParseError> {
    let (el, ctx) = (&c.el, &c.ctx);
    let colour = "an \"r g b a\" colour";
    let uuid = attr_string(el, ctx, "UUID")?;
    let display_name = attr_string(el, ctx, "displayName")?;
    let opacity = attr_f64(el, ctx, "opacity", 1.0)?;
    let fill_color = attr_parse(el, ctx, "fillColor", colour, Rgba::parse)?;
    let locked = attr_bool(el, ctx, "locked", false)?;
    let rotation = attr_f64(el, ctx, "rotation", 0.0)?;
    let bezel_radius = attr_f64(el, ctx, "bezelRadius", 0.0)?;
    let vertical_alignment = attr_i64(el, ctx, "verticalAlignment", 0)?;
    let reveal_type = attr_i64(el, ctx, "revealType", 0)?;
    let adjusts_height_to_fit = attr_bool(el, ctx, "adjustsHeightToFit", false)?;
    let drawing_fill = attr_bool(el, ctx, "drawingFill", false)?;
    let drawing_shadow = attr_bool(el, ctx, "drawingShadow", false)?;
    let drawing_stroke = attr_bool(el, ctx, "drawingStroke", false)?;

    let ctx_owned = c.ctx.clone();
    let (extra, mut claimed) = split(c.el, &c.ctx, |e| match (e.name.as_str(), e.ivar_name()) {
        ("RVRect3D", Some("position")) => Some("position"),
        ("shadow", Some("shadow")) => Some("shadow"),
        ("dictionary", Some("stroke")) => Some("stroke"),
        ("NSString", Some("PlainText")) => Some("PlainText"),
        ("NSString", Some("RTFData")) => Some("RTFData"),
        ("NSString", Some("WinFlowData")) => Some("WinFlowData"),
        ("NSString", Some("WinFontData")) => Some("WinFontData"),
        _ => None,
    });

    let position = take(&mut claimed, "position").ok_or_else(|| {
        ctx_owned.err(ParseErrorKind::MissingChild {
            element: "RVRect3D position".into(),
        })
    })?;
    let position = leaf(position, |s, ctx| {
        Rect3D::parse(s).ok_or_else(|| {
            ctx.err(ParseErrorKind::BadRect {
                value: s.to_owned(),
            })
        })
    })?;

    let shadow = take(&mut claimed, "shadow")
        .map(|s| {
            leaf(s, |text, ctx| {
                let (shadow, matched) = Shadow::parse(text);
                if !matched {
                    p.warn(ctx, format!("shadow {text:?} doesn't match assumed `blur|r g b a|{{x, y}}` layout; kept raw"));
                }
                Ok(shadow)
            })
        })
        .transpose()?;

    let stroke = take(&mut claimed, "stroke").map(stroke_from).transpose()?;

    let mut payload = |key: &'static str| -> Result<Option<Leaf<Payload>>, ParseError> {
        take(&mut claimed, key)
            .map(|c| leaf(c, |s, ctx| decode_b64(s, key, ctx)))
            .transpose()
    };
    let plain_text = payload("PlainText")?;
    let rtf = payload("RTFData")?;
    let win_flow = payload("WinFlowData")?;
    let win_font = payload("WinFontData")?;
    if rtf.is_none() {
        p.warn(
            &ctx_owned,
            "text element has no RTFData; only PlainText (if any) is available",
        );
    }

    Ok(TextElement {
        xml_path: ctx_owned.path.clone(),
        uuid,
        display_name,
        opacity,
        fill_color,
        locked,
        rotation,
        bezel_radius,
        vertical_alignment,
        reveal_type,
        adjusts_height_to_fit,
        drawing_fill,
        drawing_shadow,
        drawing_stroke,
        position,
        shadow,
        stroke,
        plain_text,
        rtf,
        win_flow,
        win_font,
        extra,
    })
}

fn stroke_from(c: Claimed<'_>) -> Result<Stroke, ParseError> {
    fn key(e: &RawElement) -> Option<&str> {
        e.attr("rvXMLDictionaryKey").map(|a| a.raw_value.as_str())
    }
    let (extra, mut claimed) = split(c.el, &c.ctx, |e| match (e.name.as_str(), key(e)) {
        ("NSColor", Some("RVShapeElementStrokeColorKey")) => Some("color"),
        ("NSNumber", Some("RVShapeElementStrokeWidthKey")) => Some("width"),
        _ => None,
    });
    let color = take(&mut claimed, "color")
        .map(|c| {
            leaf(c, |s, ctx| {
                Rgba::parse(s).ok_or_else(|| {
                    ctx.err(ParseErrorKind::BadAttr {
                        attr: "RVShapeElementStrokeColorKey".into(),
                        value: s.to_owned(),
                        expected: "an \"r g b a\" colour",
                    })
                })
            })
        })
        .transpose()?;
    let width = take(&mut claimed, "width")
        .map(|c| {
            leaf(c, |s, ctx| {
                parse_f64(s).ok_or_else(|| {
                    ctx.err(ParseErrorKind::BadAttr {
                        attr: "RVShapeElementStrokeWidthKey".into(),
                        value: s.to_owned(),
                        expected: "a number",
                    })
                })
            })
        })
        .transpose()?;
    Ok(Stroke {
        color,
        width,
        extra,
    })
}

fn leaf<T>(
    c: Claimed<'_>,
    parse: impl FnOnce(&str, &Ctx<'_>) -> Result<T, ParseError>,
) -> Result<Leaf<T>, ParseError> {
    let text = leaf_text(&c.el, &c.ctx)?;
    let value = parse(&text, &c.ctx)?;
    Ok(Leaf {
        value,
        extra: keep_all(c.el),
    })
}

fn decode_b64(s: &str, field: &str, ctx: &Ctx<'_>) -> Result<Payload, ParseError> {
    let compact: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    base64::engine::general_purpose::STANDARD_PAD_INDIFFERENT
        .decode(compact.as_bytes())
        .map(|bytes| Payload { bytes })
        .map_err(|e| {
            ctx.err(ParseErrorKind::Base64 {
                field: field.to_owned(),
                message: e.to_string(),
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_parses_five_numbers() {
        assert_eq!(
            Rect3D::parse("{56 42 0 1808 996}"),
            Some(Rect3D {
                x: 56.0,
                y: 42.0,
                z: 0.0,
                width: 1808.0,
                height: 996.0
            })
        );
        assert_eq!(Rect3D::parse("{56 42 0 1808}"), None);
        assert_eq!(Rect3D::parse("56 42 0 1808 996"), None);
    }

    #[test]
    fn shadow_spec_example() {
        let (s, ok) = Shadow::parse("4|0 0 0 1|{2.82843, -2.82843}");
        assert!(ok);
        assert_eq!(s.blur, Some(4.0));
        assert_eq!(s.offset, Some((2.82843, -2.82843)));
        let (s, ok) = Shadow::parse("weird");
        assert!(!ok);
        assert_eq!(s.fields, vec!["weird"]);
    }

    #[test]
    fn rgba() {
        assert_eq!(
            Rgba::parse("1 1 1 0"),
            Some(Rgba {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.0
            })
        );
        assert_eq!(Rgba::parse("1 1 1"), None);
        assert_eq!(Rgba::parse("1 1 1 1 1"), None);
    }
}
