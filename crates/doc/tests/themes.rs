#![allow(clippy::unwrap_used, clippy::expect_used)]
//! E7: theme files, theme editing documents, applying to slides with another canvas.

use std::path::PathBuf;

use doc::{Edit, Theme};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("doc-theme-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn theme_files_round_trip_with_media_relative_to_the_theme() {
    let dir = temp("file");
    let mut d = doc::load(&fixture("song_1024x768.pro6")).unwrap();
    d.path = dir.join("songs").join("song.opres");
    d.groups[0].slides[0].background_image = Some(doc::BackgroundImage {
        media: doc::MediaRef {
            path: "Media/bg.png".into(),
            sha256: None,
        },
        fit: doc::Fit::Fill,
        adjust: doc::Adjust::default(),
    });
    let t = Theme::from_slide("Sunday", &d, (0, 0)).unwrap();
    let file = dir.join("Themes").join(doc::theme::file_name("Sunday"));
    let saved = t.save(&file).unwrap();
    assert_eq!(
        saved.background_image.as_ref().unwrap().media.path,
        "../songs/Media/bg.png"
    );
    let back = Theme::load(&file).unwrap();
    assert_eq!(back, saved);
    // Pointed at another presentation's folder.
    let other = back.for_document(&dir.join("songs").join("other.pro6"));
    assert_eq!(other.background_image.unwrap().media.path, "Media/bg.png");
    assert!(doc::theme::is_theme(&file));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_theme_is_edited_as_a_one_slide_document_and_saved_back() {
    let dir = temp("edit");
    let d = doc::load(&fixture("song_1024x768.pro6")).unwrap();
    let file = dir.join("Blue.optheme");
    let t = Theme::from_slide("Blue", &d, (0, 0))
        .unwrap()
        .save(&file)
        .unwrap();
    let mut td = doc::load(&file).unwrap();
    assert_eq!(td.theme.as_ref().unwrap().uuid, t.uuid);
    assert_eq!(td.slides().count(), 1);
    assert_eq!(doc::save_path(&td), file, "a theme saves to its own file");
    // Edit it like any slide: move the box and colour the text.
    Edit::SetFrame {
        group: 0,
        slide: 0,
        element: 0,
        frame: doc::Rect {
            x: 10.0,
            y: 20.0,
            width: 300.0,
            height: 100.0,
        },
    }
    .apply(&mut td)
    .unwrap();
    doc::save(&td).unwrap();
    let back = Theme::load(&file).unwrap();
    assert_eq!(back.uuid, t.uuid, "same theme");
    assert_eq!(back.text_frame.x, 10.0);
    assert_eq!(back.text, t.text);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn some_slides_with_another_canvas_scale_the_theme_in() {
    let wide = doc::load(&fixture("two_fonts_1920x1080.pro6")).unwrap();
    let theme = Theme::from_slide("Wide", &wide, (0, 0)).unwrap();
    let mut d = doc::load(&fixture("song_1024x768.pro6")).unwrap();
    let canvas = d.canvas;
    let e = Edit::ApplyTheme {
        slides: vec![(0, 0)],
        theme: Box::new(theme.clone()),
        all: false,
        link: false,
    };
    e.apply(&mut d).unwrap();
    assert_eq!(d.canvas, canvas, "some slides: the canvas stays");
    let (s, (ox, oy)) = doc::theme::fit(theme.canvas, canvas);
    let t = d.groups[0].slides[0].text_boxes().next().unwrap();
    assert_eq!(t.frame.x, (ox + theme.text_frame.x * s).round());
    assert_eq!(t.frame.y, (oy + theme.text_frame.y * s).round());
    let size = t.runs().next().unwrap().size;
    assert_eq!(
        size,
        ((theme.text.run.size * s * 2.0).round() / 2.0).max(1.0)
    );
    assert!(
        d.groups[0].slides[0].theme.is_none(),
        "no link in a .pro6 document"
    );
    // All slides: the presentation takes the theme's canvas.
    let mut all = doc::load(&fixture("song_1024x768.pro6")).unwrap();
    Edit::ApplyTheme {
        slides: vec![],
        theme: Box::new(theme.clone()),
        all: true,
        link: true,
    }
    .apply(&mut all)
    .unwrap();
    assert_eq!(all.canvas, theme.canvas);
    assert!(
        all.slides()
            .all(|s| s.theme.as_ref().is_some_and(|l| l.uuid == theme.uuid))
    );
    // Plain themes are .pro6-safe; the export writes the new canvas.
    let back = doc::from_pro6(doc::to_pro6(&all).unwrap());
    assert_eq!(back.canvas, theme.canvas);
}
