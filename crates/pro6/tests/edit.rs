#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Text edits regenerate RTFData + PlainText, keep UUIDs, and touch nothing else.

use std::path::PathBuf;

use pro6::diff::semantic_diff;
use pro6::rtf::Align;
use pro6::{DisplayElement, Document, EditError, WinPayloads};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn load(name: &str) -> Document {
    pro6::load(&fixture(name)).unwrap()
}

fn first_text_mut(doc: &mut Document, group: usize, slide: usize) -> &mut pro6::TextElement {
    let s = &mut doc.groups.as_mut().unwrap().items[group]
        .slides
        .as_mut()
        .unwrap()
        .items[slide];
    match &mut s.elements.as_mut().unwrap().items[0] {
        DisplayElement::Text(t) => t,
        DisplayElement::Other(_) => panic!("not text"),
    }
}

#[test]
fn set_text_regenerates_rtf_and_plain_text_only() {
    let path = fixture("song_1024x768.pro6");
    let original = load("song_1024x768.pro6");
    let mut doc = original.clone();
    first_text_mut(&mut doc, 0, 1)
        .set_text(
            "New line one\nNew {line} two \u{2019}",
            WinPayloads::Drop,
            &path,
        )
        .unwrap();

    let out = pro6::write_string(&doc);
    let back = pro6::parse_str(&out, &path).unwrap();
    let t = back.groups()[0].slides()[1].text_elements().next().unwrap();
    let rtf = t.rtf_doc(&path).unwrap().unwrap();
    assert_eq!(rtf.plain_text(), "New line one\nNew {line} two \u{2019}");
    assert_eq!(
        t.plain_text.as_ref().unwrap().value.text(),
        "New line one\nNew {line} two \u{2019}"
    );
    for run in rtf.runs() {
        assert_eq!(
            (run.font.as_deref(), run.size_half_pt),
            (Some("Arial"), 110),
            "style kept"
        );
    }
    assert!(rtf.paragraphs.iter().all(|p| p.align == Align::Center));

    // Only the two payloads of that one element changed; every UUID is identical.
    let diffs = semantic_diff(&pro6::to_raw(&original).root, &pro6::to_raw(&back).root);
    assert_eq!(diffs.len(), 2, "{diffs:#?}");
    assert!(
        diffs
            .iter()
            .all(|d| d.xml_path.ends_with("NSString[PlainText]")
                || d.xml_path.ends_with("NSString[RTFData]"))
    );
    assert!(diffs.iter().all(|d| {
        d.xml_path
            .contains("RVSlideGrouping[0]/array[slides]/RVDisplaySlide[1]")
    }));
}

#[test]
fn win_payloads_flag() {
    let path = fixture("unknowns.pro6");
    for (mode, expect_present) in [(WinPayloads::Keep, true), (WinPayloads::Drop, false)] {
        let mut doc = load("unknowns.pro6");
        let slide = &mut doc.groups.as_mut().unwrap().items[0]
            .slides
            .as_mut()
            .unwrap()
            .items[0];
        let t = slide
            .elements
            .as_mut()
            .unwrap()
            .items
            .iter_mut()
            .find_map(|e| match e {
                DisplayElement::Text(t) => Some(t),
                DisplayElement::Other(_) => None,
            })
            .unwrap();
        t.set_text("Edited", mode, &path).unwrap();
        let out = pro6::write_string(&doc);
        assert_eq!(out.contains("WinFlowData"), expect_present, "{mode:?}");
        assert_eq!(out.contains("WinFontData"), expect_present, "{mode:?}");
        // Unknown siblings survive either way.
        assert!(out.contains("<SyntheticChild rvXMLIvarName=\"future\">opaque</SyntheticChild>"));
    }
}

#[test]
fn mixed_font_element_takes_first_run_style() {
    let path = fixture("two_fonts_1920x1080.pro6");
    let mut doc = load("two_fonts_1920x1080.pro6");
    first_text_mut(&mut doc, 0, 0)
        .set_text("Plain", WinPayloads::Drop, &path)
        .unwrap();
    let back = pro6::parse_str(&pro6::write_string(&doc), &path).unwrap();
    let rtf = back
        .text_elements()
        .next()
        .unwrap()
        .rtf_doc(&path)
        .unwrap()
        .unwrap();
    let runs: Vec<_> = rtf
        .runs()
        .map(|r| (r.text.as_str(), r.font.as_deref(), r.size_half_pt))
        .collect();
    assert_eq!(runs, [("Plain", Some("Arial"), 120)]);
    assert_eq!(rtf.fonts.len(), 2, "font table kept");
}

#[test]
fn crlf_plain_text_style_is_kept_and_missing_plain_text_is_created() {
    let path = fixture("song_1024x768.pro6");
    let mut doc = load("song_1024x768.pro6");
    let t = first_text_mut(&mut doc, 0, 0);
    t.plain_text.as_mut().unwrap().value.bytes = b"a\r\nb".to_vec();
    t.set_text("x\ny", WinPayloads::Drop, &path).unwrap();
    assert_eq!(t.plain_text.as_ref().unwrap().value.bytes, b"x\r\ny");

    t.plain_text = None;
    t.set_text("z", WinPayloads::Drop, &path).unwrap();
    let back = pro6::parse_str(&pro6::write_string(&doc), &path).unwrap();
    let t = back.text_elements().next().unwrap();
    assert_eq!(t.plain_text.as_ref().unwrap().value.text(), "z");
}

#[test]
fn refuses_without_style() {
    let path = fixture("song_1024x768.pro6");
    let mut doc = load("song_1024x768.pro6");
    let t = first_text_mut(&mut doc, 0, 0);
    t.rtf = None;
    let err = t.set_text("x", WinPayloads::Drop, &path).unwrap_err();
    assert!(matches!(err, EditError::NoStyle { .. }));
    assert!(err.user_message().contains("Nothing was changed"));
    assert!(
        t.plain_text
            .as_ref()
            .unwrap()
            .value
            .text()
            .starts_with("Amazing"),
        "no partial edit"
    );
}

#[test]
fn pp6_placeholder_text_is_recognised() {
    let path = fixture("song_1024x768.pro6");
    let mut doc = load("song_1024x768.pro6");
    let t = first_text_mut(&mut doc, 0, 1);
    assert!(!t.is_placeholder(&path));
    t.set_text("Double-click to edit", WinPayloads::Drop, &path)
        .unwrap();
    assert!(t.is_placeholder(&path));
    t.plain_text = None; // falls back to the RTF text
    assert!(t.is_placeholder(&path));
    t.set_text("Double-click to edit me", WinPayloads::Drop, &path)
        .unwrap();
    assert!(!t.is_placeholder(&path));
}
