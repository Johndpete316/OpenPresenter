//! Lossless XML tree.
//!
//! Every byte of the source that matters for re-emitting the document is kept:
//! attribute source text (quoting and spacing), escaped text exactly as written,
//! whitespace between elements, comments, the XML declaration and a BOM.
//! The typed model is built on top of this tree and never discards it.

use std::borrow::Cow;

use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::error::{Ctx, ParseError, ParseErrorKind};

/// A parsed XML document with everything outside the root element preserved.
#[derive(Debug, Clone, PartialEq)]
pub struct RawDocument {
    pub has_bom: bool,
    /// Nodes before the root element (declaration, comments, whitespace).
    pub prolog: Vec<RawNode>,
    pub root: RawElement,
    /// Nodes after the root element.
    pub epilog: Vec<RawNode>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RawNode {
    Element(RawElement),
    /// Text content in its original escaped form (entity references included).
    Text(String),
    CData(String),
    Comment(String),
    /// Contents of `<?xml ...?>` without the delimiters.
    Decl(String),
    /// Processing instruction contents without the delimiters.
    PI(String),
    DocType(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawElement {
    pub name: String,
    pub attrs: Vec<RawAttr>,
    /// The attribute portion of the start tag exactly as written, for byte-exact re-emit.
    pub attrs_src: String,
    /// Written as `<x/>` rather than `<x></x>`.
    pub self_closing: bool,
    pub children: Vec<RawNode>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawAttr {
    pub name: String,
    /// Value as written in the file, still XML-escaped.
    pub raw_value: String,
}

impl RawAttr {
    /// Unescaped attribute value.
    pub fn value(&self) -> Result<Cow<'_, str>, quick_xml::escape::EscapeError> {
        quick_xml::escape::unescape(&self.raw_value)
    }
}

impl RawElement {
    pub fn attr(&self, name: &str) -> Option<&RawAttr> {
        self.attrs.iter().find(|a| a.name == name)
    }

    /// `rvXMLIvarName`: PP6's name for the slot this element fills in its parent.
    pub fn ivar_name(&self) -> Option<&str> {
        self.attr("rvXMLIvarName").map(|a| a.raw_value.as_str())
    }

    /// Child elements with their index among all children (not only elements).
    pub fn elements(&self) -> impl Iterator<Item = (usize, &RawElement)> {
        self.children
            .iter()
            .enumerate()
            .filter_map(|(i, n)| match n {
                RawNode::Element(e) => Some((i, e)),
                _ => None,
            })
    }

    /// Concatenated, unescaped text and CDATA content of direct children.
    pub fn text(&self) -> Result<String, quick_xml::escape::EscapeError> {
        let mut out = String::new();
        for child in &self.children {
            match child {
                RawNode::Text(t) => out.push_str(&quick_xml::escape::unescape(t)?),
                RawNode::CData(t) => out.push_str(t),
                _ => {}
            }
        }
        Ok(out)
    }
}

/// Build a lossless tree from XML source. `ctx` supplies the file path for errors.
pub(crate) fn parse(src: &str, ctx: &Ctx<'_>) -> Result<RawDocument, ParseError> {
    let (has_bom, body) = match src.strip_prefix('\u{feff}') {
        Some(rest) => (true, rest),
        None => (false, src),
    };
    let mut reader = Reader::from_str(body);
    let cfg = reader.config_mut();
    cfg.trim_text(false);
    cfg.expand_empty_elements = false;
    cfg.check_end_names = true;

    let mut prolog = Vec::new();
    let mut epilog = Vec::new();
    let mut root: Option<RawElement> = None;
    // Open elements; the last is the innermost.
    let mut stack: Vec<RawElement> = Vec::new();

    let xml_err = |reader: &Reader<&[u8]>, stack: &[RawElement], message: String| {
        let offset = usize::try_from(reader.error_position()).unwrap_or(usize::MAX);
        let (line, column) = line_col(body, offset);
        stack_ctx(ctx, stack).err(ParseErrorKind::Xml {
            message,
            line,
            column,
        })
    };

    loop {
        let event = reader
            .read_event()
            .map_err(|e| xml_err(&reader, &stack, e.to_string()))?;
        let node = match event {
            Event::Eof => break,
            Event::Start(start) => {
                stack.push(start_element(&start, false, &reader, &stack, &xml_err)?);
                continue;
            }
            Event::Empty(start) => {
                RawNode::Element(start_element(&start, true, &reader, &stack, &xml_err)?)
            }
            Event::End(_) => {
                // check_end_names guarantees the end tag matches the innermost open element.
                let Some(done) = stack.pop() else {
                    return Err(xml_err(&reader, &stack, "unexpected closing tag".into()));
                };
                RawNode::Element(done)
            }
            Event::Text(t) => RawNode::Text(t.into_inner().into_owned()),
            Event::GeneralRef(r) => RawNode::Text(format!("&{};", r.into_inner())),
            Event::CData(t) => RawNode::CData(t.into_inner().into_owned()),
            Event::Comment(t) => RawNode::Comment(t.into_inner().into_owned()),
            Event::Decl(d) => RawNode::Decl(d.to_string()),
            Event::PI(p) => RawNode::PI(p.to_string()),
            Event::DocType(t) => RawNode::DocType(t.into_inner().into_owned()),
        };

        if let Some(parent) = stack.last_mut() {
            push_child(parent, node);
        } else if let RawNode::Element(el) = node {
            if root.is_some() {
                return Err(xml_err(
                    &reader,
                    &stack,
                    "more than one root element".into(),
                ));
            }
            root = Some(el);
        } else if root.is_none() {
            prolog.push(node);
        } else {
            epilog.push(node);
        }
    }

    if !stack.is_empty() {
        let open = stack
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>()
            .join(" > ");
        return Err(xml_err(
            &reader,
            &stack,
            format!("file ends before these elements are closed: {open}"),
        ));
    }
    let Some(root) = root else {
        return Err(xml_err(&reader, &stack, "no root element".into()));
    };
    Ok(RawDocument {
        has_bom,
        prolog,
        root,
        epilog,
    })
}

/// Merge adjacent text pieces (quick-xml splits text around entity references).
fn push_child(parent: &mut RawElement, node: RawNode) {
    if let RawNode::Text(t) = &node
        && let Some(RawNode::Text(prev)) = parent.children.last_mut()
    {
        prev.push_str(t);
        return;
    }
    parent.children.push(node);
}

fn start_element<F>(
    start: &quick_xml::events::BytesStart<'_>,
    self_closing: bool,
    reader: &Reader<&[u8]>,
    stack: &[RawElement],
    xml_err: &F,
) -> Result<RawElement, ParseError>
where
    F: Fn(&Reader<&[u8]>, &[RawElement], String) -> ParseError,
{
    let name = start.name().0.to_owned();
    let mut attrs = Vec::new();
    for attr in start.attributes() {
        let attr = attr.map_err(|e| xml_err(reader, stack, format!("in <{name}>: {e}")))?;
        attrs.push(RawAttr {
            name: attr.key.0.to_owned(),
            raw_value: attr.value.into_owned(),
        });
    }
    Ok(RawElement {
        attrs_src: start.attributes_raw().to_owned(),
        name,
        attrs,
        self_closing,
        children: Vec::new(),
    })
}

/// Path context for the currently open element stack.
fn stack_ctx<'a>(ctx: &Ctx<'a>, stack: &[RawElement]) -> Ctx<'a> {
    let mut c = ctx.clone();
    for (depth, el) in stack.iter().enumerate() {
        // An open element is not yet in its parent's children, so its sibling
        // index equals the number of elements the parent already holds.
        let index = depth
            .checked_sub(1)
            .and_then(|p| stack.get(p))
            .map(|parent| parent.elements().count());
        c = c.child(segment(el, index));
        c = c.with_uuid(uuid_attr(el));
    }
    c
}

/// Path segment: `name` for the root, `name[ivar]` when the element has an
/// `rvXMLIvarName`, otherwise `name[index]` (0-based among element siblings).
pub(crate) fn segment(el: &RawElement, index: Option<usize>) -> String {
    match (el.ivar_name(), index) {
        (Some(ivar), _) => format!("{}[{ivar}]", el.name),
        (None, Some(i)) => format!("{}[{i}]", el.name),
        (None, None) => el.name.clone(),
    }
}

/// `UUID` or `uuid` attribute (PP6 uses both spellings).
pub(crate) fn uuid_attr(el: &RawElement) -> Option<&str> {
    el.attr("UUID")
        .or_else(|| el.attr("uuid"))
        .map(|a| a.raw_value.as_str())
}

/// 1-based line and column for a byte offset.
fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(src.len());
    let before = src.get(..offset).unwrap_or(src);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rfind('\n')
        .map_or(before.len(), |nl| before.len() - nl - 1)
        + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn p(src: &str) -> Result<RawDocument, ParseError> {
        parse(src, &Ctx::root(Path::new("t.pro6")))
    }

    #[test]
    fn keeps_whitespace_entities_and_attr_source() {
        let src = "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<a  x='1' y=\"&amp;\">\r\n  <b>t &lt; u</b><c/>\r\n</a>\n";
        let doc = p(src).unwrap();
        assert!(doc.has_bom);
        assert_eq!(doc.prolog.len(), 2); // decl + CRLF
        assert_eq!(doc.root.attrs_src, "  x='1' y=\"&amp;\"");
        assert_eq!(doc.root.attr("y").unwrap().value().unwrap(), "&");
        let b = doc.root.elements().next().unwrap().1;
        assert_eq!(b.children, vec![RawNode::Text("t &lt; u".into())]);
        assert_eq!(b.text().unwrap(), "t < u");
        let (_, c) = doc.root.elements().nth(1).unwrap();
        assert!(c.self_closing);
        assert_eq!(doc.root.children.len(), 4); // ws, b, c, ws
        assert_eq!(doc.epilog, vec![RawNode::Text("\n".into())]);
    }

    #[test]
    fn truncated_file_reports_open_path() {
        let err = p("<RVPresentationDocument><array rvXMLIvarName=\"groups\"><x>").unwrap_err();
        assert!(matches!(err.kind(), ParseErrorKind::Xml { .. }), "{err}");
        assert_eq!(err.xml_path(), "RVPresentationDocument/array[groups]/x[0]");
    }

    #[test]
    fn mismatched_end_tag_is_error() {
        let err = p("<a><b></a>").unwrap_err();
        assert!(matches!(err.kind(), ParseErrorKind::Xml { .. }));
    }

    #[test]
    fn line_col_is_one_based() {
        assert_eq!(line_col("ab\ncd", 4), (2, 2));
        assert_eq!(line_col("ab", 0), (1, 1));
    }
}
