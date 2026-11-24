//! Edits: every change the editor makes is one [`Edit`]. Each either applies fully
//! or returns an error and leaves the document unchanged (it validates first, or
//! works on a copy). Undo is snapshot-based (see `history`), so edits don't carry
//! their own revert; they describe themselves (`label`) and what they use
//! (`features`, for the `.pro6` compatibility check).

use crate::compat::{Feature, Mode, needs_native};
use crate::convert::fresh_slide;
use crate::model::*;

pub use pro6::ops::new_uuid;

/// Shortest and longest slide timer (seconds).
pub const MIN_TIMER_SECS: f64 = 0.5;
pub const MAX_TIMER_SECS: f64 = 3600.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Replace the text of the slide's first text box (`\n` = new paragraph), in the
    /// style of its first run and the alignment of its first paragraph.
    SetText {
        group: usize,
        slide: usize,
        text: String,
    },
    /// Replace the text of text box `element` (index into the slide's elements),
    /// like [`Edit::SetText`].
    SetBoxText {
        group: usize,
        slide: usize,
        element: usize,
        text: String,
    },
    /// Replace a text box's styled text (rich text editing, style changes).
    SetRichText {
        group: usize,
        slide: usize,
        element: usize,
        paragraphs: Vec<Paragraph>,
    },
    SetVAlign {
        group: usize,
        slide: usize,
        element: usize,
        valign: VAlign,
    },
    /// Text outline (`None` = none).
    SetOutline {
        group: usize,
        slide: usize,
        element: usize,
        outline: Option<Outline>,
    },
    /// Drop shadow (`None` = none).
    SetShadow {
        group: usize,
        slide: usize,
        element: usize,
        shadow: Option<Shadow>,
    },
    /// Move / resize an element (canvas pixels).
    SetFrame {
        group: usize,
        slide: usize,
        element: usize,
        frame: Rect,
    },
    /// Rotate an element (degrees clockwise).
    SetRotation {
        group: usize,
        slide: usize,
        element: usize,
        degrees: f64,
    },
    /// Element opacity, 0–1.
    SetOpacity {
        group: usize,
        slide: usize,
        element: usize,
        opacity: f64,
    },
    /// Mirror an image.
    SetFlip {
        group: usize,
        slide: usize,
        element: usize,
        horizontal: bool,
        vertical: bool,
    },
    /// Image adjustments (brightness, ..., vignette).
    SetImageAdjust {
        group: usize,
        slide: usize,
        element: usize,
        adjust: Adjust,
    },
    /// Insert an element at `at` in the slide's elements (`None` = on top). It must
    /// have a fresh UUID.
    AddElement {
        group: usize,
        slide: usize,
        at: Option<usize>,
        element: Element,
    },
    RemoveElement {
        group: usize,
        slide: usize,
        element: usize,
    },
    /// Change an element's place in the stacking order (`to` is its index after
    /// the move; the last element is drawn on top).
    MoveElement {
        group: usize,
        slide: usize,
        from: usize,
        to: usize,
    },
    SetLocked {
        group: usize,
        slide: usize,
        element: usize,
        locked: bool,
    },
    SetHidden {
        group: usize,
        slide: usize,
        element: usize,
        hidden: bool,
    },
    RenameElement {
        group: usize,
        slide: usize,
        element: usize,
        name: String,
    },
    /// Apply a theme (already pointed at this document's folder, see
    /// [`crate::Theme::for_document`]) to `slides`, or to every slide when `all`
    /// (which also rescales the canvas to the theme's). `link`: remember the theme
    /// on each slide (native documents).
    ApplyTheme {
        slides: Vec<(usize, usize)>,
        theme: Box<crate::Theme>,
        all: bool,
        link: bool,
    },
    /// The slide's video (`None` = none).
    SetSlideVideo {
        group: usize,
        slide: usize,
        video: Option<SlideVideo>,
    },
    /// Slide background image (`None` = none).
    SetBackgroundImage {
        group: usize,
        slide: usize,
        image: Option<BackgroundImage>,
    },
    SetSlideEnabled {
        group: usize,
        slide: usize,
        enabled: bool,
    },
    /// The slide's auto-advance timer (`None` = none: it waits for the operator).
    SetSlideTimer {
        group: usize,
        slide: usize,
        timer: Option<SlideTimer>,
    },
    SetSlideNotes {
        group: usize,
        slide: usize,
        notes: String,
    },
    /// Slide background colour (`None` = the document's / none).
    SetSlideBackground {
        group: usize,
        slide: usize,
        color: Option<Rgba>,
    },
    /// New slide at `at` in `group`, copying the layout of a neighbouring slide.
    AddSlide {
        group: usize,
        at: usize,
        text: String,
    },
    DuplicateSlide {
        group: usize,
        slide: usize,
    },
    RemoveSlide {
        group: usize,
        slide: usize,
    },
    /// Move a slide (within a group or to another one). `to` is the index in the
    /// destination group after removal from the source.
    MoveSlide {
        group: usize,
        slide: usize,
        to_group: usize,
        to: usize,
    },
    AddGroup {
        at: usize,
        name: String,
        color: Option<Rgba>,
    },
    RenameGroup {
        group: usize,
        name: String,
    },
    RecolorGroup {
        group: usize,
        color: Option<Rgba>,
    },
    MoveGroup {
        from: usize,
        to: usize,
    },
    /// Remove a group, its slides, and its entries in every arrangement.
    RemoveGroup {
        group: usize,
    },
    /// New arrangement; `uuid` is chosen by the caller (use [`new_uuid`]) so it can be
    /// selected afterwards. `groups` are indices into the document's groups.
    AddArrangement {
        uuid: String,
        name: String,
        groups: Vec<usize>,
    },
    RenameArrangement {
        uuid: String,
        name: String,
    },
    SetArrangementGroups {
        uuid: String,
        groups: Vec<usize>,
    },
    RemoveArrangement {
        uuid: String,
    },
    /// Lyrics text → groups, slides and arrangement (see [`crate::lyrics`]).
    /// `colors`: default group colours by name; `theme`: style for new slides
    /// (else they copy the presentation's first slide).
    SetLyrics {
        text: String,
        mode: crate::lyrics::LyricsMode,
        colors: Vec<(String, Rgba)>,
        theme: Option<Box<crate::Theme>>,
    },
    /// Several edits as one step (one undo, all or nothing).
    Batch {
        label: String,
        edits: Vec<Edit>,
    },
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EditError {
    #[error("{what} doesn't exist")]
    NotFound { what: String },
    #[error("the presentation has no slide to copy the layout from")]
    NoTemplate,
    #[error("slide {uuid} has no text box")]
    NoTextBox { uuid: String },
    #[error("text box {uuid} has no styled text to take the font and size from")]
    NoStyle { uuid: String },
    #[error("text box {uuid}'s text is damaged: {message}")]
    Damaged { uuid: String, message: String },
}

impl EditError {
    pub fn user_message(&self) -> String {
        match self {
            EditError::NotFound { what } => format!(
                "That {what} no longer exists (the presentation changed). Nothing was changed."
            ),
            EditError::NoTemplate => "This presentation has no slides to copy the font and layout from, so a new slide can't be made here. Nothing was changed.".to_owned(),
            EditError::NoTextBox { .. } => {
                "This slide has no text box to put text in. Nothing was changed.".to_owned()
            }
            EditError::NoStyle { .. } => "This text box has no font or size set, so new text can't be styled to match. Nothing was changed.".to_owned(),
            EditError::Damaged { message, .. } => format!(
                "This text box's current text is damaged, so it can't be edited safely ({message}). Nothing was changed."
            ),
        }
    }
}

fn not_found(what: &str) -> EditError {
    EditError::NotFound {
        what: what.to_owned(),
    }
}

impl Edit {
    /// For the Undo / Redo menu.
    pub fn label(&self) -> String {
        match self {
            Edit::SetText { .. } | Edit::SetBoxText { .. } => "Edit text".into(),
            Edit::SetRichText { .. } => "Edit text".into(),
            Edit::SetVAlign { .. } => "Vertical alignment".into(),
            Edit::SetOutline { .. } => "Outline".into(),
            Edit::SetShadow { .. } => "Drop shadow".into(),
            Edit::SetFrame { .. } => "Move".into(),
            Edit::SetRotation { .. } => "Rotate".into(),
            Edit::SetOpacity { .. } => "Opacity".into(),
            Edit::SetFlip { .. } => "Flip".into(),
            Edit::SetImageAdjust { .. } => "Adjust image".into(),
            Edit::AddElement {
                element: Element::Image(_),
                ..
            } => "Insert image".into(),
            Edit::AddElement { .. } => "Insert text box".into(),
            Edit::RemoveElement { .. } => "Delete".into(),
            Edit::MoveElement { .. } => "Arrange".into(),
            Edit::SetLocked { locked: true, .. } => "Lock".into(),
            Edit::SetLocked { locked: false, .. } => "Unlock".into(),
            Edit::SetHidden { hidden: true, .. } => "Hide".into(),
            Edit::SetHidden { hidden: false, .. } => "Show".into(),
            Edit::RenameElement { .. } => "Rename".into(),
            Edit::ApplyTheme { theme, .. } => format!("Apply theme {}", theme.name),
            Edit::SetSlideVideo { video: Some(_), .. } => "Set video".into(),
            Edit::SetSlideVideo { video: None, .. } => "Remove video".into(),
            Edit::SetBackgroundImage { image: Some(_), .. } => "Set background image".into(),
            Edit::SetBackgroundImage { image: None, .. } => "Remove background image".into(),
            Edit::SetSlideEnabled { enabled: true, .. } => "Enable slide".into(),
            Edit::SetSlideEnabled { enabled: false, .. } => "Disable slide".into(),
            Edit::SetSlideNotes { .. } => "Edit notes".into(),
            Edit::SetSlideTimer { timer: Some(_), .. } => "Set timer".into(),
            Edit::SetSlideTimer { timer: None, .. } => "Remove timer".into(),
            Edit::SetSlideBackground { .. } => "Change background".into(),
            Edit::AddSlide { .. } => "Add slide".into(),
            Edit::DuplicateSlide { .. } => "Duplicate slide".into(),
            Edit::RemoveSlide { .. } => "Delete slide".into(),
            Edit::MoveSlide { .. } => "Move slide".into(),
            Edit::AddGroup { .. } => "Add group".into(),
            Edit::RenameGroup { .. } => "Rename group".into(),
            Edit::RecolorGroup { .. } => "Change group colour".into(),
            Edit::MoveGroup { .. } => "Move group".into(),
            Edit::RemoveGroup { .. } => "Delete group".into(),
            Edit::AddArrangement { .. } => "Add arrangement".into(),
            Edit::RenameArrangement { .. } => "Rename arrangement".into(),
            Edit::SetArrangementGroups { .. } => "Change arrangement".into(),
            Edit::RemoveArrangement { .. } => "Delete arrangement".into(),
            Edit::SetLyrics { .. } => "Paste lyrics".into(),
            Edit::Batch { label, .. } => label.clone(),
        }
    }

    /// Features this edit uses (checked against `.pro6` support before applying).
    pub fn features(&self) -> Vec<Feature> {
        match self {
            Edit::SetText { .. } | Edit::SetBoxText { .. } | Edit::AddSlide { .. } => {
                vec![Feature::Text, Feature::SlideStructure]
            }
            Edit::SetRichText { paragraphs, .. } => {
                let mut v = vec![
                    Feature::Text,
                    Feature::FontFamily,
                    Feature::FontSize,
                    Feature::TextColor,
                    Feature::Bold,
                    Feature::Italic,
                    Feature::Underline,
                    Feature::HorizontalAlign,
                ];
                if paragraphs.iter().any(|p| p.line_spacing != 1.0) {
                    v.push(Feature::LineSpacing);
                }
                if paragraphs
                    .iter()
                    .flat_map(|p| &p.runs)
                    .any(|r| r.letter_spacing != 0.0)
                {
                    v.push(Feature::LetterSpacing);
                }
                v
            }
            Edit::SetVAlign { .. } => vec![Feature::VerticalAlign],
            // Turning an effect off is only a flag, which .pro6 holds.
            Edit::SetOutline {
                outline: Some(_), ..
            } => vec![Feature::TextOutline],
            Edit::SetShadow {
                shadow: Some(_), ..
            } => vec![Feature::TextShadow],
            Edit::SetOutline { .. } | Edit::SetShadow { .. } => vec![Feature::Text],
            Edit::SetFrame { .. } => vec![Feature::ElementFrame],
            Edit::SetRotation { .. } => vec![Feature::ElementRotation],
            Edit::SetOpacity { .. } => vec![Feature::ElementOpacity],
            Edit::SetFlip { .. } => vec![Feature::ImageElement],
            Edit::SetImageAdjust { adjust, .. } => {
                let mut v = vec![Feature::ImageElement];
                if adjust.has_color() {
                    v.push(Feature::ImageAdjustColor);
                }
                if adjust.has_vignette() {
                    v.push(Feature::Vignette);
                }
                v
            }
            Edit::AddElement { element, .. } => match element {
                Element::Image(_) => vec![Feature::ImageElement],
                // A copy of an existing text box: .pro6 holds it.
                Element::Text(t) if t.src.is_some() => vec![Feature::Text, Feature::ElementFrame],
                _ => vec![Feature::NewTextBox],
            },
            Edit::RemoveElement { .. }
            | Edit::MoveElement { .. }
            | Edit::RenameElement { .. }
            | Edit::SetHidden { hidden: false, .. } => vec![Feature::SlideStructure],
            Edit::SetLocked { .. } => vec![Feature::LayerLock],
            Edit::ApplyTheme { theme, .. } => theme.features(),
            Edit::SetLyrics { theme, .. } => {
                let mut v = vec![Feature::Text, Feature::SlideStructure];
                if let Some(t) = theme {
                    v.extend(t.features());
                }
                v
            }
            Edit::SetHidden { hidden: true, .. } => vec![Feature::LayerHide],
            Edit::SetBackgroundImage { image: Some(b), .. } => {
                let mut v = vec![Feature::ImageBackground];
                if b.adjust.has_color() {
                    v.push(Feature::ImageAdjustColor);
                }
                if b.adjust.has_vignette() {
                    v.push(Feature::Vignette);
                }
                v
            }
            Edit::SetBackgroundImage { image: None, .. } => vec![Feature::SlideStructure],
            Edit::SetSlideVideo { video: Some(_), .. } => vec![Feature::SlideVideo],
            Edit::SetSlideVideo { video: None, .. } => vec![Feature::SlideStructure],
            Edit::SetSlideBackground { .. } => vec![Feature::BackgroundColor],
            Edit::Batch { edits, .. } => {
                let mut v: Vec<Feature> = edits.iter().flat_map(Edit::features).collect();
                v.dedup();
                v
            }
            _ => vec![Feature::SlideStructure],
        }
    }

    /// Apply to `doc`, all or nothing.
    pub fn apply(&self, doc: &mut Document) -> Result<(), EditError> {
        let _span =
            tracing::info_span!("edit", edit = %self.label(), file = %doc.path.display()).entered();
        let r = match self {
            Edit::Batch { edits, .. } => {
                let mut copy = doc.clone();
                let r = edits.iter().try_for_each(|e| e.apply_one(&mut copy));
                if r.is_ok() {
                    *doc = copy;
                }
                r
            }
            e => e.apply_one(doc),
        };
        match &r {
            Ok(()) => tracing::info!("edit applied"),
            Err(e) => tracing::warn!(error = %e, "edit refused"),
        }
        r
    }

    fn apply_one(&self, doc: &mut Document) -> Result<(), EditError> {
        match self {
            Edit::Batch { .. } => self.apply(doc),
            Edit::SetText { group, slide, text } => {
                let s = slide_mut(doc, *group, *slide)?;
                let uuid = s.uuid.clone();
                let t = first_text_mut(s).ok_or(EditError::NoTextBox { uuid })?;
                set_text(t, text)
            }
            Edit::SetBoxText {
                group,
                slide,
                element,
                text,
            } => set_text(text_mut(doc, *group, *slide, *element)?, text),
            Edit::SetRichText {
                group,
                slide,
                element,
                paragraphs,
            } => {
                let t = text_mut(doc, *group, *slide, *element)?;
                if t.problem.is_some() {
                    return set_text(t, "");
                }
                if paragraphs
                    .iter()
                    .flat_map(|p| &p.runs)
                    .any(|r| r.font.trim().is_empty() || !(r.size.is_finite() && r.size > 0.0))
                {
                    return Err(EditError::NoStyle {
                        uuid: t.uuid.clone(),
                    });
                }
                t.paragraphs.clone_from(paragraphs);
                Ok(())
            }
            Edit::SetVAlign {
                group,
                slide,
                element,
                valign,
            } => {
                text_mut(doc, *group, *slide, *element)?.valign = *valign;
                Ok(())
            }
            Edit::SetOutline {
                group,
                slide,
                element,
                outline,
            } => {
                if outline.is_some_and(|o| !(o.width.is_finite() && o.width > 0.0)) {
                    return Err(not_found("usable outline width"));
                }
                text_mut(doc, *group, *slide, *element)?.outline = *outline;
                Ok(())
            }
            Edit::SetShadow {
                group,
                slide,
                element,
                shadow,
            } => {
                text_mut(doc, *group, *slide, *element)?.shadow = *shadow;
                Ok(())
            }
            Edit::SetFrame {
                group,
                slide,
                element,
                frame,
            } => {
                if !(frame.width > 0.0 && frame.height > 0.0)
                    || ![frame.x, frame.y, frame.width, frame.height]
                        .iter()
                        .all(|v| v.is_finite())
                {
                    return Err(not_found("usable size"));
                }
                match placed_mut(doc, *group, *slide, *element)? {
                    Placed::Text(t) => t.frame = *frame,
                    Placed::Image(i) => i.frame = *frame,
                }
                Ok(())
            }
            Edit::SetRotation {
                group,
                slide,
                element,
                degrees,
            } => {
                if !degrees.is_finite() {
                    return Err(not_found("usable angle"));
                }
                match placed_mut(doc, *group, *slide, *element)? {
                    Placed::Text(t) => t.rotation = *degrees,
                    Placed::Image(i) => i.rotation = *degrees,
                }
                Ok(())
            }
            Edit::SetOpacity {
                group,
                slide,
                element,
                opacity,
            } => {
                if !opacity.is_finite() {
                    return Err(not_found("usable opacity"));
                }
                let o = opacity.clamp(0.0, 1.0);
                match placed_mut(doc, *group, *slide, *element)? {
                    Placed::Text(t) => t.opacity = o,
                    Placed::Image(i) => i.opacity = o,
                }
                Ok(())
            }
            Edit::SetFlip {
                group,
                slide,
                element,
                horizontal,
                vertical,
            } => {
                let i = image_mut(doc, *group, *slide, *element)?;
                i.flip_h = *horizontal;
                i.flip_v = *vertical;
                Ok(())
            }
            Edit::SetImageAdjust {
                group,
                slide,
                element,
                adjust,
            } => {
                image_mut(doc, *group, *slide, *element)?.adjust = clean_adjust(adjust)?;
                Ok(())
            }
            Edit::AddElement {
                group,
                slide,
                at,
                element,
            } => {
                let s = slide_mut(doc, *group, *slide)?;
                let at = at.unwrap_or(s.elements.len());
                if at > s.elements.len() {
                    return Err(not_found("element position"));
                }
                if element
                    .frame()
                    .is_some_and(|f| !(f.width > 0.0 && f.height > 0.0))
                {
                    return Err(not_found("usable size"));
                }
                if s.elements.iter().any(|e| e.uuid() == element.uuid()) {
                    return Err(not_found("new element ID"));
                }
                s.elements.insert(at, element.clone());
                Ok(())
            }
            Edit::RemoveElement {
                group,
                slide,
                element,
            } => {
                let s = slide_mut(doc, *group, *slide)?;
                if *element >= s.elements.len() {
                    return Err(not_found("element"));
                }
                s.elements.remove(*element);
                Ok(())
            }
            Edit::SetLyrics {
                text,
                mode,
                colors,
                theme,
            } => {
                let mut copy = doc.clone();
                crate::lyrics::apply(&mut copy, text, *mode, colors, theme.as_deref())?;
                *doc = copy;
                Ok(())
            }
            Edit::ApplyTheme {
                slides,
                theme,
                all,
                link,
            } => {
                for &(g, s) in slides {
                    slide_mut(doc, g, s)?;
                }
                theme.apply(doc, slides, *all, *link);
                Ok(())
            }
            Edit::MoveElement {
                group,
                slide,
                from,
                to,
            } => {
                let s = slide_mut(doc, *group, *slide)?;
                if *from >= s.elements.len() || *to >= s.elements.len() {
                    return Err(not_found("element"));
                }
                let e = s.elements.remove(*from);
                s.elements.insert(*to, e);
                Ok(())
            }
            Edit::SetLocked {
                group,
                slide,
                element,
                locked,
            } => {
                match placed_mut(doc, *group, *slide, *element)? {
                    Placed::Text(t) => t.locked = *locked,
                    Placed::Image(i) => i.locked = *locked,
                }
                Ok(())
            }
            Edit::SetHidden {
                group,
                slide,
                element,
                hidden,
            } => {
                match placed_mut(doc, *group, *slide, *element)? {
                    Placed::Text(t) => t.hidden = *hidden,
                    Placed::Image(i) => i.hidden = *hidden,
                }
                Ok(())
            }
            Edit::RenameElement {
                group,
                slide,
                element,
                name,
            } => {
                match placed_mut(doc, *group, *slide, *element)? {
                    Placed::Text(t) => t.name.clone_from(name),
                    Placed::Image(i) => i.name.clone_from(name),
                }
                Ok(())
            }
            Edit::SetSlideVideo {
                group,
                slide,
                video,
            } => {
                let video = match video {
                    Some(v) => {
                        let ok = |x: f64| x.is_finite();
                        let a = v.adjust;
                        if !ok(v.volume)
                            || ![a.brightness, a.contrast, a.saturation, a.gamma, a.hue]
                                .into_iter()
                                .all(ok)
                        {
                            return Err(EditError::NotFound {
                                what: "valid video setting".into(),
                            });
                        }
                        let c = |x: f64| x.clamp(-1.0, 1.0);
                        Some(SlideVideo {
                            volume: v.volume.clamp(0.0, 1.0),
                            adjust: VideoAdjust {
                                brightness: c(a.brightness),
                                contrast: c(a.contrast),
                                saturation: c(a.saturation),
                                gamma: c(a.gamma),
                                hue: c(a.hue),
                            },
                            ..v.clone()
                        })
                    }
                    None => None,
                };
                slide_mut(doc, *group, *slide)?.video = video;
                Ok(())
            }
            Edit::SetBackgroundImage {
                group,
                slide,
                image,
            } => {
                let image = match image {
                    Some(b) => Some(BackgroundImage {
                        adjust: clean_adjust(&b.adjust)?,
                        ..b.clone()
                    }),
                    None => None,
                };
                slide_mut(doc, *group, *slide)?.background_image = image;
                Ok(())
            }
            Edit::SetSlideEnabled {
                group,
                slide,
                enabled,
            } => {
                slide_mut(doc, *group, *slide)?.enabled = *enabled;
                Ok(())
            }
            Edit::SetSlideNotes {
                group,
                slide,
                notes,
            } => {
                slide_mut(doc, *group, *slide)?.notes.clone_from(notes);
                Ok(())
            }
            Edit::SetSlideTimer {
                group,
                slide,
                timer,
            } => {
                if let Some(t) = timer {
                    let secs = t.duration.as_secs_f64();
                    if !(MIN_TIMER_SECS..=MAX_TIMER_SECS).contains(&secs) {
                        return Err(EditError::NotFound {
                            what: format!(
                                "a timer of {secs} s (it must be {MIN_TIMER_SECS}–{MAX_TIMER_SECS} s)"
                            ),
                        });
                    }
                }
                slide_mut(doc, *group, *slide)?.timer = *timer;
                Ok(())
            }
            Edit::SetSlideBackground {
                group,
                slide,
                color,
            } => {
                slide_mut(doc, *group, *slide)?.background = *color;
                Ok(())
            }
            Edit::AddSlide { group, at, text } => {
                let g = doc.groups.get(*group).ok_or_else(|| not_found("group"))?;
                if *at > g.slides.len() {
                    return Err(not_found("slide position"));
                }
                // Template: the slide before `at` in this group, else after, else any slide.
                let template = g
                    .slides
                    .get(at.wrapping_sub(1))
                    .or_else(|| g.slides.get(*at))
                    .or_else(|| doc.slides().next())
                    .ok_or(EditError::NoTemplate)?;
                let mut new = fresh_slide(template);
                if let Some(t) = first_text_mut(&mut new) {
                    // A box emptied by older versions has no runs; borrow the first
                    // styled run in the presentation so the new slide can still be made.
                    if t.problem.is_none()
                        && t.runs().next().is_none()
                        && let Some(r) = doc.text_boxes().find_map(|b| b.runs().next())
                    {
                        let r = Run {
                            text: String::new(),
                            ..r.clone()
                        };
                        match t.paragraphs.first_mut() {
                            Some(p) => p.runs.push(r),
                            None => t.paragraphs.push(Paragraph {
                                runs: vec![r],
                                ..Paragraph::default()
                            }),
                        }
                    }
                    set_text(t, text)?;
                }
                // A new slide starts enabled, without the template's label/notes/hot key.
                new.enabled = true;
                new.label.clear();
                new.notes.clear();
                new.hot_key.clear();
                doc.groups[*group].slides.insert(*at, new);
                Ok(())
            }
            Edit::DuplicateSlide { group, slide } => {
                let copy = fresh_slide(slide_mut(doc, *group, *slide)?);
                doc.groups[*group].slides.insert(slide + 1, copy);
                Ok(())
            }
            Edit::RemoveSlide { group, slide } => {
                slide_mut(doc, *group, *slide)?;
                doc.groups[*group].slides.remove(*slide);
                Ok(())
            }
            Edit::MoveSlide {
                group,
                slide,
                to_group,
                to,
            } => {
                slide_mut(doc, *group, *slide)?;
                let dest_len = doc
                    .groups
                    .get(*to_group)
                    .ok_or_else(|| not_found("group"))?
                    .slides
                    .len();
                let dest_len_after = if to_group == group {
                    dest_len - 1
                } else {
                    dest_len
                };
                if *to > dest_len_after {
                    return Err(not_found("slide position"));
                }
                let s = doc.groups[*group].slides.remove(*slide);
                doc.groups[*to_group].slides.insert(*to, s);
                Ok(())
            }
            Edit::AddGroup { at, name, color } => {
                if *at > doc.groups.len() {
                    return Err(not_found("group position"));
                }
                doc.groups.insert(
                    *at,
                    Group {
                        uuid: new_uuid(),
                        name: name.clone(),
                        color: *color,
                        slides: Vec::new(),
                        src: None,
                    },
                );
                Ok(())
            }
            Edit::RenameGroup { group, name } => {
                group_mut(doc, *group)?.name.clone_from(name);
                Ok(())
            }
            Edit::RecolorGroup { group, color } => {
                group_mut(doc, *group)?.color = *color;
                Ok(())
            }
            Edit::MoveGroup { from, to } => {
                if *from >= doc.groups.len() || *to >= doc.groups.len() {
                    return Err(not_found("group"));
                }
                let g = doc.groups.remove(*from);
                doc.groups.insert(*to, g);
                Ok(())
            }
            Edit::RemoveGroup { group } => {
                let uuid = group_mut(doc, *group)?.uuid.clone();
                doc.groups.remove(*group);
                for a in &mut doc.arrangements {
                    a.groups.retain(|id| !id.eq_ignore_ascii_case(&uuid));
                }
                Ok(())
            }
            Edit::AddArrangement { uuid, name, groups } => {
                let ids = group_ids(doc, groups)?;
                doc.arrangements.push(Arrangement {
                    uuid: uuid.clone(),
                    name: name.clone(),
                    color: Some(Rgba {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
                    }),
                    groups: ids,
                });
                Ok(())
            }
            Edit::RenameArrangement { uuid, name } => {
                arrangement_mut(doc, uuid)?.name.clone_from(name);
                Ok(())
            }
            Edit::SetArrangementGroups { uuid, groups } => {
                let ids = group_ids(doc, groups)?;
                arrangement_mut(doc, uuid)?.groups = ids;
                Ok(())
            }
            Edit::RemoveArrangement { uuid } => {
                arrangement_mut(doc, uuid)?;
                doc.arrangements
                    .retain(|a| !a.uuid.eq_ignore_ascii_case(uuid));
                if doc
                    .selected_arrangement
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(uuid))
                {
                    doc.selected_arrangement = None;
                }
                Ok(())
            }
        }
    }

    /// Slides (by UUID) this edit changes in a way a `.pro6` might not hold: the
    /// ones flagged when the user accepts a native-only feature.
    pub fn slides_touched(&self, doc: &Document) -> Vec<String> {
        let at = |g: usize, s: usize| {
            doc.groups
                .get(g)
                .and_then(|g| g.slides.get(s))
                .map(|s| s.uuid.clone())
        };
        match self {
            Edit::SetText { group, slide, .. }
            | Edit::SetBoxText { group, slide, .. }
            | Edit::SetRichText { group, slide, .. }
            | Edit::SetVAlign { group, slide, .. }
            | Edit::SetOutline { group, slide, .. }
            | Edit::SetShadow { group, slide, .. }
            | Edit::SetFrame { group, slide, .. }
            | Edit::SetRotation { group, slide, .. }
            | Edit::SetOpacity { group, slide, .. }
            | Edit::SetFlip { group, slide, .. }
            | Edit::SetImageAdjust { group, slide, .. }
            | Edit::AddElement { group, slide, .. }
            | Edit::RemoveElement { group, slide, .. }
            | Edit::MoveElement { group, slide, .. }
            | Edit::SetLocked { group, slide, .. }
            | Edit::SetHidden { group, slide, .. }
            | Edit::RenameElement { group, slide, .. }
            | Edit::SetBackgroundImage { group, slide, .. }
            | Edit::SetSlideVideo { group, slide, .. }
            | Edit::SetSlideTimer { group, slide, .. }
            | Edit::SetSlideBackground { group, slide, .. }
            | Edit::DuplicateSlide { group, slide } => at(*group, *slide).into_iter().collect(),
            Edit::Batch { edits, .. } => edits.iter().flat_map(|e| e.slides_touched(doc)).collect(),
            Edit::ApplyTheme {
                slides, all: true, ..
            } if slides.is_empty() => doc.slides().map(|s| s.uuid.clone()).collect(),
            Edit::ApplyTheme { slides, .. } => {
                slides.iter().filter_map(|&(g, s)| at(g, s)).collect()
            }
            _ => Vec::new(),
        }
    }
}

fn group_mut(doc: &mut Document, group: usize) -> Result<&mut Group, EditError> {
    doc.groups.get_mut(group).ok_or_else(|| not_found("group"))
}

fn slide_mut(doc: &mut Document, group: usize, slide: usize) -> Result<&mut Slide, EditError> {
    group_mut(doc, group)?
        .slides
        .get_mut(slide)
        .ok_or_else(|| not_found("slide"))
}

fn text_mut(
    doc: &mut Document,
    group: usize,
    slide: usize,
    element: usize,
) -> Result<&mut TextBox, EditError> {
    match slide_mut(doc, group, slide)?.elements.get_mut(element) {
        Some(Element::Text(t)) => Ok(t),
        _ => Err(not_found("text box")),
    }
}

fn first_text_mut(s: &mut Slide) -> Option<&mut TextBox> {
    s.elements.iter_mut().find_map(|e| match e {
        Element::Text(t) => Some(&mut **t),
        _ => None,
    })
}

enum Placed<'a> {
    Text(&'a mut TextBox),
    Image(&'a mut ImageBox),
}

/// An element the editor can move (a text box or an image).
fn placed_mut(
    doc: &mut Document,
    group: usize,
    slide: usize,
    element: usize,
) -> Result<Placed<'_>, EditError> {
    match slide_mut(doc, group, slide)?.elements.get_mut(element) {
        Some(Element::Text(t)) => Ok(Placed::Text(t)),
        Some(Element::Image(i)) => Ok(Placed::Image(i)),
        _ => Err(not_found("element")),
    }
}

fn image_mut(
    doc: &mut Document,
    group: usize,
    slide: usize,
    element: usize,
) -> Result<&mut ImageBox, EditError> {
    match slide_mut(doc, group, slide)?.elements.get_mut(element) {
        Some(Element::Image(i)) => Ok(i),
        _ => Err(not_found("image")),
    }
}

/// Adjustments clamped to their ranges; refuses NaN.
fn clean_adjust(a: &Adjust) -> Result<Adjust, EditError> {
    let v = [
        a.brightness,
        a.contrast,
        a.saturation,
        a.temperature,
        a.tint,
        a.vignette,
        a.vignette_size,
    ];
    if !v.iter().all(|v| v.is_finite()) {
        return Err(not_found("usable adjustment"));
    }
    let c = |v: f64| v.clamp(-1.0, 1.0);
    Ok(Adjust {
        brightness: c(a.brightness),
        contrast: c(a.contrast),
        saturation: c(a.saturation),
        temperature: c(a.temperature),
        tint: c(a.tint),
        vignette: a.vignette.clamp(0.0, 1.0),
        vignette_size: a.vignette_size.clamp(0.0, 1.0),
    })
}

fn arrangement_mut<'a>(
    doc: &'a mut Document,
    uuid: &str,
) -> Result<&'a mut Arrangement, EditError> {
    doc.arrangements
        .iter_mut()
        .find(|a| a.uuid.eq_ignore_ascii_case(uuid))
        .ok_or_else(|| not_found("arrangement"))
}

fn group_ids(doc: &Document, seq: &[usize]) -> Result<Vec<String>, EditError> {
    seq.iter()
        .map(|&g| {
            doc.groups
                .get(g)
                .map(|g| g.uuid.clone())
                .ok_or_else(|| not_found("group"))
        })
        .collect()
}

pub(crate) fn set_text_pub(t: &mut TextBox, text: &str) -> Result<(), EditError> {
    set_text(t, text)
}

fn set_text(t: &mut TextBox, text: &str) -> Result<(), EditError> {
    match &t.problem {
        None => {}
        Some(TextProblem::NoStyledText) => {
            return Err(EditError::NoStyle {
                uuid: t.uuid.clone(),
            });
        }
        Some(TextProblem::Unreadable { message }) => {
            return Err(EditError::Damaged {
                uuid: t.uuid.clone(),
                message: message.clone(),
            });
        }
        Some(TextProblem::FontNotInTable { index }) => {
            return Err(EditError::Damaged {
                uuid: t.uuid.clone(),
                message: format!("font {index} isn't in its font table"),
            });
        }
    }
    let Some(style) = t.runs().next().cloned() else {
        return Err(EditError::NoStyle {
            uuid: t.uuid.clone(),
        });
    };
    let align = t.paragraphs.first().map(|p| p.align).unwrap_or_default();
    let line_spacing = t.paragraphs.first().map_or(1.0, |p| p.line_spacing);
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    t.paragraphs = text
        .split('\n')
        .map(|line| Paragraph {
            align,
            line_spacing,
            // An empty box keeps one empty run, so it remembers its style (typing into
            // it, or a new slide copying it, needs the font and size). Empty lines
            // between others have no run, as a .pro6 stores them.
            runs: if line.is_empty() && !text.is_empty() {
                Vec::new()
            } else {
                vec![Run {
                    text: line.to_owned(),
                    ..style.clone()
                }]
            },
        })
        .collect();
    Ok(())
}

/// Outcome of checking an edit against the document's compatibility mode.
#[derive(Debug, Clone, PartialEq)]
pub enum Check {
    /// Apply it.
    Ok,
    /// The document is `.pro6` and the edit uses something `.pro6` can't hold: ask
    /// first ("This won't work in ProPresenter 6").
    NeedsNative {
        features: Vec<Feature>,
        /// UUIDs of the slides that would only open correctly in OpenPresenter.
        slides: Vec<String>,
    },
}

/// Check `features` (used on `slides`) against `doc`'s mode.
pub fn check_features(doc: &Document, features: &[Feature], slides: Vec<String>) -> Check {
    let native: Vec<Feature> = features
        .iter()
        .copied()
        .filter(|f| needs_native(*f))
        .collect();
    if doc.mode == Mode::Pro6 && !native.is_empty() {
        Check::NeedsNative {
            features: native,
            slides,
        }
    } else {
        Check::Ok
    }
}

pub fn check(doc: &Document, edit: &Edit) -> Check {
    check_features(doc, &edit.features(), edit.slides_touched(doc))
}

/// "Continue anyway": apply, flag the touched slides and switch to native mode.
pub fn mark_native(doc: &mut Document, slides: &[String]) {
    doc.mode = Mode::Native;
    for s in doc.groups.iter_mut().flat_map(|g| &mut g.slides) {
        if slides.contains(&s.uuid) {
            s.pro6_incompatible = true;
        }
    }
}
