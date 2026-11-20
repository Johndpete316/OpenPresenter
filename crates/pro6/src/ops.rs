//! Editor operations on a document: slide text, slides, groups and arrangements.
//!
//! Every operation either applies fully or returns an error and leaves the
//! document unchanged (operations validate before mutating). New slides copy an
//! existing slide (layout, font, size) with fresh UUIDs, so nothing is invented.

use std::path::Path;

use crate::edit::{EditError, WinPayloads};
use crate::extra::{Extra, TypedArray};
use crate::model::{DisplayElement, Document, Rgba, Slide, SlideGroup};
use crate::raw::{RawElement, RawNode};

/// New UUID in PP6's style (uppercase, hyphenated v4).
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase()
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Replace the text of the slide's first text box (`\n` = new paragraph).
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
    /// New arrangement with the given group sequence; `uuid` is chosen by the caller
    /// (use [`new_uuid`]) so it can be selected afterwards.
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
}

impl Op {
    /// Apply to `doc`. Text edits drop the Windows-only payloads (see `WinPayloads`).
    pub fn apply(&self, doc: &mut Document) -> Result<(), EditError> {
        let path = doc.path.clone();
        let _span = tracing::info_span!("edit", op = ?self, file = %path.display()).entered();
        let r = self.apply_inner(doc, &path);
        match &r {
            Ok(()) => tracing::info!("edit applied"),
            Err(e) => tracing::warn!(error = %e, "edit refused"),
        }
        r
    }

    fn apply_inner(&self, doc: &mut Document, path: &Path) -> Result<(), EditError> {
        match self {
            Op::SetText { group, slide, text } => {
                let s = slide_mut(doc, *group, *slide)?;
                let uuid = s.uuid.clone();
                let t = first_text_mut(s).ok_or(EditError::NoTextElement { uuid })?;
                t.set_text(text, WinPayloads::Drop, path)
            }
            Op::AddSlide { group, at, text } => {
                let groups_len = groups(doc).len();
                if *group >= groups_len {
                    return Err(not_found("group"));
                }
                let len = groups(doc)[*group].slides().len();
                if *at > len {
                    return Err(not_found("slide position"));
                }
                // Template: the slide before `at` in this group, else after, else any slide.
                let template = groups(doc)[*group]
                    .slides()
                    .get(at.wrapping_sub(1))
                    .or_else(|| groups(doc)[*group].slides().get(*at))
                    .or_else(|| doc.slides().next())
                    .cloned()
                    .ok_or(EditError::NoTemplate)?;
                let mut new = fresh_copy(&template);
                if let Some(t) = first_text_mut(&mut new) {
                    t.set_text(text, WinPayloads::Drop, path)?;
                }
                // A new slide starts enabled, without label/notes/hot key of the template.
                new.enabled = true;
                new.label.clear();
                new.notes.clear();
                new.hot_key.clear();
                slides_mut(doc, *group)?.items.insert(*at, new);
                Ok(())
            }
            Op::DuplicateSlide { group, slide } => {
                let copy = fresh_copy(slide_mut(doc, *group, *slide)?);
                slides_mut(doc, *group)?.items.insert(slide + 1, copy);
                Ok(())
            }
            Op::RemoveSlide { group, slide } => {
                slide_mut(doc, *group, *slide)?;
                slides_mut(doc, *group)?.items.remove(*slide);
                Ok(())
            }
            Op::MoveSlide {
                group,
                slide,
                to_group,
                to,
            } => {
                slide_mut(doc, *group, *slide)?;
                let dest_len = groups(doc)
                    .get(*to_group)
                    .ok_or_else(|| not_found("group"))?
                    .slides()
                    .len();
                let dest_len_after = if to_group == group {
                    dest_len - 1
                } else {
                    dest_len
                };
                if *to > dest_len_after {
                    return Err(not_found("slide position"));
                }
                let s = slides_mut(doc, *group)?.items.remove(*slide);
                slides_mut(doc, *to_group)?.items.insert(*to, s);
                Ok(())
            }
            Op::AddGroup { at, name, color } => {
                let arr = groups_array_mut(doc);
                if *at > arr.items.len() {
                    return Err(not_found("group position"));
                }
                arr.items.insert(*at, new_group(name, *color));
                Ok(())
            }
            Op::RenameGroup { group, name } => {
                group_mut(doc, *group)?.name.clone_from(name);
                Ok(())
            }
            Op::RecolorGroup { group, color } => {
                group_mut(doc, *group)?.color = *color;
                Ok(())
            }
            Op::MoveGroup { from, to } => {
                let arr = groups_array_mut(doc);
                if *from >= arr.items.len() || *to >= arr.items.len() {
                    return Err(not_found("group"));
                }
                let g = arr.items.remove(*from);
                arr.items.insert(*to, g);
                Ok(())
            }
            Op::RemoveGroup { group } => {
                let uuid = group_mut(doc, *group)?.uuid.clone();
                groups_array_mut(doc).items.remove(*group);
                for el in arrangement_elements_mut(doc) {
                    remove_group_id(el, &uuid);
                }
                Ok(())
            }
            Op::AddArrangement {
                uuid,
                name,
                groups: seq,
            } => {
                let ids = group_ids(doc, seq)?;
                let mut el = RawElement::new(
                    "RVSongArrangement",
                    &[("name", name), ("uuid", uuid), ("color", "0 0 0 0")],
                );
                el.children = vec![RawNode::Element(id_array(&ids))];
                el.self_closing = false;
                arrangements_array_mut(doc)
                    .children
                    .push(RawNode::Element(el));
                arrangements_array_mut(doc).self_closing = false;
                Ok(())
            }
            Op::RenameArrangement { uuid, name } => {
                arrangement_mut(doc, uuid)?.set_attr("name", name);
                Ok(())
            }
            Op::SetArrangementGroups { uuid, groups: seq } => {
                let ids = group_ids(doc, seq)?;
                let el = arrangement_mut(doc, uuid)?;
                match find_id_array_mut(el) {
                    Some(arr) => {
                        *arr = RawElement {
                            children: id_array(&ids).children,
                            self_closing: ids.is_empty(),
                            ..arr.clone()
                        };
                    }
                    None => {
                        el.children.push(RawNode::Element(id_array(&ids)));
                        el.self_closing = false;
                    }
                }
                Ok(())
            }
            Op::RemoveArrangement { uuid } => {
                arrangement_mut(doc, uuid)?;
                if let Some(arr) = find_arrangements_mut(doc) {
                    arr.children.retain(|n| !matches!(n, RawNode::Element(e) if uuid_of(e).is_some_and(|u| u.eq_ignore_ascii_case(uuid))));
                }
                if doc
                    .selected_arrangement_id
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(uuid))
                {
                    doc.selected_arrangement_id = None;
                }
                Ok(())
            }
        }
    }
}

fn not_found(what: &str) -> EditError {
    EditError::NotFound {
        what: what.to_owned(),
    }
}

fn groups(doc: &Document) -> &[SlideGroup] {
    doc.groups()
}

fn groups_array_mut(doc: &mut Document) -> &mut TypedArray<SlideGroup> {
    doc.groups.get_or_insert_with(|| TypedArray {
        items: Vec::new(),
        extra: Extra {
            element: RawElement::new("array", &[("rvXMLIvarName", "groups")]),
            typed_slots: Vec::new(),
        },
    })
}

fn group_mut(doc: &mut Document, group: usize) -> Result<&mut SlideGroup, EditError> {
    doc.groups
        .as_mut()
        .and_then(|g| g.items.get_mut(group))
        .ok_or_else(|| not_found("group"))
}

fn slides_mut(doc: &mut Document, group: usize) -> Result<&mut TypedArray<Slide>, EditError> {
    Ok(group_mut(doc, group)?
        .slides
        .get_or_insert_with(|| TypedArray {
            items: Vec::new(),
            extra: Extra {
                element: RawElement::new("array", &[("rvXMLIvarName", "slides")]),
                typed_slots: Vec::new(),
            },
        }))
}

fn slide_mut(doc: &mut Document, group: usize, slide: usize) -> Result<&mut Slide, EditError> {
    slides_mut(doc, group)?
        .items
        .get_mut(slide)
        .ok_or_else(|| not_found("slide"))
}

fn first_text_mut(s: &mut Slide) -> Option<&mut crate::model::TextElement> {
    s.elements.as_mut()?.items.iter_mut().find_map(|e| match e {
        DisplayElement::Text(t) => Some(&mut **t),
        DisplayElement::Other(_) => None,
    })
}

fn new_group(name: &str, color: Option<Rgba>) -> SlideGroup {
    let uuid = new_uuid();
    let color_str = color
        .map(|c| format!("{} {} {} {}", c.r, c.g, c.b, c.a))
        .unwrap_or_else(|| "0 0 0 0".into());
    SlideGroup {
        name: name.to_owned(),
        color,
        uuid: uuid.clone(),
        slides: Some(TypedArray {
            items: Vec::new(),
            extra: Extra {
                element: RawElement::new("array", &[("rvXMLIvarName", "slides")]),
                typed_slots: Vec::new(),
            },
        }),
        extra: Extra {
            element: RawElement::new(
                "RVSlideGrouping",
                &[("name", name), ("uuid", &uuid), ("color", &color_str)],
            ),
            typed_slots: Vec::new(),
        },
    }
}

/// Deep copy with new UUIDs for the slide, its elements and anything below that has one.
fn fresh_copy(s: &Slide) -> Slide {
    let mut c = s.clone();
    c.uuid = new_uuid();
    refresh_raw_uuids(&mut c.extra.element);
    if let Some(e) = c.elements.as_mut() {
        for el in &mut e.items {
            match el {
                DisplayElement::Text(t) => {
                    t.uuid = new_uuid();
                    refresh_raw_uuids(&mut t.extra.element);
                }
                DisplayElement::Other(raw) => refresh_raw_uuids(raw),
            }
        }
    }
    c
}

/// Replace every `UUID`/`uuid` attribute in a raw subtree (cues, media, ...).
fn refresh_raw_uuids(el: &mut RawElement) {
    for name in ["UUID", "uuid"] {
        if el.attr(name).is_some() {
            el.set_attr(name, &new_uuid());
        }
    }
    for c in &mut el.children {
        if let RawNode::Element(e) = c {
            refresh_raw_uuids(e);
        }
    }
}

// ---- arrangements (raw XML, assumed structure; see arrangement.rs) ----------------

fn uuid_of(el: &RawElement) -> Option<String> {
    el.attr("uuid")
        .or_else(|| el.attr("UUID"))
        .and_then(|a| a.value().ok())
        .map(|v| v.into_owned())
}

fn find_arrangements_mut(doc: &mut Document) -> Option<&mut RawElement> {
    doc.extra.element.children.iter_mut().find_map(|n| match n {
        RawNode::Element(e) if e.ivar_name() == Some("arrangements") => Some(e),
        _ => None,
    })
}

fn arrangements_array_mut(doc: &mut Document) -> &mut RawElement {
    if find_arrangements_mut(doc).is_none() {
        doc.extra
            .element
            .children
            .push(RawNode::Element(RawElement::new(
                "array",
                &[("rvXMLIvarName", "arrangements")],
            )));
        doc.extra.element.self_closing = false;
    }
    #[allow(clippy::expect_used)] // inserted just above
    find_arrangements_mut(doc).expect("arrangements array exists")
}

fn arrangement_elements_mut(doc: &mut Document) -> Vec<&mut RawElement> {
    match find_arrangements_mut(doc) {
        Some(arr) => arr
            .children
            .iter_mut()
            .filter_map(|n| match n {
                RawNode::Element(e) => Some(e),
                _ => None,
            })
            .collect(),
        None => Vec::new(),
    }
}

fn arrangement_mut<'a>(doc: &'a mut Document, uuid: &str) -> Result<&'a mut RawElement, EditError> {
    arrangement_elements_mut(doc)
        .into_iter()
        .find(|e| uuid_of(e).is_some_and(|u| u.eq_ignore_ascii_case(uuid)))
        .ok_or_else(|| not_found("arrangement"))
}

fn group_ids(doc: &Document, seq: &[usize]) -> Result<Vec<String>, EditError> {
    seq.iter()
        .map(|&g| {
            doc.groups()
                .get(g)
                .map(|g| g.uuid.clone())
                .ok_or_else(|| not_found("group"))
        })
        .collect()
}

fn id_array(ids: &[String]) -> RawElement {
    let mut arr = RawElement::new("array", &[("rvXMLIvarName", "groupIDs")]);
    arr.children = ids
        .iter()
        .map(|id| {
            let mut s = RawElement::new("NSString", &[]);
            s.set_text(id);
            RawNode::Element(s)
        })
        .collect();
    arr.self_closing = ids.is_empty();
    arr
}

/// The array holding an arrangement's group IDs: the first descendant array whose
/// children are NSStrings (or that is named `groupIDs`).
fn find_id_array_mut(el: &mut RawElement) -> Option<&mut RawElement> {
    let idx = el.children.iter().position(|n| match n {
        RawNode::Element(e) => {
            e.name == "array"
                && (e.ivar_name() == Some("groupIDs")
                    || e.elements().any(|(_, c)| c.name == "NSString"))
        }
        _ => false,
    })?;
    match &mut el.children[idx] {
        RawNode::Element(e) => Some(e),
        _ => None,
    }
}

fn remove_group_id(el: &mut RawElement, group_uuid: &str) {
    el.children.retain(|n| match n {
        RawNode::Element(e) if e.name == "NSString" => e
            .text()
            .map_or(true, |t| !t.trim().eq_ignore_ascii_case(group_uuid)),
        _ => true,
    });
    for c in &mut el.children {
        if let RawNode::Element(e) = c {
            remove_group_id(e, group_uuid);
        }
    }
}
