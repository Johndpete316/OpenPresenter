#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Regression test for the M7 editor behaviour, kept through the editor redesign
//! (docs/editor-audit.md): open → edit text → reorder slides and groups → edit an
//! arrangement → save → reopen. Written against the M7 `pro6` operations first; now
//! runs the same flow through the native model (`doc`), which the app uses, with the
//! real save path (backup + atomic write) and a fresh parse from disk.

use std::path::PathBuf;

use doc::{Edit as Op, new_uuid};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn text(doc: &doc::Document, g: usize, s: usize) -> String {
    doc.groups()[g].slides()[s].plain_text()
}

/// Everything the M7 editor can change, in a comparable form.
/// (uuid, text, enabled)
type SlideSummary = (String, String, bool);
/// (uuid, name, colour, slides)
type GroupSummary = (String, String, Option<[u8; 3]>, Vec<SlideSummary>);

#[derive(Debug, PartialEq)]
struct Summary {
    groups: Vec<GroupSummary>,
    arrangements: Vec<(String, String, Vec<String>)>,
}

fn summary(doc: &doc::Document) -> Summary {
    let c = |c: &doc::Rgba| {
        let b = |v: f64| (v * 255.0).round() as u8;
        [b(c.r), b(c.g), b(c.b)]
    };
    let groups = doc
        .groups()
        .iter()
        .enumerate()
        .map(|(gi, g)| {
            let slides = g
                .slides()
                .iter()
                .enumerate()
                .map(|(si, s)| {
                    let t = s
                        .text_boxes()
                        .next()
                        .map(|_| text(doc, gi, si))
                        .unwrap_or_default();
                    (s.uuid.clone(), t, s.enabled)
                })
                .collect();
            (
                g.uuid.clone(),
                g.name.clone(),
                g.color.as_ref().map(c),
                slides,
            )
        })
        .collect();
    let arrangements = doc
        .arrangements()
        .into_iter()
        .map(|a| {
            let ids = a
                .groups
                .iter()
                .map(|&g| doc.groups()[g].uuid.clone())
                .collect();
            (a.uuid, a.name, ids)
        })
        .collect();
    Summary {
        groups,
        arrangements,
    }
}

#[test]
fn open_edit_reorder_arrange_save_reopen() {
    let dir = std::env::temp_dir().join(format!("openpresenter-editor-regression-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("song.pro6");
    std::fs::copy(fixture("song_1024x768.pro6"), &file).unwrap();

    let mut doc = doc::open(&file).unwrap();
    let before = summary(&doc);
    assert_eq!(doc.groups().len(), 3);
    let g0_uuid = doc.groups()[0].uuid.clone();
    let g1_uuid = doc.groups()[1].uuid.clone();
    let first_slide = doc.groups()[0].slides()[0].uuid.clone();
    let second_slide = doc.groups()[0].slides()[1].uuid.clone();

    let arr = new_uuid();
    let ops = [
        Op::SetText {
            group: 0,
            slide: 0,
            text: "Edited line one\nEdited line two".into(),
        },
        // Swap the first two slides of group 0.
        Op::MoveSlide {
            group: 0,
            slide: 0,
            to_group: 0,
            to: 1,
        },
        // Move group 0 below group 1.
        Op::MoveGroup { from: 0, to: 1 },
        Op::RenameGroup {
            group: 2,
            name: "Bridge".into(),
        },
        Op::AddArrangement {
            uuid: arr.clone(),
            name: "Sunday".into(),
            groups: vec![0, 1, 0],
        },
        Op::SetArrangementGroups {
            uuid: arr.clone(),
            groups: vec![1, 0, 1, 2],
        },
    ];
    for op in &ops {
        op.apply(&mut doc).unwrap();
    }
    let edited = summary(&doc);
    assert_ne!(edited, before);

    doc::save(&doc).unwrap();
    let back = doc::load(&file).unwrap();
    assert_eq!(summary(&back), edited, "reopened file matches the edits");

    // Spot checks on what the edits did.
    assert_eq!(back.groups()[0].uuid, g1_uuid);
    assert_eq!(back.groups()[1].uuid, g0_uuid);
    assert_eq!(back.groups()[1].slides()[0].uuid, second_slide);
    assert_eq!(back.groups()[1].slides()[1].uuid, first_slide);
    assert_eq!(text(&back, 1, 1), "Edited line one\nEdited line two");
    assert_eq!(back.groups()[2].name, "Bridge");
    let a = back
        .arrangements()
        .into_iter()
        .find(|a| a.uuid == arr)
        .unwrap();
    assert_eq!(a.name, "Sunday");
    assert_eq!(a.groups, vec![1, 0, 1, 2]);

    // The backup is the untouched original.
    let bak = doc::load(&dir.join("song.pro6.bak")).unwrap();
    assert_eq!(summary(&bak), before);
    assert_eq!(
        std::fs::read(dir.join("song.pro6.bak")).unwrap(),
        std::fs::read(fixture("song_1024x768.pro6")).unwrap()
    );

    // Saving again without changes is stable.
    doc::save(&back).unwrap();
    assert_eq!(summary(&doc::load(&file).unwrap()), edited);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// E1 acceptance: open a fixture, rearrange slides and groups, move a text box,
/// save, reopen: identical. Through the core, as the editor does it.
#[test]
fn e1_rearrange_move_save_reopen_identical() {
    use std::sync::Arc;
    use std::time::Duration;

    use opcore::{Command, CoreConfig, ShowState};

    let dir = std::env::temp_dir().join(format!("openpresenter-e1-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("song.pro6");
    std::fs::copy(fixture("song_1024x768.pro6"), &file).unwrap();
    let (core, _join) = opcore::spawn(CoreConfig::default()).unwrap();
    let rx = core.subscribe(None);
    let wait = |what: &str, pred: &dyn Fn(&ShowState) -> bool| -> Arc<ShowState> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let s = rx
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
            if pred(&s) {
                return s;
            }
        }
    };
    core.send(Command::Open(file.clone()));
    wait("open", &|s| s.docs.contains_key(&file));
    let edits = [
        Op::MoveSlide {
            group: 0,
            slide: 1,
            to_group: 1,
            to: 0,
        },
        Op::MoveGroup { from: 2, to: 0 },
        Op::SetFrame {
            group: 1,
            slide: 0,
            element: 0,
            frame: doc::Rect {
                x: 40.0,
                y: 300.0,
                width: 900.0,
                height: 220.0,
            },
        },
        Op::SetRotation {
            group: 1,
            slide: 0,
            element: 0,
            degrees: 5.0,
        },
        Op::SetSlideNotes {
            group: 0,
            slide: 0,
            notes: "Slow down here".into(),
        },
    ];
    let n = edits.len();
    for e in edits {
        core.send(Command::Edit {
            presentation: file.clone(),
            edit: e,
            accept_native: false,
        });
    }
    let s = wait("edited", &|s| {
        s.editing
            .get(&file)
            .is_some_and(|e| e.doc.groups[0].slides[0].notes == "Slow down here")
    });
    let edited = summary(&s.editing[&file].doc);
    let frame = |d: &doc::Document| {
        let t = d.groups[1].slides[0].text_boxes().next().unwrap();
        (t.frame, t.rotation)
    };
    let edited_frame = frame(&s.editing[&file].doc);
    core.send(Command::Save(file.clone()));
    let s = wait("saved", &|s| !s.unsaved.contains(&file));
    let back = doc::load(&file).unwrap();
    assert_eq!(summary(&back), edited, "groups, slides, text, arrangements");
    assert_eq!(frame(&back), edited_frame, "the moved box");
    assert_eq!(back.groups[0].slides[0].notes, "Slow down here");
    // Undo still reaches back through every step after saving.
    for _ in 0..n {
        core.send(Command::Undo(file.clone()));
    }
    let s2 = wait("undone", &|s| {
        s.editing.get(&file).is_some_and(|e| e.undo.is_none())
    });
    assert_eq!(
        summary(&s2.editing[&file].doc),
        summary(&doc::load(&dir.join("song.pro6.bak")).unwrap())
    );
    drop(s);
    std::fs::remove_dir_all(&dir).unwrap();
}
