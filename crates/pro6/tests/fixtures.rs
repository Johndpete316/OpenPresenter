#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Parse every hand-written fixture and check the facts it was written to cover.

use std::path::PathBuf;

use pretty_assertions::assert_eq;
use pro6::{CanvasSize, DisplayElement, Document, ParseErrorKind, Rgba};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn load(name: &str) -> Document {
    pro6::load(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn song_1024x768() {
    let doc = load("song_1024x768.pro6");
    assert_eq!(
        doc.canvas,
        CanvasSize {
            width: 1024,
            height: 768
        }
    );
    assert_eq!(doc.ccli_song_title, "Amazing Grace");
    assert_eq!(
        doc.selected_arrangement_id.as_deref(),
        Some("AA000000-0001-4000-8000-000000000001")
    );
    let arr = doc.arrangements();
    assert_eq!(
        arr.iter()
            .map(|a| (a.name.as_str(), a.groups.clone()))
            .collect::<Vec<_>>(),
        [("Default", vec![0, 1, 2, 1]), ("Short", vec![0, 1])]
    );
    assert_eq!(
        doc.background_color,
        Some(Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0
        })
    );

    let groups: Vec<_> = doc
        .groups()
        .iter()
        .map(|g| (g.name.as_str(), g.uuid.as_str(), g.slides().len()))
        .collect();
    assert_eq!(
        groups,
        [
            ("Verse 1", "6C3E1A52-0B2D-4C1F-9E5A-1F0D2B3C4A51", 2),
            ("Chorus", "6C3E1A52-0B2D-4C1F-9E5A-1F0D2B3C4A52", 2),
            ("Verse 2", "6C3E1A52-0B2D-4C1F-9E5A-1F0D2B3C4A53", 1),
        ]
    );
    assert_eq!(
        doc.groups()[1].color,
        Some(Rgba {
            r: 1.0,
            g: 0.2,
            b: 0.2,
            a: 1.0
        })
    );

    let enabled: Vec<_> = doc.slides().map(|s| s.enabled).collect();
    assert_eq!(enabled, [true, true, true, false, true]);
    let disabled = doc.slides().find(|s| !s.enabled).unwrap();
    assert_eq!(disabled.uuid, "A1B2C3D4-0001-4000-8000-000000000004");
    assert_eq!(disabled.label, "Skip me");

    let tl = doc.timeline.as_ref().unwrap();
    assert_eq!(
        (tl.duration, tl.loop_, tl.selected_media_track_index),
        (0.0, false, -1)
    );
    assert_eq!((tl.time_cue_count(), tl.media_track_count()), (0, 0));

    let t = doc.text_elements().next().unwrap();
    assert_eq!(t.position.value.width, 960.0);
    assert_eq!(t.position.value.height, 720.0);
    let shadow = t.shadow.as_ref().unwrap();
    assert_eq!(shadow.value.color.unwrap().a, 0.3333333432674408);
    assert_eq!(
        t.stroke.as_ref().unwrap().width.as_ref().unwrap().value,
        0.0
    );

    // Multi-line text, from both RTF and PlainText.
    let last = doc.text_elements().last().unwrap();
    let rtf = last.rtf_doc(&doc.path).unwrap().unwrap();
    assert_eq!(rtf.paragraphs.len(), 4);
    assert_eq!(
        rtf.plain_text(),
        last.plain_text.as_ref().unwrap().value.text()
    );
    for el in doc.text_elements() {
        let rtf = el.rtf_doc(&doc.path).unwrap().unwrap();
        for run in rtf.runs() {
            assert_eq!(
                (run.font.as_deref(), run.size_half_pt),
                (Some("Arial"), 110)
            );
        }
    }
    assert!(doc.warnings.is_empty(), "{:#?}", doc.warnings);
}

#[test]
fn two_fonts_1920x1080() {
    let doc = load("two_fonts_1920x1080.pro6");
    assert_eq!(
        doc.canvas,
        CanvasSize {
            width: 1920,
            height: 1080
        }
    );
    assert_eq!(doc.notes, "Two fonts & sizes in one element");
    let names: Vec<_> = doc.groups().iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["", "Ref"]);
    assert_eq!(doc.groups()[0].color.unwrap().a, 0.0);

    let welcome = doc.text_elements().next().unwrap();
    let rtf = welcome.rtf_doc(&doc.path).unwrap().unwrap();
    let runs: Vec<_> = rtf
        .runs()
        .map(|r| (r.font.as_deref().unwrap(), r.size_half_pt))
        .collect();
    assert_eq!(runs, [("Arial", 120), ("Georgia", 80)]);
    assert_eq!(
        rtf.plain_text(),
        welcome.plain_text.as_ref().unwrap().value.text()
    );
    assert_eq!(
        rtf.plain_text(),
        "Welcome to Caf\u{e9} Sunday\nDoors open at 9:30 \u{2014} see you there"
    );

    let shadow = &welcome.shadow.as_ref().unwrap().value;
    assert_eq!(
        (shadow.blur, shadow.offset),
        (Some(4.0), Some((2.82843, -2.82843)))
    );
    assert_eq!(
        welcome
            .stroke
            .as_ref()
            .unwrap()
            .width
            .as_ref()
            .unwrap()
            .value,
        2.0
    );
    assert_eq!(welcome.vertical_alignment, 0); // 0 = middle
    assert!(doc.warnings.is_empty(), "{:#?}", doc.warnings);
}

#[test]
fn unknowns_are_preserved_not_dropped() {
    let doc = load("unknowns.pro6");
    let unknown_root: Vec<_> = doc.extra.unknown_attrs().map(|a| a.name.as_str()).collect();
    assert_eq!(unknown_root, ["futureRootAttr"]);
    assert!(
        doc.extra
            .raw_children()
            .any(|e| e.name == "SyntheticRootChild")
    );
    assert_eq!(
        doc.selected_arrangement_id.as_deref(),
        Some("9F8E7D6C-0003-4000-8000-0000000000A1")
    );
    assert_eq!(doc.arrangements_raw().unwrap().elements().count(), 1);

    let tl = doc.timeline.as_ref().unwrap();
    assert!(tl.loop_);
    assert_eq!(
        (tl.duration, tl.time_cue_count(), tl.media_track_count()),
        (30.0, 1, 1)
    );

    // Non-group item in `groups` stays raw, in the array's extra.
    let groups = doc.groups.as_ref().unwrap();
    assert_eq!(groups.items.len(), 1);
    assert!(
        groups
            .extra
            .raw_children()
            .any(|e| e.name == "SyntheticGroupLike")
    );
    assert_eq!(groups.items[0].extra.unknown_attrs().count(), 1);

    let slide = doc.slides().next().unwrap();
    assert_eq!(slide.cue_count(), 1);
    match &slide.elements()[0] {
        DisplayElement::Other(raw) => assert_eq!(raw.name, "SyntheticImageElement"),
        other => panic!("expected raw element, got {other:?}"),
    }
    let text = slide.text_elements().next().unwrap();
    assert_eq!(text.opacity, 0.75);
    assert!(text.shadow.is_none() && text.stroke.is_none());
    assert!(text.extra.find_ivar("future").is_some());
    assert_eq!(
        text.extra
            .unknown_attrs()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["futureTextAttr"]
    );
    assert_eq!(text.win_flow.as_ref().unwrap().value.bytes, [0, 1, 2]);
    assert_eq!(text.win_font.as_ref().unwrap().value.bytes, [3, 4, 5]);

    // Each unexpected structure is reported, not silently accepted.
    let msgs: Vec<_> = doc.warnings.iter().map(|w| w.message.as_str()).collect();
    assert!(
        msgs.iter().any(|m| m.contains("SyntheticGroupLike")),
        "{msgs:#?}"
    );
    assert!(
        msgs.iter().any(|m| m.contains("SyntheticImageElement")),
        "{msgs:#?}"
    );
    assert!(
        msgs.iter().any(|m| m.contains("SyntheticRootChild")),
        "{msgs:#?}"
    );
}

#[test]
fn shell_and_typed_slots_recorded_for_round_trip() {
    let doc = load("song_1024x768.pro6");
    assert!(!doc.shell.has_bom);
    assert!(matches!(
        doc.shell.prolog.first(),
        Some(pro6::raw::RawNode::Decl(_))
    ));
    let slots: Vec<_> = doc.extra.typed_slots.iter().map(|(_, k)| *k).collect();
    assert_eq!(slots, ["timeline", "groups"]);
    let t = doc.text_elements().next().unwrap();
    let slots: Vec<_> = t.extra.typed_slots.iter().map(|(_, k)| *k).collect();
    assert_eq!(
        slots,
        ["position", "shadow", "stroke", "PlainText", "RTFData"]
    );
    // Raw source text kept verbatim.
    assert_eq!(
        t.shadow.as_ref().unwrap().extra.element.text().unwrap(),
        "0|0 0 0 0.3333333432674408|{4, -4}"
    );
}

fn invalid(name: &str) -> pro6::ParseError {
    let path = fixture(&format!("invalid/{name}"));
    match pro6::load(&path) {
        Err(e) => e,
        Ok(doc) => doc
            .text_elements()
            .find_map(|t| t.rtf_doc(&doc.path).err())
            .unwrap_or_else(|| panic!("{name} unexpectedly parsed")),
    }
}

#[test]
fn invalid_fixtures_fail_with_specific_errors() {
    assert!(matches!(
        invalid("truncated.pro6").kind(),
        ParseErrorKind::Xml { .. }
    ));
    assert!(matches!(
        invalid("bad_base64.pro6").kind(),
        ParseErrorKind::Base64 { .. }
    ));
    assert!(matches!(
        invalid("bad_rect.pro6").kind(),
        ParseErrorKind::BadRect { .. }
    ));
    assert!(matches!(
        invalid("wrong_root.pro6").kind(),
        ParseErrorKind::NotPro6Root { .. }
    ));
    assert!(matches!(
        invalid("missing_canvas.pro6").kind(),
        ParseErrorKind::MissingAttr { attr: "width" }
    ));
    assert!(matches!(
        invalid("unbalanced_rtf.pro6").kind(),
        ParseErrorKind::Rtf { .. }
    ));

    let e = invalid("bad_rect.pro6");
    assert_eq!(e.uuid(), Some("33333333-0000-4000-8000-000000000001"));
    assert!(
        e.xml_path()
            .ends_with("RVTextElement[0]/RVRect3D[position]"),
        "{}",
        e.xml_path()
    );
}

#[test]
fn missing_file_is_io_error() {
    let e = pro6::load(&fixture("does_not_exist.pro6")).unwrap_err();
    assert!(matches!(e.kind(), ParseErrorKind::Io(_)));
}

#[test]
fn user_messages() {
    let names = [
        "truncated.pro6",
        "bad_base64.pro6",
        "bad_rect.pro6",
        "wrong_root.pro6",
        "missing_canvas.pro6",
        "unbalanced_rtf.pro6",
    ];
    let msgs: Vec<String> = names
        .iter()
        .map(|n| format!("{n}: {}", invalid(n).user_message()))
        .collect();
    insta::assert_snapshot!(msgs.join("\n"));
}

#[test]
fn arrangements_view() {
    let doc = load("unknowns.pro6");
    let a = doc.arrangements();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].name, "Main");
    assert_eq!(a[0].groups, [0]);
    assert!(a[0].unknown_group_ids.is_empty());
    assert_eq!(
        doc.selected_arrangement().unwrap().uuid,
        "9F8E7D6C-0003-4000-8000-0000000000A1"
    );
    assert!(load("render_1024x768.pro6").arrangements().is_empty());
}

#[test]
fn slide_timers_from_slide_cues() {
    use std::time::Duration;
    let doc = load("announcements_synthetic.pro6");
    let t = doc.slide_timings().unwrap();
    let get = |n: u32| {
        t.timers
            .get(&format!("A0000000-0005-4000-8000-00000000000{n}"))
            .copied()
            .unwrap()
    };
    assert_eq!(get(1).duration, Duration::from_secs(3));
    assert_eq!(get(2).duration, Duration::from_secs(5));
    assert!(!get(3).loop_to_beginning);
    assert!(get(4).loop_to_beginning, "last slide loops back");
    assert_eq!((t.other_cues, t.with_delay), (0, 0));
    assert!(load("song_1024x768.pro6").slide_timings().is_none());
    // The synthetic cue in unknowns.pro6 isn't a timer: counted, not used.
    assert!(load("unknowns.pro6").slide_timings().is_none());
}

/// Real PP6 files (not in the repo; put them in `pp6-examples/` to run this).
#[test]
fn real_welcome_file_timers() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../pp6-examples/AA001 Welcome.pro6");
    if !path.exists() {
        eprintln!("pp6-examples not present; skipping");
        return;
    }
    let doc = pro6::load(&path).unwrap();
    let t = doc.slide_timings().unwrap();
    assert_eq!(t.timers.len(), 14);
    assert_eq!(t.timers.values().filter(|x| x.loop_to_beginning).count(), 1);
}
