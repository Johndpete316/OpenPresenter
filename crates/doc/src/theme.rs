//! Themes (docs/EDITOR-PLAN.md E7): a named canvas size, background, text box
//! position and full text style, kept in `Library/Themes/<name>.optheme`.
//!
//! - Applying a theme is one edit per presentation ([`Edit::ApplyTheme`]).
//! - Native documents keep a link on each slide ([`ThemeLink`]), so editing the
//!   theme can restyle them; `.pro6` documents get a copy of the styles.
//! - A theme with another canvas size rescales the presentation (the M4 rule:
//!   uniform fit, centred) when applied to all of it, and is scaled into the
//!   canvas when applied to some slides.
//! - A theme is edited in the editor as a one-slide document (see
//!   [`Theme::to_document`]); saving writes the `.optheme`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::compat::Mode;
use crate::model::*;
use crate::style::{RunStyle, TextStyle};
use crate::{FORMAT_VERSION, THEME_EXT};

/// Slide (or theme document) → the theme it follows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeLink {
    pub uuid: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub format_version: u32,
    pub uuid: String,
    pub name: String,
    pub canvas: CanvasSize,
    pub background: Option<Rgba>,
    /// Media path relative to the theme file (or absolute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_image: Option<BackgroundImage>,
    /// The main text box's place, canvas pixels.
    pub text_frame: Rect,
    pub text: TextStyle,
    #[serde(skip)]
    pub path: PathBuf,
}

pub fn is_theme(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(THEME_EXT))
}

/// File name for a theme called `name` (characters a file name can't hold are
/// replaced).
pub fn file_name(name: &str) -> String {
    let safe: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_()&'".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.trim().is_empty() {
        "Theme".to_owned()
    } else {
        safe
    };
    format!("{}.{THEME_EXT}", safe.trim())
}

/// `target` as a path relative to `base_dir` (forward slashes), when they share a
/// root; otherwise `target` as it is.
pub fn relative_to(target: &Path, base_dir: &Path) -> String {
    use std::path::Component;
    let (target, base_dir) = (&normalise(target), &normalise(base_dir));
    let t: Vec<Component> = target.components().collect();
    let b: Vec<Component> = base_dir.components().collect();
    let common = t.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let shares_root = common > 0
        && !matches!(
            (t.first(), b.first()),
            (Some(Component::Prefix(x)), Some(Component::Prefix(y))) if x != y
        );
    if !shares_root || !target.is_absolute() {
        return target.to_string_lossy().into_owned();
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in common..b.len() {
        parts.push("..".into());
    }
    for c in &t[common..] {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    parts.join("/")
}

/// `p` with `.` and `dir/..` taken out (without touching the disk).
fn normalise(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir
                if matches!(out.components().next_back(), Some(Component::Normal(_))) =>
            {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("{path}: {source}", path = .path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: not a readable theme: {message}", path = .path.display())]
    Json { path: PathBuf, message: String },
    #[error("the slide has no text box with styled text to take the style from")]
    NoText,
}

impl ThemeError {
    pub fn user_message(&self) -> String {
        match self {
            ThemeError::Io { path, source } => {
                format!("The theme '{}' couldn't be read or written ({source}).", path.display())
            }
            ThemeError::Json { path, message } => format!(
                "'{}' is damaged or isn't an OpenPresenter theme ({message}).",
                path.display()
            ),
            ThemeError::NoText => "A theme takes its text style from a text box, and this slide has none with styled text. Nothing was made.".to_owned(),
        }
    }
}

impl Theme {
    /// A theme from slide `(g, s)` of `doc`: its canvas, background and first text
    /// box. The background image keeps pointing at the same file (absolute path;
    /// [`Theme::save`] makes it relative to the theme file).
    pub fn from_slide(
        name: &str,
        doc: &Document,
        (g, s): (usize, usize),
    ) -> Result<Theme, ThemeError> {
        let slide = doc
            .groups
            .get(g)
            .and_then(|gr| gr.slides.get(s))
            .ok_or(ThemeError::NoText)?;
        let t = slide
            .text_boxes()
            .find(|t| t.problem.is_none() && t.runs().next().is_some())
            .ok_or(ThemeError::NoText)?;
        let Some(crate::Style::Text(text)) = crate::Style::of(&Element::Text(Box::new(t.clone())))
        else {
            return Err(ThemeError::NoText);
        };
        let background_image = slide.background_image.clone().map(|mut b| {
            b.media.path = b.media.resolve(&doc.path).to_string_lossy().into_owned();
            b
        });
        Ok(Theme {
            format_version: FORMAT_VERSION,
            uuid: crate::new_uuid(),
            name: name.trim().to_owned(),
            canvas: doc.canvas,
            background: slide.background.or(doc.background),
            background_image,
            text_frame: t.frame,
            text,
            path: PathBuf::new(),
        })
    }

    /// The built-in look for a new presentation when there's nothing to copy: white
    /// centred text on black, sized for `canvas`.
    pub fn plain(canvas: CanvasSize) -> Theme {
        let (w, h) = (f64::from(canvas.width), f64::from(canvas.height));
        Theme {
            format_version: FORMAT_VERSION,
            uuid: crate::new_uuid(),
            name: "Plain".into(),
            canvas,
            background: Some(Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            }),
            background_image: None,
            text_frame: Rect {
                x: (w * 0.05).round(),
                y: (h * 0.08).round(),
                width: (w * 0.9).round(),
                height: (h * 0.84).round(),
            },
            text: TextStyle {
                run: RunStyle {
                    font: "Arial".into(),
                    // 72 on a 1080-high canvas.
                    size: (h / 15.0).round(),
                    color: Some([255, 255, 255]),
                    bold: false,
                    italic: false,
                    underline: false,
                    letter_spacing: 0.0,
                },
                align: Align::Center,
                line_spacing: 1.0,
                valign: VAlign::Middle,
                outline: None,
                shadow: None,
            },
            path: PathBuf::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Theme, ThemeError> {
        let bytes = std::fs::read(path).map_err(|source| ThemeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let mut t: Theme = serde_json::from_slice(&bytes).map_err(|e| ThemeError::Json {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        t.path = path.to_path_buf();
        Ok(t)
    }

    /// Write to `path` (atomically), with the background image path relative to it.
    pub fn save(&self, path: &Path) -> Result<Theme, ThemeError> {
        let mut t = self.clone();
        t.path = path.to_path_buf();
        if let Some(b) = t.background_image.as_mut() {
            let abs = b.media.resolve(&self.path);
            let dir = path.parent().unwrap_or(Path::new(""));
            b.media.path = relative_to(&abs, dir);
        }
        let bytes = serde_json::to_vec_pretty(&t).unwrap_or_default();
        crate::file::write_atomic(path, &bytes).map_err(|source| ThemeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(t)
    }

    /// This theme with its media re-pointed for a document at `doc_path`.
    pub fn for_document(&self, doc_path: &Path) -> Theme {
        let mut t = self.clone();
        if let Some(b) = t.background_image.as_mut() {
            let abs = b.media.resolve(&self.path);
            let dir = doc_path.parent().unwrap_or(Path::new(""));
            b.media.path = relative_to(&abs, dir);
        }
        t
    }

    pub fn link(&self) -> ThemeLink {
        ThemeLink {
            uuid: self.uuid.clone(),
            name: self.name.clone(),
        }
    }

    /// The theme as a one-slide document, for editing it in the editor.
    pub fn to_document(&self) -> Document {
        let r = &self.text.run;
        let text_box = TextBox {
            uuid: crate::new_uuid(),
            name: "Theme text".into(),
            frame: self.text_frame,
            rotation: 0.0,
            opacity: 1.0,
            locked: false,
            hidden: false,
            valign: self.text.valign,
            fill: None,
            corner_radius: 0.0,
            outline: self.text.outline,
            shadow: self.text.shadow,
            paragraphs: vec![Paragraph {
                align: self.text.align,
                line_spacing: self.text.line_spacing,
                runs: vec![Run {
                    // '\n' inside a run is a line break within the paragraph.
                    text: "Sample text for this theme\nSecond line".into(),
                    font: r.font.clone(),
                    size: r.size,
                    color: r.color,
                    bold: r.bold,
                    italic: r.italic,
                    underline: r.underline,
                    letter_spacing: r.letter_spacing,
                }],
            }],
            problem: None,
            src: None,
        };
        Document {
            path: self.path.clone(),
            format_version: FORMAT_VERSION,
            mode: Mode::Native,
            canvas: self.canvas,
            background: None,
            selected_arrangement: None,
            groups: vec![Group {
                uuid: crate::new_uuid(),
                name: self.name.clone(),
                color: None,
                slides: vec![Slide {
                    uuid: crate::new_uuid(),
                    enabled: true,
                    label: String::new(),
                    notes: String::new(),
                    hot_key: String::new(),
                    background: self.background,
                    background_image: self.background_image.clone(),
                    elements: vec![Element::Text(Box::new(text_box))],
                    timer: None,
                    pro6_incompatible: false,
                    theme: None,
                    src: None,
                }],
                src: None,
            }],
            arrangements: Vec::new(),
            cue_report: CueReport::default(),
            theme: Some(self.link()),
            pro6: None,
        }
    }

    /// Back from an edited theme document (its first slide and text box).
    pub fn from_document(doc: &Document) -> Result<Theme, ThemeError> {
        let link = doc.theme.clone().unwrap_or_else(|| ThemeLink {
            uuid: crate::new_uuid(),
            name: doc
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        });
        let mut t = Theme::from_slide(&link.name, doc, (0, 0))?;
        t.uuid = link.uuid;
        t.path = doc.path.clone();
        Ok(t)
    }

    /// Text boxes of a slide get the theme's style; the first also its place.
    /// `scale`, `(ox, oy)`: how the theme's canvas maps into the document's.
    fn style_slide(&self, slide: &mut Slide, scale: f64, (ox, oy): (f64, f64), link: bool) {
        slide.background = self.background;
        slide.background_image = self.background_image.clone();
        slide.theme = link.then(|| self.link());
        let st = &self.text;
        let mut first = true;
        for el in &mut slide.elements {
            let Element::Text(t) = el else { continue };
            if t.problem.is_some() {
                continue;
            }
            if first {
                t.frame = Rect {
                    x: (ox + self.text_frame.x * scale).round(),
                    y: (oy + self.text_frame.y * scale).round(),
                    width: (self.text_frame.width * scale).round().max(1.0),
                    height: (self.text_frame.height * scale).round().max(1.0),
                };
                t.rotation = 0.0;
                first = false;
            }
            let run = RunStyle {
                size: ((st.run.size * scale * 2.0).round() / 2.0).max(1.0),
                letter_spacing: st.run.letter_spacing * scale,
                ..st.run.clone()
            };
            for p in &mut t.paragraphs {
                p.align = st.align;
                p.line_spacing = st.line_spacing;
                for r in &mut p.runs {
                    run.apply(r);
                }
            }
            crate::rich::normalize(&mut t.paragraphs);
            t.valign = st.valign;
            t.outline = st.outline.map(|o| Outline {
                width: o.width * scale,
                ..o
            });
            t.shadow = st.shadow.map(|s| Shadow {
                blur: s.blur * scale,
                offset: (s.offset.0 * scale, s.offset.1 * scale),
                ..s
            });
        }
    }

    /// Apply to `slides` of `doc` (all of them when `all`). With `all`, a different
    /// canvas size rescales the whole presentation to the theme's first.
    pub fn apply(&self, doc: &mut Document, slides: &[(usize, usize)], all: bool, link: bool) {
        if all && doc.canvas != self.canvas {
            rescale(doc, self.canvas);
        }
        let (scale, offset) = fit(self.canvas, doc.canvas);
        let targets: Vec<(usize, usize)> = if all {
            doc.groups
                .iter()
                .enumerate()
                .flat_map(|(g, gr)| (0..gr.slides.len()).map(move |s| (g, s)))
                .collect()
        } else {
            slides.to_vec()
        };
        for (g, s) in targets {
            if let Some(slide) = doc.groups.get_mut(g).and_then(|gr| gr.slides.get_mut(s)) {
                self.style_slide(slide, scale, offset, link);
            }
        }
    }

    /// Whether `.pro6` can hold what this theme sets.
    pub fn features(&self) -> Vec<crate::Feature> {
        use crate::Feature as F;
        let mut v = vec![
            F::BackgroundColor,
            F::ElementFrame,
            F::Text,
            F::FontFamily,
            F::FontSize,
            F::TextColor,
            F::Bold,
            F::Italic,
            F::Underline,
            F::HorizontalAlign,
            F::VerticalAlign,
        ];
        if self.text.line_spacing != 1.0 {
            v.push(F::LineSpacing);
        }
        if self.text.run.letter_spacing != 0.0 {
            v.push(F::LetterSpacing);
        }
        if self.text.outline.is_some() {
            v.push(F::TextOutline);
        }
        if self.text.shadow.is_some() {
            v.push(F::TextShadow);
        }
        if let Some(b) = &self.background_image {
            v.push(F::ImageBackground);
            if b.adjust.has_color() {
                v.push(F::ImageAdjustColor);
            }
            if b.adjust.has_vignette() {
                v.push(F::Vignette);
            }
        }
        v
    }
}

/// Uniform fit of canvas `from` into `to`, centred: `(scale, (x offset, y offset))`.
pub fn fit(from: CanvasSize, to: CanvasSize) -> (f64, (f64, f64)) {
    if from == to || from.width == 0 || from.height == 0 {
        return (1.0, (0.0, 0.0));
    }
    let sx = f64::from(to.width) / f64::from(from.width);
    let sy = f64::from(to.height) / f64::from(from.height);
    let s = sx.min(sy);
    (
        s,
        (
            (f64::from(to.width) - f64::from(from.width) * s) / 2.0,
            (f64::from(to.height) - f64::from(from.height) * s) / 2.0,
        ),
    )
}

/// Change the canvas size, scaling everything drawn on it (the M4 rescale rule:
/// uniform fit, centred; text sizes and effects by the same factor).
pub fn rescale(doc: &mut Document, to: CanvasSize) {
    let (s, (ox, oy)) = fit(doc.canvas, to);
    doc.canvas = to;
    if s == 1.0 && ox == 0.0 && oy == 0.0 {
        return;
    }
    let map = |r: &Rect| Rect {
        x: (ox + r.x * s).round(),
        y: (oy + r.y * s).round(),
        width: (r.width * s).round().max(1.0),
        height: (r.height * s).round().max(1.0),
    };
    for slide in doc.groups.iter_mut().flat_map(|g| &mut g.slides) {
        for el in &mut slide.elements {
            match el {
                Element::Text(t) => {
                    t.frame = map(&t.frame);
                    for r in t.paragraphs.iter_mut().flat_map(|p| &mut p.runs) {
                        r.size = ((r.size * s * 2.0).round() / 2.0).max(1.0);
                        r.letter_spacing *= s;
                    }
                    if let Some(o) = t.outline.as_mut() {
                        o.width *= s;
                    }
                    if let Some(sh) = t.shadow.as_mut() {
                        sh.blur *= s;
                        sh.offset = (sh.offset.0 * s, sh.offset.1 * s);
                    }
                }
                Element::Image(i) => i.frame = map(&i.frame),
                Element::Pro6(_) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths() {
        assert_eq!(
            relative_to(
                Path::new("/lib/Themes/Media/a.png"),
                Path::new("/lib/songs")
            ),
            "../Themes/Media/a.png"
        );
        assert_eq!(
            relative_to(Path::new("/lib/songs/Media/a.png"), Path::new("/lib/songs")),
            "Media/a.png"
        );
        assert_eq!(
            relative_to(Path::new("rel/a.png"), Path::new("/x")),
            "rel/a.png"
        );
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("Sunday / Blue"), "Sunday _ Blue.optheme");
        assert_eq!(file_name("  "), "Theme.optheme");
    }

    #[test]
    fn fit_centres_another_aspect() {
        let (s, (ox, oy)) = fit(
            CanvasSize {
                width: 1024,
                height: 768,
            },
            CanvasSize {
                width: 1920,
                height: 1080,
            },
        );
        assert_eq!(s, 1080.0 / 768.0);
        assert!((ox - (1920.0 - 1024.0 * s) / 2.0).abs() < 1e-9);
        assert_eq!(oy, 0.0);
    }
}
