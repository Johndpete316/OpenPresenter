//! Round-trip support shared by every typed node.
//!
//! Each typed node owns an [`Extra`]: its original element with the children the
//! typed model took over removed, plus a record of where those children sat.
//! Attributes are all kept, raw and in order; typed fields are a parsed view of
//! them. The writer (M1) re-emits the raw text for any value that is unchanged.

use crate::error::{Ctx, ParseError, ParseErrorKind};
use crate::raw::{RawAttr, RawElement, RawNode, segment, uuid_attr};
use crate::schema;

#[derive(Debug, Clone, PartialEq)]
pub struct Extra {
    /// Original element, minus children owned by typed fields.
    pub element: RawElement,
    /// `(original child index, slot key)` for each child the typed model owns, in order.
    pub typed_slots: Vec<(usize, &'static str)>,
}

impl Extra {
    /// Attributes not described by the spec for this element.
    pub fn unknown_attrs(&self) -> impl Iterator<Item = &RawAttr> {
        let known = schema::known_attrs(&self.element.name).unwrap_or(&[]);
        self.element
            .attrs
            .iter()
            .filter(move |a| !known.contains(&a.name.as_str()))
    }

    /// Child elements kept raw (not modelled, or not yet understood).
    pub fn raw_children(&self) -> impl Iterator<Item = &RawElement> {
        self.element.elements().map(|(_, e)| e)
    }

    /// Raw child element with the given `rvXMLIvarName`.
    pub fn find_ivar(&self, ivar: &str) -> Option<&RawElement> {
        self.raw_children().find(|e| e.ivar_name() == Some(ivar))
    }
}

/// A typed array (`<array rvXMLIvarName=...>`): parsed items plus the array's own shell.
/// Items the parser didn't recognise stay raw in `extra`, in position.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedArray<T> {
    pub items: Vec<T>,
    pub extra: Extra,
}

/// A leaf whose value is its text content (`RVRect3D`, `NSString`, `NSColor`, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct Leaf<T> {
    pub value: T,
    /// Original element including its text content.
    pub extra: Extra,
}

/// Non-fatal finding recorded while parsing (unexpected structure, assumed formats).
#[derive(Debug, Clone, PartialEq)]
pub struct Warning {
    pub xml_path: String,
    pub message: String,
}

pub(crate) struct Claimed<'a> {
    pub key: &'static str,
    pub el: RawElement,
    pub ctx: Ctx<'a>,
}

/// Slot key for array items; the only key that may be claimed more than once.
pub(crate) const ITEM: &str = "item";

/// Split `el`'s children into ones the typed model claims and the rest.
/// `claim` returns a slot key for children the caller wants. Non-`ITEM` keys are
/// claimed once; later duplicates stay raw.
pub(crate) fn split<'a>(
    mut el: RawElement,
    ctx: &Ctx<'a>,
    mut claim: impl FnMut(&RawElement) -> Option<&'static str>,
) -> (Extra, Vec<Claimed<'a>>) {
    let children = std::mem::take(&mut el.children);
    let mut typed_slots = Vec::new();
    let mut claimed: Vec<Claimed<'a>> = Vec::new();
    let mut element_index = 0usize;
    for (i, child) in children.into_iter().enumerate() {
        match child {
            RawNode::Element(c) => {
                let idx = element_index;
                element_index += 1;
                let key = claim(&c).filter(|k| *k == ITEM || !claimed.iter().any(|x| x.key == *k));
                match key {
                    Some(key) => {
                        let child_ctx = ctx.child(segment(&c, Some(idx))).with_uuid(uuid_attr(&c));
                        typed_slots.push((i, key));
                        claimed.push(Claimed {
                            key,
                            el: c,
                            ctx: child_ctx,
                        });
                    }
                    None => el.children.push(RawNode::Element(c)),
                }
            }
            other => el.children.push(other),
        }
    }
    (
        Extra {
            element: el,
            typed_slots,
        },
        claimed,
    )
}

/// Element-less split, for leaves and nodes whose children all stay raw.
pub(crate) fn keep_all(el: RawElement) -> Extra {
    Extra {
        element: el,
        typed_slots: Vec::new(),
    }
}

/// Take the claimed child with `key`, if present.
pub(crate) fn take<'a>(claimed: &mut Vec<Claimed<'a>>, key: &str) -> Option<Claimed<'a>> {
    let pos = claimed.iter().position(|c| c.key == key)?;
    Some(claimed.remove(pos))
}

// ---- attribute helpers --------------------------------------------------------

pub(crate) fn attr_str(
    el: &RawElement,
    ctx: &Ctx<'_>,
    name: &str,
) -> Result<Option<String>, ParseError> {
    let Some(a) = el.attr(name) else {
        return Ok(None);
    };
    a.value().map(|v| Some(v.into_owned())).map_err(|_| {
        ctx.err(ParseErrorKind::BadAttr {
            attr: name.to_owned(),
            value: a.raw_value.clone(),
            expected: "text with valid XML escapes",
        })
    })
}

pub(crate) fn attr_string(
    el: &RawElement,
    ctx: &Ctx<'_>,
    name: &str,
) -> Result<String, ParseError> {
    Ok(attr_str(el, ctx, name)?.unwrap_or_default())
}

/// Parse an optional attribute. Empty values count as absent.
pub(crate) fn attr_parse<T>(
    el: &RawElement,
    ctx: &Ctx<'_>,
    name: &str,
    expected: &'static str,
    parse: fn(&str) -> Option<T>,
) -> Result<Option<T>, ParseError> {
    match attr_str(el, ctx, name)? {
        None => Ok(None),
        Some(v) if v.trim().is_empty() => Ok(None),
        Some(v) => parse(v.trim()).map(Some).ok_or_else(|| {
            ctx.err(ParseErrorKind::BadAttr {
                attr: name.to_owned(),
                value: v,
                expected,
            })
        }),
    }
}

pub(crate) fn attr_bool(
    el: &RawElement,
    ctx: &Ctx<'_>,
    name: &str,
    default: bool,
) -> Result<bool, ParseError> {
    Ok(attr_parse(el, ctx, name, "true or false", parse_bool)?.unwrap_or(default))
}

pub(crate) fn attr_f64(
    el: &RawElement,
    ctx: &Ctx<'_>,
    name: &str,
    default: f64,
) -> Result<f64, ParseError> {
    Ok(attr_parse(el, ctx, name, "a number", parse_f64)?.unwrap_or(default))
}

pub(crate) fn attr_i64(
    el: &RawElement,
    ctx: &Ctx<'_>,
    name: &str,
    default: i64,
) -> Result<i64, ParseError> {
    Ok(attr_parse(el, ctx, name, "a whole number", parse_i64)?.unwrap_or(default))
}

pub(crate) fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

pub(crate) fn parse_f64(s: &str) -> Option<f64> {
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

pub(crate) fn parse_i64(s: &str) -> Option<i64> {
    s.parse().ok()
}

/// Leaf text content, unescaped and trimmed.
pub(crate) fn leaf_text(el: &RawElement, ctx: &Ctx<'_>) -> Result<String, ParseError> {
    el.text().map(|t| t.trim().to_owned()).map_err(|_| {
        ctx.err(ParseErrorKind::BadAttr {
            attr: "#text".to_owned(),
            value: String::new(),
            expected: "text with valid XML escapes",
        })
    })
}
