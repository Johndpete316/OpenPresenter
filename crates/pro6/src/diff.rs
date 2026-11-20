//! Semantic comparison of two raw trees: what PP6 would read, ignoring formatting.
//!
//! Compared: element names and order, attribute sets (unescaped values, order
//! ignored), and text content (whitespace-only text ignored, surrounding whitespace
//! trimmed, base64 `NSString` payloads compared by decoded bytes). Not compared:
//! comments, processing instructions, the XML declaration, quoting and indentation.

use std::collections::BTreeMap;

use base64::Engine as _;

use crate::raw::{RawElement, RawNode, segment};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    /// Element path in the first tree.
    pub xml_path: String,
    pub message: String,
}

impl std::fmt::Display for Diff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.xml_path, self.message)
    }
}

/// Differences between `a` (original) and `b` (rewritten), in document order.
pub fn semantic_diff(a: &RawElement, b: &RawElement) -> Vec<Diff> {
    let mut out = Vec::new();
    compare(a, b, segment(a, None), &mut out);
    out
}

fn compare(a: &RawElement, b: &RawElement, path: String, out: &mut Vec<Diff>) {
    let mut diff = |message: String| {
        out.push(Diff {
            xml_path: path.clone(),
            message,
        })
    };
    if a.name != b.name {
        diff(format!("element <{}> became <{}>", a.name, b.name));
        return;
    }

    let attrs = |e: &RawElement| -> BTreeMap<String, String> {
        e.attrs
            .iter()
            .map(|x| {
                (
                    x.name.clone(),
                    x.value()
                        .map_or_else(|_| x.raw_value.clone(), |v| v.into_owned()),
                )
            })
            .collect()
    };
    let (aa, ba) = (attrs(a), attrs(b));
    for (k, v) in &aa {
        match ba.get(k) {
            None => diff(format!("attribute `{k}` removed (was {v:?})")),
            Some(w) if w != v => diff(format!("attribute `{k}` changed {v:?} → {w:?}")),
            _ => {}
        }
    }
    for (k, w) in &ba {
        if !aa.contains_key(k) {
            diff(format!("attribute `{k}` added ({w:?})"));
        }
    }

    let (at, bt) = (text_of(a), text_of(b));
    if at != bt {
        let both_b64 = a.name == "NSString" && decode(&at).is_some() && decode(&bt).is_some();
        if !both_b64 || decode(&at) != decode(&bt) {
            let what = if both_b64 { "decoded payload" } else { "text" };
            diff(format!(
                "{what} changed {} → {}",
                preview(&at),
                preview(&bt)
            ));
        }
    }

    let ae: Vec<&RawElement> = a.elements().map(|(_, e)| e).collect();
    let be: Vec<&RawElement> = b.elements().map(|(_, e)| e).collect();
    if ae.len() != be.len() {
        diff(format!("child element count {} → {}", ae.len(), be.len()));
    }
    for (i, (x, y)) in ae.iter().zip(&be).enumerate() {
        compare(x, y, format!("{path}/{}", segment(x, Some(i))), out);
    }
}

fn text_of(e: &RawElement) -> String {
    let mut s = String::new();
    for c in &e.children {
        match c {
            RawNode::Text(t) => s.push_str(
                &quick_xml::escape::unescape(t).map_or_else(|_| t.clone(), |v| v.into_owned()),
            ),
            RawNode::CData(t) => s.push_str(t),
            _ => {}
        }
    }
    s.trim().to_owned()
}

fn decode(s: &str) -> Option<Vec<u8>> {
    let compact: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    base64::engine::general_purpose::STANDARD_PAD_INDIFFERENT
        .decode(compact.as_bytes())
        .ok()
}

fn preview(s: &str) -> String {
    const MAX: usize = 60;
    if s.chars().count() <= MAX {
        format!("{s:?}")
    } else {
        format!("{:?}…", s.chars().take(MAX).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Ctx;
    use std::path::Path;

    fn root(s: &str) -> RawElement {
        crate::raw::parse(s, &Ctx::root(Path::new("t")))
            .unwrap()
            .root
    }

    #[test]
    fn formatting_is_not_a_difference() {
        let a = root(
            "<a x='1' y=\"&amp;\">\n  <b>hi</b>\n  <!-- c -->\n  <NSString>aGk=</NSString></a>",
        );
        let b = root("<a y=\"&#38;\" x=\"1\"><b> hi </b><NSString>aG\nk=</NSString></a>");
        assert_eq!(semantic_diff(&a, &b), vec![]);
    }

    #[test]
    fn reports_real_changes_with_paths() {
        let a = root(
            "<r><array rvXMLIvarName=\"g\"><s UUID=\"1\" n=\"a\"/><s/></array><NSString>aGk=</NSString></r>",
        );
        let b = root(
            "<r><array rvXMLIvarName=\"g\"><s UUID=\"2\" m=\"b\"/></array><NSString>aG8=</NSString></r>",
        );
        let d: Vec<String> = semantic_diff(&a, &b)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            d,
            [
                "r/array[g]: child element count 2 → 1",
                "r/array[g]/s[0]: attribute `UUID` changed \"1\" → \"2\"",
                "r/array[g]/s[0]: attribute `n` removed (was \"a\")",
                "r/array[g]/s[0]: attribute `m` added (\"b\")",
                "r/NSString[1]: decoded payload changed \"aGk=\" → \"aG8=\"",
            ]
        );
    }
}
