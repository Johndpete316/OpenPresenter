//! Read-only view of `arrangements`.
//!
//! **Assumed structure** (unverified; docs/PLAN.md Unknowns): each child of the
//! `arrangements` array is one arrangement with `uuid` (or `UUID`), `name` and
//! `color` attributes, containing (somewhere below it) `NSString` elements whose
//! text is the UUID of a slide group, in play order, repeats allowed (PP6 commonly
//! writes `<RVSongArrangement ...><array rvXMLIvarName="groupIDs"><NSString>…`).
//! The parse is tolerant: element names aren't checked, and IDs that don't match
//! a group are reported, not guessed. The raw XML is untouched, so round-trips
//! are unaffected.

use crate::model::{Document, Rgba};
use crate::raw::{RawElement, RawNode};

#[derive(Debug, Clone, PartialEq)]
pub struct Arrangement {
    pub uuid: String,
    pub name: String,
    pub color: Option<Rgba>,
    /// Indices into `Document::groups()`, in play order.
    pub groups: Vec<usize>,
    /// Group IDs listed by the arrangement that match no group.
    pub unknown_group_ids: Vec<String>,
}

impl Document {
    /// Arrangements, parsed on demand from the preserved raw XML.
    pub fn arrangements(&self) -> Vec<Arrangement> {
        let Some(arr) = self.arrangements_raw() else {
            return Vec::new();
        };
        arr.elements()
            .filter_map(|(_, el)| {
                let attr = |n: &str| {
                    el.attr(n)
                        .and_then(|a| a.value().ok())
                        .map(|v| v.into_owned())
                };
                let uuid = attr("uuid")
                    .or_else(|| attr("UUID"))
                    .filter(|u| !u.is_empty())?;
                let mut ids = Vec::new();
                collect_strings(el, &mut ids);
                let mut groups = Vec::new();
                let mut unknown_group_ids = Vec::new();
                for id in ids {
                    match self
                        .groups()
                        .iter()
                        .position(|g| g.uuid.eq_ignore_ascii_case(&id))
                    {
                        Some(i) => groups.push(i),
                        None => unknown_group_ids.push(id),
                    }
                }
                Some(Arrangement {
                    uuid,
                    name: attr("name").unwrap_or_default(),
                    color: attr("color").as_deref().and_then(Rgba::parse),
                    groups,
                    unknown_group_ids,
                })
            })
            .collect()
    }

    /// The arrangement `selectedArrangementID` points at, if it exists.
    pub fn selected_arrangement(&self) -> Option<Arrangement> {
        let id = self.selected_arrangement_id.as_deref()?;
        self.arrangements()
            .into_iter()
            .find(|a| a.uuid.eq_ignore_ascii_case(id))
    }
}

fn collect_strings(el: &RawElement, out: &mut Vec<String>) {
    for c in &el.children {
        if let RawNode::Element(e) = c {
            if e.name == "NSString" {
                if let Ok(t) = e.text() {
                    let t = t.trim();
                    if !t.is_empty() {
                        out.push(t.to_owned());
                    }
                }
            } else {
                collect_strings(e, out);
            }
        }
    }
}
