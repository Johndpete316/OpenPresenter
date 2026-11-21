//! Edits: every change the editor makes is one [`Edit`]. Each either applies fully
//! or returns an error and leaves the document unchanged (it validates first, or
//! works on a copy). Undo is snapshot-based (see `history`), so edits don't carry
//! their own revert; they describe themselves (`label`) and what they use
//! (`features`, for the `.pro6` compatibility check).

use crate::compat::{Feature, Mode, needs_native};
use crate::convert::fresh_slide;
use crate::model::*;

pub use pro6::ops::new_uuid;

#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Replace the text of the slide's first text box (`\n` = new paragraph), in the
    /// style of its first run and the alignment of its first paragraph.
    SetText {
        group: usize,
        slide: usize,
        text: String,
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
            Edit::SetText { .. } => "Edit text".into(),
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
            Edit::Batch { label, .. } => label.clone(),
        }
    }

    /// Features this edit uses (checked against `.pro6` support before applying).
    pub fn features(&self) -> Vec<Feature> {
        match self {
            Edit::SetText { .. } | Edit::AddSlide { .. } => {
                vec![Feature::Text, Feature::SlideStructure]
            }
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
            Edit::SetText { group, slide, .. } | Edit::DuplicateSlide { group, slide } => {
                at(*group, *slide).into_iter().collect()
            }
            Edit::Batch { edits, .. } => edits.iter().flat_map(|e| e.slides_touched(doc)).collect(),
            _ => Vec::new(),
        }
    }
}

fn group_mut(doc: &mut Document, group: usize) -> Result<&mut Group, EditError> {
    doc.groups
        .get_mut(group)
        .ok_or_else(|| not_found("group"))
}

fn slide_mut(doc: &mut Document, group: usize, slide: usize) -> Result<&mut Slide, EditError> {
    group_mut(doc, group)?
        .slides
        .get_mut(slide)
        .ok_or_else(|| not_found("slide"))
}

fn first_text_mut(s: &mut Slide) -> Option<&mut TextBox> {
    s.elements.iter_mut().find_map(|e| match e {
        Element::Text(t) => Some(&mut **t),
        Element::Pro6(_) => None,
    })
}

fn arrangement_mut<'a>(doc: &'a mut Document, uuid: &str) -> Result<&'a mut Arrangement, EditError> {
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
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    t.paragraphs = text
        .split('\n')
        .map(|line| Paragraph {
            align,
            runs: if line.is_empty() {
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
        /// UUIDs of the slides that would only open correctly in OpenPresent.
        slides: Vec<String>,
    },
}

/// Check `features` (used on `slides`) against `doc`'s mode.
pub fn check_features(doc: &Document, features: &[Feature], slides: Vec<String>) -> Check {
    let native: Vec<Feature> = features.iter().copied().filter(|f| needs_native(*f)).collect();
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
