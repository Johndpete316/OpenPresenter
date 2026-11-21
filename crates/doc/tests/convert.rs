#![allow(clippy::unwrap_used, clippy::expect_used)]
//! E0 acceptance: fixtures convert both ways with zero semantic diff; save rules.

use std::path::{Path, PathBuf};

use doc::{Document, Edit, ExportChoice, Mode};
use pretty_assertions::assert_eq;

fn fixtures() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| doc::is_pro6(p))
        .collect();
    v.sort();
    assert!(v.len() >= 5, "fixtures found: {v:?}");
    v
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// The native content without bookkeeping (source XML, origin keys).
fn semantic(d: &Document) -> Document {
    let mut d = d.clone();
    d.pro6 = None;
    d.path = PathBuf::new();
    for g in &mut d.groups {
        g.src = None;
        for s in &mut g.slides {
            s.src = None;
            for e in &mut s.elements {
                match e {
                    doc::Element::Text(t) => t.src = None,
                    doc::Element::Image(i) => i.src = None,
                    doc::Element::Pro6(o) => o.src = None,
                }
            }
        }
    }
    d
}

/// Export, write, parse again and convert: what a reopened `.pro6` would hold.
fn through_pro6(d: &Document) -> Document {
    let out = doc::to_pro6(d).unwrap();
    let xml = pro6::write_string(&out);
    let back = pro6::parse_str(&xml, &d.path).unwrap();
    assert_eq!(pro6::write_string(&back), xml, "pro6 write is stable");
    doc::from_pro6(back)
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("doc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn unchanged_documents_export_byte_identical() {
    for f in fixtures() {
        let src = std::fs::read_to_string(&f).unwrap();
        let original = pro6::load(&f).unwrap();
        let native = doc::from_pro6(original.clone());
        let out = doc::to_pro6(&native).unwrap();
        assert_eq!(
            pro6::write_string(&out),
            pro6::write_string(&original),
            "{}",
            f.display()
        );
        // The pro6 writer itself is byte-identical on fixtures (M1), so this is the file.
        assert_eq!(pro6::write_string(&out), src, "{}", f.display());
        assert_eq!(semantic(&through_pro6(&native)), semantic(&native));
    }
}

#[test]
fn native_json_round_trips() {
    for f in fixtures() {
        let native = doc::load(&f).unwrap();
        let json = doc::file::to_json(&native);
        let back = doc::file::from_json(&json, Path::new("x.opres")).unwrap();
        assert_eq!(semantic(&back), semantic(&native), "{}", f.display());
        assert_eq!(
            pro6::write_string(back.pro6.as_deref().unwrap()),
            pro6::write_string(native.pro6.as_deref().unwrap()),
            "embedded source survives"
        );
        // Exports the same from the JSON copy.
        assert_eq!(
            pro6::write_string(&doc::to_pro6(&back).unwrap()),
            pro6::write_string(&doc::to_pro6(&native).unwrap())
        );
    }
}

#[test]
fn every_edit_survives_pro6_export() {
    let mut d = doc::load(&fixture("song_1024x768.pro6")).unwrap();
    let arr = doc::new_uuid();
    let edits = vec![
        Edit::SetText {
            group: 0,
            slide: 0,
            text: "First line\n\nThird line".into(),
        },
        Edit::AddSlide {
            group: 1,
            at: 0,
            text: "Brand new".into(),
        },
        Edit::DuplicateSlide { group: 0, slide: 1 },
        Edit::MoveSlide {
            group: 0,
            slide: 0,
            to_group: 2,
            to: 0,
        },
        Edit::RemoveSlide { group: 1, slide: 1 },
        Edit::AddGroup {
            at: 1,
            name: "Tag".into(),
            color: Some(doc::Rgba {
                r: 1.0,
                g: 0.5,
                b: 0.0,
                a: 1.0,
            }),
        },
        Edit::RenameGroup {
            group: 0,
            name: "Verse One".into(),
        },
        Edit::RecolorGroup {
            group: 0,
            color: None,
        },
        Edit::MoveGroup { from: 3, to: 0 },
        Edit::AddArrangement {
            uuid: arr.clone(),
            name: "Short".into(),
            groups: vec![1, 0, 1],
        },
        Edit::SetArrangementGroups {
            uuid: arr.clone(),
            groups: vec![0, 1, 2, 1],
        },
        Edit::RenameArrangement {
            uuid: arr.clone(),
            name: "Shorter".into(),
        },
        Edit::SetFrame {
            group: 1,
            slide: 0,
            element: 0,
            frame: doc::Rect {
                x: 10.0,
                y: 20.5,
                width: 600.0,
                height: 300.0,
            },
        },
        Edit::SetRotation {
            group: 1,
            slide: 0,
            element: 0,
            degrees: 12.5,
        },
        Edit::SetBoxText {
            group: 1,
            slide: 0,
            element: 0,
            text: "Box text".into(),
        },
        Edit::SetRichText {
            group: 1,
            slide: 0,
            element: 0,
            paragraphs: {
                let base = doc::Run {
                    text: "Plain ".into(),
                    font: "Liberation Sans".into(),
                    size: 48.0,
                    color: Some([255, 255, 255]),
                    bold: false,
                    italic: false,
                    underline: false,
                    letter_spacing: 0.0,
                };
                vec![
                    doc::Paragraph {
                        align: doc::Align::Center,
                        line_spacing: 1.0,
                        runs: vec![
                            base.clone(),
                            doc::Run {
                                text: "bold red".into(),
                                bold: true,
                                color: Some([220, 30, 30]),
                                ..base.clone()
                            },
                        ],
                    },
                    doc::Paragraph {
                        align: doc::Align::Right,
                        line_spacing: 1.0,
                        runs: vec![doc::Run {
                            text: "Serif, italic, underlined\nwith a line break".into(),
                            font: "Liberation Serif".into(),
                            size: 30.0,
                            italic: true,
                            underline: true,
                            ..base
                        }],
                    },
                ]
            },
        },
        Edit::SetVAlign {
            group: 1,
            slide: 0,
            element: 0,
            valign: doc::VAlign::Bottom,
        },
        Edit::SetSlideEnabled {
            group: 1,
            slide: 0,
            enabled: false,
        },
        Edit::SetSlideNotes {
            group: 1,
            slide: 0,
            notes: "Watch the key change".into(),
        },
        Edit::SetSlideBackground {
            group: 1,
            slide: 0,
            color: Some(doc::Rgba {
                r: 0.0,
                g: 0.2,
                b: 0.4,
                a: 1.0,
            }),
        },
        Edit::SetSlideBackground {
            group: 3,
            slide: 0,
            color: None,
        },
    ];
    for e in &edits {
        e.apply(&mut d).unwrap();
        // After every step the document survives a .pro6 round trip unchanged.
        assert_eq!(semantic(&through_pro6(&d)), semantic(&d), "after {e:?}");
    }
    // An empty group (Tag, now at 2) round-trips too, and removing a group drops it
    // from arrangements.
    Edit::RemoveGroup { group: 0 }.apply(&mut d).unwrap();
    assert_eq!(semantic(&through_pro6(&d)), semantic(&d));
    Edit::RemoveArrangement { uuid: arr }.apply(&mut d).unwrap();
    assert_eq!(semantic(&through_pro6(&d)), semantic(&d));
    let back = through_pro6(&d);
    assert_eq!(back.groups[1].name, "Tag");
}

#[test]
fn copies_get_fresh_ids_in_the_pro6() {
    let mut d = doc::load(&fixture("song_1024x768.pro6")).unwrap();
    Edit::DuplicateSlide { group: 0, slide: 0 }
        .apply(&mut d)
        .unwrap();
    let out = doc::to_pro6(&d).unwrap();
    let xml = pro6::write_string(&out);
    let back = pro6::parse_str(&xml, &d.path).unwrap();
    let mut ids: Vec<String> = back.slides().map(|s| s.uuid.clone()).collect();
    ids.extend(back.text_elements().map(|t| t.uuid.clone()));
    let n = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n, "no duplicate UUIDs after a copy");
}

#[test]
fn refused_edits_change_nothing() {
    let d0 = doc::load(&fixture("song_1024x768.pro6")).unwrap();
    let mut d = d0.clone();
    for e in [
        Edit::RemoveSlide {
            group: 0,
            slide: 99,
        },
        Edit::MoveGroup { from: 0, to: 9 },
        Edit::SetArrangementGroups {
            uuid: "nope".into(),
            groups: vec![0],
        },
        Edit::Batch {
            label: "half".into(),
            edits: vec![
                Edit::RenameGroup {
                    group: 0,
                    name: "changed".into(),
                },
                Edit::RemoveSlide {
                    group: 0,
                    slide: 99,
                },
            ],
        },
    ] {
        assert!(e.apply(&mut d).is_err());
        assert_eq!(d, d0, "{e:?}");
    }
}

#[test]
fn pro6_mode_saves_the_pro6_with_a_backup() {
    let dir = temp_dir("save-pro6");
    let file = dir.join("song.pro6");
    std::fs::copy(fixture("song_1024x768.pro6"), &file).unwrap();
    let mut d = doc::open(&file).unwrap();
    Edit::RenameGroup {
        group: 0,
        name: "Renamed".into(),
    }
    .apply(&mut d)
    .unwrap();
    let saved = doc::save(&d).unwrap();
    assert_eq!(saved.path, file);
    assert_eq!(semantic(&saved), semantic(&d));
    assert_eq!(doc::load(&file).unwrap().groups[0].name, "Renamed");
    assert_eq!(
        std::fs::read(dir.join("song.pro6.bak")).unwrap(),
        std::fs::read(fixture("song_1024x768.pro6")).unwrap()
    );
    assert!(!dir.join("song.opres").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn native_mode_saves_opres_next_to_the_untouched_original() {
    let dir = temp_dir("save-native");
    let file = dir.join("song.pro6");
    std::fs::copy(fixture("song_1024x768.pro6"), &file).unwrap();
    let original = std::fs::read(&file).unwrap();

    let mut d = doc::open(&file).unwrap();
    assert_eq!(d.mode, Mode::Pro6);
    // A native-only feature is accepted on slide 1 ("Continue anyway").
    let slide = d.groups[0].slides[0].uuid.clone();
    match doc::edit::check_features(&d, &[doc::Feature::Vignette], vec![slide.clone()]) {
        doc::Check::NeedsNative { features, slides } => {
            assert_eq!(features, vec![doc::Feature::Vignette]);
            assert_eq!(slides, vec![slide.clone()]);
        }
        doc::Check::Ok => panic!("vignette needs native mode"),
    }
    doc::mark_native(&mut d, &[slide]);
    assert_eq!(d.mode, Mode::Native);
    assert!(d.groups[0].slides[0].pro6_incompatible);
    // In native mode nothing needs asking any more.
    assert_eq!(
        doc::edit::check_features(&d, &[doc::Feature::Vignette], vec![]),
        doc::Check::Ok
    );

    let saved = doc::save(&d).unwrap();
    let opres = dir.join("song.opres");
    assert_eq!(saved.path, opres);
    assert_eq!(
        std::fs::read(&file).unwrap(),
        original,
        "original untouched"
    );
    assert!(!dir.join("song.pro6.bak").exists());

    // Opening the .pro6 now opens the .opres.
    assert_eq!(doc::preferred(&file), opres);
    let back = doc::open(&file).unwrap();
    assert_eq!(back.path, opres);
    assert_eq!(back.mode, Mode::Native);
    assert_eq!(semantic(&back), semantic(&saved));

    // Saving again writes the .opres (with a backup of the previous one).
    let again = doc::save(&back).unwrap();
    assert_eq!(again.path, opres);
    assert!(dir.join("song.opres.bak").exists());
    assert_eq!(std::fs::read(&file).unwrap(), original);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn exporting_native_documents_skips_or_simplifies_incompatible_slides() {
    let mut d = doc::load(&fixture("song_1024x768.pro6")).unwrap();
    let uuid = d.groups[0].slides[1].uuid.clone();
    doc::mark_native(&mut d, std::slice::from_ref(&uuid));
    let n = d.slides().count();

    let (out, report) = doc::export_pro6(&d, ExportChoice::SkipIncompatible).unwrap();
    assert_eq!(
        report.left_out,
        vec![format!("{} / slide 2", d.groups[0].name)]
    );
    assert_eq!(out.slides().count(), n - 1);
    assert!(out.slides().all(|s| s.uuid != uuid));

    let (out, report) = doc::export_pro6(&d, ExportChoice::DropUnsupported).unwrap();
    assert_eq!(report.left_out.len(), 1);
    assert_eq!(out.slides().count(), n);
}

#[test]
fn opres_versions() {
    let d = doc::load(&fixture("render_1024x768.pro6")).unwrap();
    let mut v: serde_json::Value = serde_json::from_slice(&doc::file::to_json(&d)).unwrap();
    assert_eq!(v["format_version"], doc::FORMAT_VERSION);
    v["format_version"] = (doc::FORMAT_VERSION + 1).into();
    let err =
        doc::file::from_json(&serde_json::to_vec(&v).unwrap(), Path::new("a.opres")).unwrap_err();
    assert!(matches!(err, doc::LoadError::Newer { .. }), "{err}");
    assert!(err.user_message().contains("newer version"));
    let err = doc::file::from_json(b"{}", Path::new("a.opres")).unwrap_err();
    assert!(matches!(err, doc::LoadError::Json { .. }), "{err}");
}

#[test]
fn placeholder_boxes_are_empty_text_in_their_style() {
    // song fixture's blank-slide behaviour is covered in pro6; here: a placeholder
    // converts to empty text, keeps a style for typing into, and exports unchanged.
    let src = std::fs::read_to_string(fixture("song_1024x768.pro6")).unwrap();
    let p = pro6::parse_str(&src, Path::new("s.pro6")).unwrap();
    let mut el = p.text_elements().next().unwrap().clone();
    el.set_text(
        pro6::model::PLACEHOLDER_TEXT,
        pro6::WinPayloads::Keep,
        Path::new("s.pro6"),
    )
    .unwrap();
    let mut p2 = p.clone();
    if let Some(pro6::DisplayElement::Text(t)) = p2.groups.as_mut().unwrap().items[0]
        .slides
        .as_mut()
        .unwrap()
        .items[0]
        .elements
        .as_mut()
        .unwrap()
        .items
        .first_mut()
    {
        **t = el;
    }
    let d = doc::from_pro6(p2.clone());
    let t = d.groups[0].slides[0].text_boxes().next().unwrap();
    assert_eq!(t.plain_text(), "");
    assert!(t.runs().next().is_some_and(|r| !r.font.is_empty()));
    assert_eq!(
        pro6::write_string(&doc::to_pro6(&d).unwrap()),
        pro6::write_string(&p2)
    );
}

/// Group names, slide texts and enabled flags, plus arrangements by group name.
type Shape = (
    Vec<(String, Vec<(String, bool)>)>,
    Vec<(String, Vec<String>)>,
);

fn shape(d: &Document) -> Shape {
    let groups = d
        .groups
        .iter()
        .map(|g| {
            (
                g.name.clone(),
                g.slides
                    .iter()
                    .map(|s| (s.plain_text(), s.enabled))
                    .collect(),
            )
        })
        .collect();
    let arrangements = d
        .arrangements()
        .into_iter()
        .map(|a| {
            (
                a.name,
                a.groups.iter().map(|&g| d.groups[g].name.clone()).collect(),
            )
        })
        .collect();
    (groups, arrangements)
}

#[test]
fn native_edits_match_the_m7_pro6_operations() {
    use pro6::ops::Op;
    let file = fixture("song_1024x768.pro6");
    let mut p = pro6::load(&file).unwrap();
    let mut d = doc::load(&file).unwrap();
    let arr = doc::new_uuid();
    let pairs = vec![
        (
            Op::SetText {
                group: 0,
                slide: 1,
                text: "A\nB".into(),
            },
            Edit::SetText {
                group: 0,
                slide: 1,
                text: "A\nB".into(),
            },
        ),
        (
            Op::AddSlide {
                group: 2,
                at: 1,
                text: "Added".into(),
            },
            Edit::AddSlide {
                group: 2,
                at: 1,
                text: "Added".into(),
            },
        ),
        (
            Op::DuplicateSlide { group: 1, slide: 0 },
            Edit::DuplicateSlide { group: 1, slide: 0 },
        ),
        (
            Op::MoveSlide {
                group: 1,
                slide: 0,
                to_group: 0,
                to: 2,
            },
            Edit::MoveSlide {
                group: 1,
                slide: 0,
                to_group: 0,
                to: 2,
            },
        ),
        (
            Op::MoveGroup { from: 2, to: 0 },
            Edit::MoveGroup { from: 2, to: 0 },
        ),
        (
            Op::AddArrangement {
                uuid: arr.clone(),
                name: "Both".into(),
                groups: vec![0, 1, 0],
            },
            Edit::AddArrangement {
                uuid: arr.clone(),
                name: "Both".into(),
                groups: vec![0, 1, 0],
            },
        ),
        (Op::RemoveGroup { group: 1 }, Edit::RemoveGroup { group: 1 }),
    ];
    for (op, edit) in pairs {
        op.apply(&mut p).unwrap();
        edit.apply(&mut d).unwrap();
        let m7 = doc::from_pro6(pro6::parse_str(&pro6::write_string(&p), &file).unwrap());
        assert_eq!(shape(&through_pro6(&d)), shape(&m7), "after {edit:?}");
    }
}

#[test]
fn effects_and_spacing_need_native_mode() {
    let d = doc::load(&fixture("song_1024x768.pro6")).unwrap();
    let outline = Edit::SetOutline {
        group: 0,
        slide: 0,
        element: 0,
        outline: Some(doc::Outline {
            width: 3.0,
            color: doc::Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        }),
    };
    assert!(matches!(
        doc::check(&d, &outline),
        doc::Check::NeedsNative { .. }
    ));
    let off = Edit::SetOutline {
        group: 0,
        slide: 0,
        element: 0,
        outline: None,
    };
    assert_eq!(
        doc::check(&d, &off),
        doc::Check::Ok,
        "turning it off is fine in .pro6"
    );
    let mut paragraphs = d.groups[0].slides[0]
        .text_boxes()
        .next()
        .unwrap()
        .paragraphs
        .clone();
    paragraphs[0].line_spacing = 1.5;
    let spaced = Edit::SetRichText {
        group: 0,
        slide: 0,
        element: 0,
        paragraphs,
    };
    match doc::check(&d, &spaced) {
        doc::Check::NeedsNative { features, slides } => {
            assert_eq!(features, vec![doc::Feature::LineSpacing]);
            assert_eq!(slides, vec![d.groups[0].slides[0].uuid.clone()]);
        }
        doc::Check::Ok => panic!("line spacing is unverified in .pro6"),
    }
    // Accepted: native; exporting leaves the native-only parts out but works.
    let mut n = d.clone();
    outline.apply(&mut n).unwrap();
    spaced.apply(&mut n).unwrap();
    doc::mark_native(&mut n, &[d.groups[0].slides[0].uuid.clone()]);
    let (out, report) = doc::export_pro6(&n, ExportChoice::DropUnsupported).unwrap();
    assert_eq!(report.left_out.len(), 1);
    let back = doc::from_pro6(out);
    let t = back.groups[0].slides[0].text_boxes().next().unwrap();
    assert!(t.outline.is_none());
    assert_eq!(t.paragraphs[0].line_spacing, 1.0);
}
