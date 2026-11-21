//! Copy Style / paste style (docs/EDITOR-PLAN.md E5).
//!
//! A [`Style`] is everything about how an element looks, without its content or
//! place: for text, the run attributes, alignment, spacing, vertical alignment,
//! outline and shadow; for images, the adjustments and opacity. Pasting builds one
//! [`Edit::Batch`] for any number of targets, so it's one undo step whatever the
//! scope.

use serde::{Deserialize, Serialize};

use crate::edit::Edit;
use crate::model::*;

/// The look of a text run, without its text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunStyle {
    pub font: String,
    pub size: f64,
    pub color: Option<[u8; 3]>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub letter_spacing: f64,
}

impl RunStyle {
    fn of(r: &Run) -> RunStyle {
        RunStyle {
            font: r.font.clone(),
            size: r.size,
            color: r.color,
            bold: r.bold,
            italic: r.italic,
            underline: r.underline,
            letter_spacing: r.letter_spacing,
        }
    }

    pub(crate) fn apply(&self, r: &mut Run) {
        r.font.clone_from(&self.font);
        r.size = self.size;
        r.color = self.color;
        r.bold = self.bold;
        r.italic = self.italic;
        r.underline = self.underline;
        r.letter_spacing = self.letter_spacing;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    pub run: RunStyle,
    pub align: Align,
    pub line_spacing: f64,
    pub valign: VAlign,
    pub outline: Option<Outline>,
    pub shadow: Option<Shadow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageStyle {
    pub adjust: Adjust,
    pub opacity: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Style {
    Text(TextStyle),
    Image(ImageStyle),
}

impl Style {
    /// The style of `e`, if it has one to copy (a text box needs styled text).
    pub fn of(e: &Element) -> Option<Style> {
        match e {
            Element::Text(t) => {
                if t.problem.is_some() {
                    return None;
                }
                let first = t.runs().next()?;
                let p = t.paragraphs.first();
                Some(Style::Text(TextStyle {
                    run: RunStyle::of(first),
                    align: p.map(|p| p.align).unwrap_or_default(),
                    line_spacing: p.map_or(1.0, |p| p.line_spacing),
                    valign: t.valign,
                    outline: t.outline,
                    shadow: t.shadow,
                }))
            }
            Element::Image(i) => Some(Style::Image(ImageStyle {
                adjust: i.adjust,
                opacity: i.opacity,
            })),
            Element::Pro6(_) => None,
        }
    }

    /// Whether this style can go on `e` (text on text boxes, image on images).
    pub fn fits(&self, e: &Element) -> bool {
        match (self, e) {
            (Style::Text(_), Element::Text(t)) => t.problem.is_none(),
            (Style::Image(_), Element::Image(_)) => true,
            _ => false,
        }
    }

    /// Edits giving element `(g, s, e)` this style (empty if it already has it).
    pub fn edits_for(&self, d: &Document, (g, s, e): (usize, usize, usize)) -> Vec<Edit> {
        let Some(el) = d
            .groups
            .get(g)
            .and_then(|gr| gr.slides.get(s))
            .and_then(|sl| sl.elements.get(e))
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        match (self, el) {
            (Style::Text(st), Element::Text(t)) if t.problem.is_none() => {
                let mut paragraphs = t.paragraphs.clone();
                for p in &mut paragraphs {
                    p.align = st.align;
                    p.line_spacing = st.line_spacing;
                    for r in &mut p.runs {
                        st.run.apply(r);
                    }
                }
                crate::rich::normalize(&mut paragraphs);
                if paragraphs != t.paragraphs {
                    out.push(Edit::SetRichText {
                        group: g,
                        slide: s,
                        element: e,
                        paragraphs,
                    });
                }
                if st.valign != t.valign {
                    out.push(Edit::SetVAlign {
                        group: g,
                        slide: s,
                        element: e,
                        valign: st.valign,
                    });
                }
                if st.outline != t.outline {
                    out.push(Edit::SetOutline {
                        group: g,
                        slide: s,
                        element: e,
                        outline: st.outline,
                    });
                }
                if st.shadow != t.shadow {
                    out.push(Edit::SetShadow {
                        group: g,
                        slide: s,
                        element: e,
                        shadow: st.shadow,
                    });
                }
            }
            (Style::Image(st), Element::Image(i)) => {
                if st.adjust != i.adjust {
                    out.push(Edit::SetImageAdjust {
                        group: g,
                        slide: s,
                        element: e,
                        adjust: st.adjust,
                    });
                }
                if st.opacity != i.opacity {
                    out.push(Edit::SetOpacity {
                        group: g,
                        slide: s,
                        element: e,
                        opacity: st.opacity,
                    });
                }
            }
            _ => {}
        }
        out
    }
}

/// Where to paste a style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// These elements of slide `(g, s)`.
    Elements(usize, usize, Vec<usize>),
    /// Every element it fits on slide `(g, s)`.
    Slide(usize, usize),
    /// Every slide in group `g`.
    Group(usize),
    Presentation,
}

/// Elements `style` would go on for `scope`.
pub fn targets(d: &Document, style: &Style, scope: &Scope) -> Vec<(usize, usize, usize)> {
    let slide_targets = |g: usize, s: usize| -> Vec<(usize, usize, usize)> {
        d.groups
            .get(g)
            .and_then(|gr| gr.slides.get(s))
            .map(|sl| {
                sl.elements
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| style.fits(e))
                    .map(|(e, _)| (g, s, e))
                    .collect()
            })
            .unwrap_or_default()
    };
    match scope {
        Scope::Elements(g, s, es) => es
            .iter()
            .filter(|&&e| {
                d.groups
                    .get(*g)
                    .and_then(|gr| gr.slides.get(*s))
                    .and_then(|sl| sl.elements.get(e))
                    .is_some_and(|el| style.fits(el))
            })
            .map(|&e| (*g, *s, e))
            .collect(),
        Scope::Slide(g, s) => slide_targets(*g, *s),
        Scope::Group(g) => (0..d.groups.get(*g).map_or(0, |gr| gr.slides.len()))
            .flat_map(|s| slide_targets(*g, s))
            .collect(),
        Scope::Presentation => d
            .groups
            .iter()
            .enumerate()
            .flat_map(|(g, gr)| (0..gr.slides.len()).map(move |s| (g, s)))
            .flat_map(|(g, s)| slide_targets(g, s))
            .collect(),
    }
}

/// One edit pasting `style` onto everything in `scope` it fits, or `None` when
/// nothing would change. Also returns how many elements change.
pub fn paste(d: &Document, style: &Style, scope: &Scope) -> Option<(Edit, usize)> {
    let mut changed = 0;
    let edits: Vec<Edit> = targets(d, style, scope)
        .into_iter()
        .flat_map(|t| {
            let e = style.edits_for(d, t);
            if !e.is_empty() {
                changed += 1;
            }
            e
        })
        .collect();
    (!edits.is_empty()).then(|| {
        (
            Edit::Batch {
                label: "Paste style".into(),
                edits,
            },
            changed,
        )
    })
}
