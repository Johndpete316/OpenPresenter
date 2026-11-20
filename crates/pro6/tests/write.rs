#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Writer: unchanged documents are byte-exact; edits change only what was edited.

use std::path::PathBuf;

use pro6::{Document, Rgba};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn load(name: &str) -> (String, Document) {
    let path = fixture(name);
    let src = std::fs::read_to_string(&path).unwrap();
    (src, pro6::load(&path).unwrap())
}

#[test]
fn unchanged_fixtures_are_byte_exact() {
    for name in [
        "song_1024x768.pro6",
        "two_fonts_1920x1080.pro6",
        "unknowns.pro6",
    ] {
        let (src, doc) = load(name);
        let out = pro6::write_string(&doc);
        assert!(
            out == src,
            "{name} not byte-exact:\n{}",
            first_difference(&src, &out)
        );
    }
}

#[test]
fn typed_edits_are_written_and_reparse() {
    let (src, mut doc) = load("song_1024x768.pro6");
    doc.canvas.width = 1920;
    doc.canvas.height = 1080;
    doc.groups.as_mut().unwrap().items[0].name = "Verse 1 & 2 <new>".into();
    doc.groups.as_mut().unwrap().items[1].color = Some(Rgba {
        r: 0.0,
        g: 0.5,
        b: 1.0,
        a: 1.0,
    });
    let slide = &mut doc.groups.as_mut().unwrap().items[1]
        .slides
        .as_mut()
        .unwrap()
        .items[1];
    slide.enabled = true;
    let text = match &mut slide.elements.as_mut().unwrap().items[0] {
        pro6::DisplayElement::Text(t) => t,
        _ => unreachable!(),
    };
    text.position.value.width = 1856.0;
    text.shadow.as_mut().unwrap().value.blur = Some(2.5);

    let out = pro6::write_string(&doc);
    assert_ne!(out, src);
    let back = pro6::parse_str(&out, &fixture("edited.pro6")).unwrap();
    assert_eq!((back.canvas.width, back.canvas.height), (1920, 1080));
    assert_eq!(back.groups()[0].name, "Verse 1 & 2 <new>");
    assert_eq!(
        back.groups()[1].color,
        Some(Rgba {
            r: 0.0,
            g: 0.5,
            b: 1.0,
            a: 1.0
        })
    );
    assert!(back.slides().all(|s| s.enabled));
    let t = back.groups()[1].slides()[1].text_elements().next().unwrap();
    assert_eq!(t.position.value.width, 1856.0);
    assert_eq!(t.shadow.as_ref().unwrap().value.blur, Some(2.5));
    // Untouched siblings keep their original text exactly.
    assert!(
        out.contains(
            r#"<shadow rvXMLIvarName="shadow">0|0 0 0 0.3333333432674408|{4, -4}</shadow>"#
        )
    );
    assert!(out.contains(
        r#"<shadow rvXMLIvarName="shadow">2.5|0 0 0 0.3333333432674408|{4, -4}</shadow>"#
    ));
    assert!(out.contains(r#"name="Verse 1 &amp; 2 &lt;new&gt;""#));

    // UUIDs are preserved exactly.
    let uuids = |d: &Document| {
        let mut v: Vec<String> = d.groups().iter().map(|g| g.uuid.clone()).collect();
        v.extend(d.slides().map(|s| s.uuid.clone()));
        v.extend(d.text_elements().map(|t| t.uuid.clone()));
        v
    };
    assert_eq!(uuids(&back), uuids(&doc));
}

#[test]
fn removed_and_added_children() {
    let (_, mut doc) = load("song_1024x768.pro6");
    let groups = doc.groups.as_mut().unwrap();
    // Remove a slide, duplicate another (as an editor would).
    let slides = groups.items[0].slides.as_mut().unwrap();
    let copy = slides.items[0].clone();
    slides.items.remove(1);
    slides.items.push(copy);
    // Drop a group.
    groups.items.remove(2);
    let out = pro6::write_string(&doc);
    let back = pro6::parse_str(&out, &fixture("edited.pro6")).unwrap();
    let names: Vec<_> = back
        .groups()
        .iter()
        .map(|g| (g.name.as_str(), g.slides().len()))
        .collect();
    assert_eq!(names, [("Verse 1", 2), ("Chorus", 2)]);
    assert_eq!(
        back.groups()[0].slides()[0].uuid,
        back.groups()[0].slides()[1].uuid
    );
}

#[test]
fn unknowns_survive_edits_elsewhere() {
    let (_, mut doc) = load("unknowns.pro6");
    doc.notes = "edited".into();
    let out = pro6::write_string(&doc);
    for needle in [
        r#"futureRootAttr="kept""#,
        "<SyntheticRootChild>kept as raw XML</SyntheticRootChild>",
        "<SyntheticGroupLike name=\"not a group\" />",
        "<SyntheticImageElement",
        "<SyntheticChild rvXMLIvarName=\"future\">opaque</SyntheticChild>",
        "<SyntheticArrangement",
        "<SyntheticMediaCue",
        "<!-- SYNTHETIC FIXTURE.",
    ] {
        assert!(out.contains(needle), "lost {needle}");
    }
    assert!(out.contains(r#"notes="edited""#));
}

#[test]
fn save_is_atomic_and_round_trips() {
    let dir = std::env::temp_dir().join(format!("pro6-save-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("out.pro6");
    let (src, doc) = load("two_fonts_1920x1080.pro6");
    pro6::save(&doc, &target).unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), src);
    assert!(!dir.join("out.pro6.tmp").exists());

    let err = pro6::save(&doc, &dir.join("missing-dir/out.pro6")).unwrap_err();
    assert!(
        err.user_message().contains("original file is unchanged"),
        "{}",
        err.user_message()
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

fn first_difference(a: &str, b: &str) -> String {
    let i = a
        .bytes()
        .zip(b.bytes())
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let lo = i.saturating_sub(80);
    format!(
        "at byte {i}\nexpected: {:?}\n     got: {:?}",
        a.get(lo..(i + 80).min(a.len())).unwrap_or(""),
        b.get(lo..(i + 80).min(b.len())).unwrap_or("")
    )
}
