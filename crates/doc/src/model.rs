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
    /// Set when this document is a theme being edited (saved as its `.optheme`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<crate::theme::ThemeLink>,
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
    /// Image filling the canvas behind every element (over the background colour).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_image: Option<BackgroundImage>,
    pub elements: Vec<Element>,
    /// Video shown full screen under the slide's elements while the slide is live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<SlideVideo>,
    /// Auto-advance timer.
    pub timer: Option<SlideTimer>,
    /// Uses something a `.pro6` file can't hold (the user accepted that).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pro6_incompatible: bool,
    /// Theme this slide follows (native documents only): editing the theme can
    /// restyle it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<crate::theme::ThemeLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
}

/// A slide's video: plays full screen under the slide's elements (the background
/// layer), as a media cue does in ProPresenter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlideVideo {
    pub media: MediaRef,
    #[serde(default)]
    pub start: VideoStart,
    #[serde(default)]
    pub end: VideoEnd,
    /// 0 … 1.
    #[serde(default = "one")]
    pub volume: f64,
    #[serde(default)]
    pub fit: Fit,
    #[serde(default, skip_serializing_if = "VideoAdjust::is_default")]
    pub adjust: VideoAdjust,
}

impl SlideVideo {
    pub fn new(media: MediaRef) -> SlideVideo {
        SlideVideo {
            media,
            start: VideoStart::default(),
            end: VideoEnd::default(),
            volume: 1.0,
            fit: Fit::default(),
            adjust: VideoAdjust::default(),
        }
    }
}

/// When a slide's video starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoStart {
    /// As soon as the slide is shown.
    #[default]
    OnShow,
    /// Shown paused on its first frame until the operator presses Play.
    OnClick,
}

/// What happens when a slide's video reaches its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoEnd {
    /// The video goes away (back to black / nothing under the slide).
    #[default]
    Stop,
    /// The last frame stays on screen.
    Hold,
    /// Starts again from the beginning.
    Loop,
    /// The next slide is shown.
    Next,
}

/// Picture settings for a video, each −1 … 1 (0 = unchanged).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoAdjust {
    pub brightness: f64,
    pub contrast: f64,
    pub saturation: f64,
    pub gamma: f64,
    pub hue: f64,
}

impl VideoAdjust {
    pub fn is_default(&self) -> bool {
        *self == VideoAdjust::default()
    }
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
    Image(Box<ImageBox>),
    /// A `.pro6` element the native model doesn't describe yet (shape, media, ...).
    /// Kept in place and written back unchanged; not drawn.
    Pro6(Pro6Element),
}

impl Element {
    pub fn uuid(&self) -> &str {
        match self {
            Element::Text(t) => &t.uuid,
            Element::Image(i) => &i.uuid,
            Element::Pro6(p) => &p.uuid,
        }
    }

    pub fn set_uuid(&mut self, uuid: String) {
        match self {
            Element::Text(t) => t.uuid = uuid,
            Element::Image(i) => i.uuid = uuid,
            Element::Pro6(p) => p.uuid = uuid,
        }
    }

    pub fn src(&self) -> Option<&str> {
        match self {
            Element::Text(t) => t.src.as_deref(),
            Element::Image(i) => i.src.as_deref(),
            Element::Pro6(p) => p.src.as_deref(),
        }
    }

    /// Position and size, for elements the editor can place.
    pub fn frame(&self) -> Option<Rect> {
        match self {
            Element::Text(t) => Some(t.frame),
            Element::Image(i) => Some(i.frame),
            Element::Pro6(_) => None,
        }
    }

    pub fn rotation(&self) -> f64 {
        match self {
            Element::Text(t) => t.rotation,
            Element::Image(i) => i.rotation,
            Element::Pro6(_) => 0.0,
        }
    }

    pub fn locked(&self) -> bool {
        match self {
            Element::Text(t) => t.locked,
            Element::Image(i) => i.locked,
            Element::Pro6(_) => true,
        }
    }

    pub fn hidden(&self) -> bool {
        match self {
            Element::Text(t) => t.hidden,
            Element::Image(i) => i.hidden,
            Element::Pro6(_) => false,
        }
    }

    /// Name shown in the Layers list.
    pub fn name(&self) -> &str {
        match self {
            Element::Text(t) => &t.name,
            Element::Image(i) => &i.name,
            Element::Pro6(p) => &p.kind,
        }
    }

    pub fn as_text(&self) -> Option<&TextBox> {
        match self {
            Element::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn as_image(&self) -> Option<&ImageBox> {
        match self {
            Element::Image(i) => Some(i),
            _ => None,
        }
    }
}

/// A media file. `path` is relative to the document's folder (forward slashes), or
/// absolute (images from a `.pro6`, which stores absolute paths).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MediaRef {
    pub path: String,
    /// SHA-256 (hex) of the file when it was added, so a changed file is noticed.
    /// `None` for images that came from a `.pro6`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

impl MediaRef {
    /// Where the file is, for a document at `doc_path`.
    pub fn resolve(&self, doc_path: &std::path::Path) -> PathBuf {
        let p = std::path::Path::new(&self.path);
        if p.is_absolute() || is_windows_absolute(&self.path) {
            return p.to_path_buf();
        }
        let base = doc_path.parent().unwrap_or(std::path::Path::new(""));
        base.join(self.path.replace('\\', "/"))
    }

    /// The file name, for messages.
    pub fn file_name(&self) -> &str {
        self.path.rsplit(['/', '\\']).next().unwrap_or(&self.path)
    }
}

fn is_windows_absolute(p: &str) -> bool {
    let b = p.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
        || p.starts_with("\\\\")
}

/// Non-destructive image adjustments, applied when drawing. Each is 0 for "none".
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Adjust {
    /// −1 … 1.
    pub brightness: f64,
    /// −1 … 1.
    pub contrast: f64,
    /// −1 (grey) … 1.
    pub saturation: f64,
    /// −1 (cool / blue) … 1 (warm / orange).
    pub temperature: f64,
    /// −1 (green) … 1 (magenta).
    pub tint: f64,
    /// Darkening towards the edges, 0 … 1.
    pub vignette: f64,
    /// How far in the vignette reaches, 0 (edges only) … 1 (to the centre).
    pub vignette_size: f64,
}

impl Default for Adjust {
    fn default() -> Self {
        Adjust {
            brightness: 0.0,
            contrast: 0.0,
            saturation: 0.0,
            temperature: 0.0,
            tint: 0.0,
            vignette: 0.0,
            vignette_size: 0.5,
        }
    }
}

impl Adjust {
    pub fn is_none(&self) -> bool {
        !self.has_color() && !self.has_vignette()
    }

    /// Brightness, contrast, saturation, temperature or tint is set.
    pub fn has_color(&self) -> bool {
        [
            self.brightness,
            self.contrast,
            self.saturation,
            self.temperature,
            self.tint,
        ]
        .iter()
        .any(|v| *v != 0.0)
    }

    pub fn has_vignette(&self) -> bool {
        self.vignette != 0.0
    }

    fn is_default(&self) -> bool {
        *self == Adjust::default()
    }
}

/// An image on the slide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageBox {
    pub uuid: String,
    pub name: String,
    pub frame: Rect,
    /// Degrees clockwise.
    pub rotation: f64,
    pub opacity: f64,
    pub locked: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    #[serde(default)]
    pub flip_h: bool,
    #[serde(default)]
    pub flip_v: bool,
    pub media: MediaRef,
    #[serde(default, skip_serializing_if = "Adjust::is_default")]
    pub adjust: Adjust,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
}

/// How a background image fills the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    /// The whole picture, bars where the shapes differ.
    Fit,
    /// Cover the canvas, cropping what doesn't fit.
    #[default]
    Fill,
    /// Stretch to the canvas shape.
    Stretch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundImage {
    pub media: MediaRef,
    pub fit: Fit,
    #[serde(default, skip_serializing_if = "Adjust::is_default")]
    pub adjust: Adjust,
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
    /// Hidden in the editor's Layers: not drawn anywhere.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
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

/// Text outline, drawn around the glyphs (under the fill).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Outline {
    /// Canvas pixels.
    pub width: f64,
    pub color: Rgba,
}

/// Drop shadow of the text (and its outline).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Shadow {
    /// Colour; its alpha is the shadow's opacity.
    pub color: Rgba,
    /// Blur radius, canvas pixels.
    pub blur: f64,
    /// Offset in canvas pixels, x right, y down.
    pub offset: (f64, f64),
}

impl Shadow {
    /// `(angle in degrees clockwise from "right", distance)` of the offset.
    pub fn polar(&self) -> (f64, f64) {
        let (x, y) = self.offset;
        (y.atan2(x).to_degrees().rem_euclid(360.0), x.hypot(y))
    }

    pub fn from_polar(angle: f64, distance: f64) -> (f64, f64) {
        let a = angle.to_radians();
        (distance * a.cos(), distance * a.sin())
    }
}

fn one() -> f64 {
    1.0
}

fn is_one(v: &f64) -> bool {
    *v == 1.0
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paragraph {
    pub align: Align,
    /// Line height as a multiple of the font's own (1 = as the font says).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub line_spacing: f64,
    pub runs: Vec<Run>,
}

impl Default for Paragraph {
    fn default() -> Self {
        Paragraph {
            align: Align::default(),
            line_spacing: 1.0,
            runs: Vec::new(),
        }
    }
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
    /// Extra space between letters, canvas pixels.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub letter_spacing: f64,
}

impl Run {
    /// Same style (everything but the text).
    pub fn same_style(&self, o: &Run) -> bool {
        self.font == o.font
            && self.size == o.size
            && self.color == o.color
            && self.bold == o.bold
            && self.italic == o.italic
            && self.underline == o.underline
            && self.letter_spacing == o.letter_spacing
    }
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
        self.elements.iter().filter_map(Element::as_text)
    }

    pub fn images(&self) -> impl Iterator<Item = &ImageBox> {
        self.elements.iter().filter_map(Element::as_image)
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
