#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Editor operations, checked through a full write → parse round trip.

use std::collections::HashSet;
use std::path::PathBuf;

use pro6::ops::{Op, new_uuid};
use pro6::{Document, EditError, Rgba};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn song() -> Document {
    pro6::load(&fixture("song_1024x768.pro6")).unwrap()
}

/// Write and re-read; also checks the output is stable.
fn reload(doc: &Document) -> Document {
    let out = pro6::write_string(doc);
    let back = pro6::parse_str(&out, &doc.path).unwrap();
    assert_eq!(pro6::write_string(&back), out, "write is stable");
    back
}

fn text(doc: &Document, g: usize, s: usize) -> String {
    let t = doc.groups()[g].slides()[s].text_elements().next().unwrap();
    t.rtf_doc(&doc.path).unwrap().unwrap().plain_text()
}

fn all_uuids(doc: &Document) -> Vec<String> {
    let mut v: Vec<String> = doc.groups().iter().map(|g| g.uuid.clone()).collect();
    v.extend(doc.slides().map(|s| s.uuid.clone()));
    v.extend(doc.text_elements().map(|t| t.uuid.clone()));
    v
}

fn assert_unique(doc: &Document) {
    let v = all_uuids(doc);
    let set: HashSet<_> = v.iter().collect();
    assert_eq!(set.len(), v.len(), "duplicate UUIDs: {v:?}");
}

fn names(doc: &Document) -> Vec<(String, usize)> {
    doc.groups()
        .iter()
        .map(|g| (g.name.clone(), g.slides().len()))
        .collect()
}

#[test]
fn set_text_add_duplicate_remove_move_slides() {
    let mut d = song();
    Op::SetText {
        group: 0,
        slide: 0,
        text: "New first line\nSecond".into(),
    }
    .apply(&mut d)
    .unwrap();
    Op::AddSlide {
        group: 1,
        at: 2,
        text: "Added chorus line".into(),
    }
    .apply(&mut d)
    .unwrap();
    Op::DuplicateSlide { group: 0, slide: 1 }
        .apply(&mut d)
        .unwrap();
    Op::MoveSlide {
        group: 2,
        slide: 0,
        to_group: 1,
        to: 0,
    }
    .apply(&mut d)
    .unwrap();
    Op::RemoveSlide { group: 0, slide: 2 }
        .apply(&mut d)
        .unwrap();
    let back = reload(&d);
    assert_eq!(
        names(&back),
        [
            ("Verse 1".into(), 2),
            ("Chorus".into(), 4),
            ("Verse 2".into(), 0)
        ]
    );
    assert_eq!(text(&back, 0, 0), "New first line\nSecond");
    assert!(
        text(&back, 1, 0).starts_with("Through many dangers"),
        "moved from Verse 2"
    );
    assert_eq!(text(&back, 1, 3), "Added chorus line");
    // The new slide copies its neighbour's font/size and starts enabled even after a disabled one.
    let added = &back.groups()[1].slides()[3];
    assert!(added.enabled);
    let run = added
        .text_elements()
        .next()
        .unwrap()
        .rtf_doc(&back.path)
        .unwrap()
        .unwrap()
        .runs()
        .next()
        .unwrap()
        .clone();
    assert_eq!(
        (run.font.as_deref(), run.size_half_pt),
        (Some("Arial"), 110)
    );
    assert_unique(&back);
}

#[test]
fn groups_create_rename_recolor_move_remove_and_arrangements_follow() {
    let mut d = song();
    let red = Some(Rgba {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    Op::AddGroup {
        at: 2,
        name: "Bridge".into(),
        color: red,
    }
    .apply(&mut d)
    .unwrap();
    Op::AddSlide {
        group: 2,
        at: 0,
        text: "Bridge text".into(),
    }
    .apply(&mut d)
    .unwrap();
    Op::RenameGroup {
        group: 0,
        name: "V1".into(),
    }
    .apply(&mut d)
    .unwrap();
    Op::RecolorGroup {
        group: 3,
        color: None,
    }
    .apply(&mut d)
    .unwrap();
    Op::MoveGroup { from: 3, to: 0 }.apply(&mut d).unwrap();
    let back = reload(&d);
    assert_eq!(
        names(&back),
        [
            ("Verse 2".into(), 1),
            ("V1".into(), 2),
            ("Chorus".into(), 2),
            ("Bridge".into(), 1)
        ]
    );
    assert_eq!(back.groups()[3].color, red);
    assert_eq!(back.groups()[0].color, None);
    assert_eq!(text(&back, 3, 0), "Bridge text");
    assert_unique(&back);
    // Arrangements reference groups by UUID, so they follow renames and moves.
    let a = back.arrangements();
    let seq = |name: &str| {
        a.iter()
            .find(|x| x.name == name)
            .unwrap()
            .groups
            .iter()
            .map(|&g| back.groups()[g].name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(seq("Default"), ["V1", "Chorus", "Verse 2", "Chorus"]);

    // Removing a group removes it from every arrangement.
    let mut d = back.clone();
    Op::RemoveGroup { group: 2 }.apply(&mut d).unwrap();
    let back = reload(&d);
    let a = back.arrangements();
    let seq = |name: &str| {
        a.iter()
            .find(|x| x.name == name)
            .unwrap()
            .groups
            .iter()
            .map(|&g| back.groups()[g].name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(seq("Default"), ["V1", "Verse 2"]);
    assert_eq!(seq("Short"), ["V1"]);
    assert!(a.iter().all(|x| x.unknown_group_ids.is_empty()));
}

#[test]
fn arrangements_add_rename_reorder_remove() {
    let mut d = song();
    let id = new_uuid();
    Op::AddArrangement {
        uuid: id.clone(),
        name: "Bridge first".into(),
        groups: vec![2, 0, 1],
    }
    .apply(&mut d)
    .unwrap();
    Op::RenameArrangement {
        uuid: id.clone(),
        name: "Verse 2 first".into(),
    }
    .apply(&mut d)
    .unwrap();
    Op::SetArrangementGroups {
        uuid: id.clone(),
        groups: vec![2, 1, 1],
    }
    .apply(&mut d)
    .unwrap();
    let back = reload(&d);
    let a = back.arrangements();
    let mine = a.iter().find(|x| x.uuid == id).unwrap();
    assert_eq!(
        (mine.name.as_str(), mine.groups.clone()),
        ("Verse 2 first", vec![2, 1, 1])
    );
    assert_eq!(a.len(), 3);

    // Removing the selected arrangement clears the selection.
    let mut d = back;
    Op::RemoveArrangement {
        uuid: "AA000000-0001-4000-8000-000000000001".into(),
    }
    .apply(&mut d)
    .unwrap();
    let back = reload(&d);
    assert_eq!(back.selected_arrangement_id, None);
    assert_eq!(back.arrangements().len(), 2);

    // A file without an arrangements list gets one.
    let mut r = pro6::load(&fixture("render_1024x768.pro6")).unwrap();
    Op::AddArrangement {
        uuid: new_uuid(),
        name: "Main".into(),
        groups: vec![0, 0],
    }
    .apply(&mut r)
    .unwrap();
    assert_eq!(reload(&r).arrangements()[0].groups, [0, 0]);
}

#[test]
fn failed_edits_change_nothing() {
    let d = song();
    for op in [
        Op::RemoveSlide { group: 0, slide: 9 },
        Op::SetText {
            group: 7,
            slide: 0,
            text: "x".into(),
        },
        Op::MoveSlide {
            group: 0,
            slide: 0,
            to_group: 1,
            to: 5,
        },
        Op::AddSlide {
            group: 0,
            at: 9,
            text: "x".into(),
        },
        Op::MoveGroup { from: 0, to: 9 },
        Op::RenameArrangement {
            uuid: "nope".into(),
            name: "x".into(),
        },
        Op::SetArrangementGroups {
            uuid: "AA000000-0001-4000-8000-000000000001".into(),
            groups: vec![0, 42],
        },
    ] {
        let mut c = d.clone();
        let err = op.apply(&mut c).unwrap_err();
        assert!(matches!(err, EditError::NotFound { .. }), "{op:?}: {err:?}");
        assert!(err.user_message().contains("Nothing was changed"));
        assert_eq!(c, d, "{op:?} must not partially apply");
    }
    // An empty presentation has nothing to copy a layout from.
    let mut empty = d.clone();
    for _ in 0..3 {
        Op::RemoveGroup { group: 0 }.apply(&mut empty).unwrap();
    }
    Op::AddGroup {
        at: 0,
        name: "New".into(),
        color: None,
    }
    .apply(&mut empty)
    .unwrap();
    assert!(matches!(
        Op::AddSlide {
            group: 0,
            at: 0,
            text: "x".into()
        }
        .apply(&mut empty),
        Err(EditError::NoTemplate)
    ));
}
