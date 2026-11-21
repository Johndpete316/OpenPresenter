#![allow(clippy::unwrap_used, clippy::expect_used)]
//! E5: copy style / paste to selection, slide, group, presentation.

use std::path::PathBuf;
use std::sync::Arc;

use doc::{Document, Edit, Element, History, Scope, Style};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// The song fixture grown to at least `n` slides.
fn presentation(n: usize) -> Document {
    let mut d = doc::load(&fixture("song_1024x768.pro6")).unwrap();
    let mut i = 0;
    while d.slides().count() < n {
        let g = i % d.groups.len();
        let at = d.groups[g].slides.len();
        Edit::AddSlide {
            group: g,
            at,
            text: format!("Line {i}\nsecond line"),
        }
        .apply(&mut d)
        .unwrap();
        i += 1;
    }
    d
}

fn first_text(d: &Document) -> (usize, usize, usize) {
    for (g, gr) in d.groups.iter().enumerate() {
        for (s, sl) in gr.slides.iter().enumerate() {
            if let Some(e) = sl
                .elements
                .iter()
                .position(|e| matches!(e, Element::Text(t) if t.runs().next().is_some()))
            {
                return (g, s, e);
            }
        }
    }
    panic!("no text box");
}

/// A distinctive style on the first box.
fn styled_source(d: &mut Document) -> Style {
    let (g, s, e) = first_text(d);
    let Element::Text(t) = &mut d.groups[g].slides[s].elements[e] else {
        unreachable!()
    };
    for p in &mut t.paragraphs {
        p.align = doc::Align::Right;
        for r in &mut p.runs {
            r.bold = true;
            r.color = Some([250, 200, 10]);
            r.size = 61.0;
        }
    }
    t.valign = doc::VAlign::Bottom;
    Style::of(&d.groups[g].slides[s].elements[e]).unwrap()
}

#[test]
fn paste_to_a_thirty_slide_presentation_is_one_undo_step() {
    let mut d = presentation(30);
    assert!(d.slides().count() >= 30);
    let style = styled_source(&mut d);
    let before = d.clone();
    let (op, changed) = doc::style::paste(&d, &style, &Scope::Presentation).unwrap();
    assert!(changed >= 29, "changed {changed}");
    assert!(matches!(op, Edit::Batch { .. }));
    assert_eq!(
        doc::check(&d, &op),
        doc::Check::Ok,
        "plain styles are .pro6-safe"
    );
    let mut h = History::default();
    h.record(op.label(), Arc::new(d.clone()));
    op.apply(&mut d).unwrap();
    for t in d.text_boxes().filter(|t| t.runs().next().is_some()) {
        assert!(
            t.runs()
                .all(|r| r.bold && r.color == Some([250, 200, 10]) && r.size == 61.0)
        );
        assert!(t.paragraphs.iter().all(|p| p.align == doc::Align::Right));
        assert_eq!(t.valign, doc::VAlign::Bottom);
    }
    let undone = h.undo(Arc::new(d.clone())).unwrap();
    assert_eq!(*undone, before, "one undo reverts all of it");
    // Text is untouched.
    let texts = |d: &Document| {
        d.text_boxes()
            .map(doc::TextBox::plain_text)
            .collect::<Vec<_>>()
    };
    assert_eq!(texts(&d), texts(&before));
}

#[test]
fn scopes_pick_the_right_elements() {
    let mut d = presentation(8);
    let style = styled_source(&mut d);
    let count = |scope: Scope| doc::style::targets(&d, &style, &scope).len();
    let (g, s, e) = first_text(&d);
    assert_eq!(count(Scope::Elements(g, s, vec![e])), 1);
    assert_eq!(
        count(Scope::Slide(g, s)),
        d.groups[g].slides[s].text_boxes().count()
    );
    let in_group: usize = d.groups[g]
        .slides
        .iter()
        .map(|sl| sl.text_boxes().count())
        .sum();
    assert_eq!(count(Scope::Group(g)), in_group);
    assert_eq!(count(Scope::Presentation), d.text_boxes().count());
    // Already styled: pasting onto the source itself changes nothing.
    assert!(doc::style::paste(&d, &style, &Scope::Elements(g, s, vec![e])).is_none());
}

#[test]
fn effects_in_a_style_ask_first_in_pro6_mode() {
    let mut d = presentation(3);
    let (g, s, e) = first_text(&d);
    if let Element::Text(t) = &mut d.groups[g].slides[s].elements[e] {
        t.outline = Some(doc::Outline {
            width: 2.0,
            color: doc::Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        });
    }
    let style = Style::of(&d.groups[g].slides[s].elements[e]).unwrap();
    let (op, _) = doc::style::paste(&d, &style, &Scope::Presentation).unwrap();
    assert!(matches!(
        doc::check(&d, &op),
        doc::Check::NeedsNative { .. }
    ));
}

#[test]
fn image_styles_only_go_on_images() {
    let d = presentation(2);
    let style = Style::Image(doc::style::ImageStyle {
        adjust: doc::Adjust {
            brightness: 0.2,
            ..doc::Adjust::default()
        },
        opacity: 0.5,
    });
    assert!(doc::style::targets(&d, &style, &Scope::Presentation).is_empty());
}
