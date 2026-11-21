//! The native presentation model. Everything the app shows, plays and edits is one
//! of these; `.pro6` files are converted on load (see `convert`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::compat::Mode;

/// Colour as four 0–1 components.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Rgba {
    pub fn to_rgba8(self) -> [u8; 4] {
        let q = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        [q(self.r), q(self.g), q(self.b), q(self.a)]
    }
}

/// Document canvas in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CanvasSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// The file this was loaded from or last saved to (`.pro6` or `.opres`).
    #[serde(skip)]
    pub path: PathBuf,
    pub format_version: u32,
    pub mode: Mode,
    pub canvas: CanvasSize,
    /// Background colour drawn behind every slide that has none of its own; `None` =
    /// not drawn (black, or the background media).
    pub background: Option<Rgba>,
    /// Arrangement chosen in the file.
    pub selected_arrangement: Option<String>,
    pub groups: Vec<Group>,
    pub arrangements: Vec<Arrangement>,
    /// Slide cues that aren't timers the app understands, and timers with a delay
    /// (meaning unverified). Read from `.pro6`; reported, not played.
    #[serde(default)]
    pub cue_report: CueReport,
    /// The `.pro6` this came from, kept so a `.pro6` save leaves untouched parts
    /// byte-identical (and keeps everything the native model doesn't describe).
    #[serde(default, rename = "pro6_source", with = "crate::file::pro6_source")]
    pub pro6: Option<Arc<pro6::Document>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CueReport {
    pub other_cues: usize,
    pub with_delay: usize,
    /// Timeline cues and media tracks (not played yet).
    #[serde(default)]
    pub timeline_cues: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub uuid: String,
    pub name: String,
    pub color: Option<Rgba>,
    pub slides: Vec<Slide>,
    /// UUID of the `.pro6` group this one was made from (itself, or the original of a copy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slide {
    pub uuid: String,
    pub enabled: bool,
    pub label: String,
    pub notes: String,
    pub hot_key: String,
    /// Slide background colour, when the slide draws one.
    pub background: Option<Rgba>,
    pub elements: Vec<Element>,
    /// Auto-advance timer.
    pub timer: Option<SlideTimer>,
    /// Uses something a `.pro6` file can't hold (the user accepted that).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pro6_incompatible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SlideTimer {
    #[serde(with = "secs")]
    pub duration: Duration,
    /// After this slide, go back to the first slide instead of the next.
    pub loop_to_beginning: bool,
}

mod secs {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(d.as_secs_f64())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let v = f64::deserialize(d)?;
        Duration::try_from_secs_f64(v).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Element {
    Text(Box<TextBox>),
    /// A `.pro6` element the native model doesn't describe yet (shape, media, ...).
    /// Kept in place and written back unchanged; not drawn.
    Pro6(Pro6Element),
}

impl Element {
    pub fn uuid(&self) -> &str {
        match self {
            Element::Text(t) => &t.uuid,
            Element::Pro6(p) => &p.uuid,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pro6Element {
    pub uuid: String,
    /// XML element name, e.g. `RVShapeElement`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
}

/// Element rectangle in canvas pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VAlign {
    Top,
    #[default]
    Middle,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justified,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextBox {
    pub uuid: String,
    pub name: String,
    pub frame: Rect,
    /// Degrees clockwise.
    pub rotation: f64,
    pub opacity: f64,
    pub locked: bool,
    pub valign: VAlign,
    /// Box fill, when drawn.
    pub fill: Option<Rgba>,
    pub corner_radius: f64,
    pub outline: Option<Outline>,
    pub shadow: Option<Shadow>,
    pub paragraphs: Vec<Paragraph>,
    /// Why the text can't be drawn as stored, if it can't (damaged RTF, a run whose
    /// font isn't in the RTF font table, no styled text at all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<TextProblem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
}

impl TextBox {
    /// The text, paragraphs joined with `\n`.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for (i, p) in self.paragraphs.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            for r in &p.runs {
                out.push_str(&r.text);
            }
        }
        out
    }

    pub fn runs(&self) -> impl Iterator<Item = &Run> {
        self.paragraphs.iter().flat_map(|p| &p.runs)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TextProblem {
    /// The stored RTF couldn't be read.
    Unreadable { message: String },
    /// A run uses a font index missing from the RTF font table.
    FontNotInTable { index: i32 },
    /// No `RTFData`: only plain text, without a font or size.
    NoStyledText,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Outline {
    pub width: f64,
    pub color: Option<Rgba>,
}

/// Drop shadow as `.pro6` stores it (blur | colour | offset); field meanings assumed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Shadow {
    pub blur: Option<f64>,
    pub color: Option<Rgba>,
    pub offset: Option<(f64, f64)>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Paragraph {
    pub align: Align,
    pub runs: Vec<Run>,
}

/// A stretch of text with one style. `\n` inside `text` is a line break within the
/// paragraph; `\t` a tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub text: String,
    pub font: String,
    /// Canvas pixels (one RTF point = one canvas pixel).
    pub size: f64,
    /// `None` = default colour (black).
    pub color: Option<[u8; 3]>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// An arrangement: a sequence of groups (by UUID; groups may repeat).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Arrangement {
    pub uuid: String,
    pub name: String,
    pub color: Option<Rgba>,
    /// Group UUIDs in play order, including any that match no group.
    pub groups: Vec<String>,
}

/// An arrangement with its groups resolved to indices into `Document::groups`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedArrangement {
    pub uuid: String,
    pub name: String,
    pub color: Option<Rgba>,
    pub groups: Vec<usize>,
    /// Group IDs listed by the arrangement that match no group.
    pub unknown_group_ids: Vec<String>,
}

/// Slide timers of a document (see `Document::slide_timings`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SlideTimings {
    /// Slide UUID → its timer.
    pub timers: std::collections::HashMap<String, SlideTimer>,
    pub other_cues: usize,
    pub with_delay: usize,
}

impl Document {
    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    /// All slides in natural (group) order.
    pub fn slides(&self) -> impl Iterator<Item = &Slide> {
        self.groups.iter().flat_map(|g| &g.slides)
    }

    pub fn text_boxes(&self) -> impl Iterator<Item = &TextBox> {
        self.slides().flat_map(|s| s.text_boxes())
    }

    /// Arrangements with their groups resolved.
    pub fn arrangements(&self) -> Vec<ResolvedArrangement> {
        self.arrangements.iter().map(|a| self.resolve(a)).collect()
    }

    pub fn resolve(&self, a: &Arrangement) -> ResolvedArrangement {
        let mut groups = Vec::new();
        let mut unknown_group_ids = Vec::new();
        for id in &a.groups {
            match self
                .groups
                .iter()
                .position(|g| g.uuid.eq_ignore_ascii_case(id))
            {
                Some(i) => groups.push(i),
                None => unknown_group_ids.push(id.clone()),
            }
        }
        ResolvedArrangement {
            uuid: a.uuid.clone(),
            name: a.name.clone(),
            color: a.color,
            groups,
            unknown_group_ids,
        }
    }

    /// The arrangement `selected_arrangement` points at, if it exists.
    pub fn selected_arrangement(&self) -> Option<ResolvedArrangement> {
        let id = self.selected_arrangement.as_deref()?;
        self.arrangements
            .iter()
            .find(|a| a.uuid.eq_ignore_ascii_case(id))
            .map(|a| self.resolve(a))
    }

    /// Slide timers, if any slide has one.
    pub fn slide_timings(&self) -> Option<SlideTimings> {
        let timers: std::collections::HashMap<_, _> = self
            .slides()
            .filter_map(|s| s.timer.map(|t| (s.uuid.clone(), t)))
            .collect();
        (!timers.is_empty()).then_some(SlideTimings {
            timers,
            other_cues: self.cue_report.other_cues,
            with_delay: self.cue_report.with_delay,
        })
    }

    /// Slides flagged as not `.pro6`-compatible.
    pub fn incompatible_slides(&self) -> impl Iterator<Item = (&Group, &Slide)> {
        self.groups
            .iter()
            .flat_map(|g| g.slides.iter().map(move |s| (g, s)))
            .filter(|(_, s)| s.pro6_incompatible)
    }
}

impl Group {
    pub fn slides(&self) -> &[Slide] {
        &self.slides
    }
}

impl Slide {
    pub fn text_boxes(&self) -> impl Iterator<Item = &TextBox> {
        self.elements.iter().filter_map(|e| match e {
            Element::Text(t) => Some(&**t),
            Element::Pro6(_) => None,
        })
    }

    /// Text of every box that has some, boxes separated by newlines.
    pub fn plain_text(&self) -> String {
        self.text_boxes()
            .map(TextBox::plain_text)
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}
