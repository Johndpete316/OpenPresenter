#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Render the fixtures with only the bundled Liberation fonts loaded (deterministic).

use std::path::PathBuf;

use render::{Fonts, RenderError, SlideFrame, render_slide};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn fonts() -> Fonts {
    Fonts::from_dirs(&[&fixtures().join("fonts")])
}

fn load(name: &str) -> doc::Document {
    doc::load(&fixtures().join(name)).unwrap()
}

fn render(
    fonts: &mut Fonts,
    d: &doc::Document,
    i: usize,
    w: u32,
    h: u32,
) -> Result<SlideFrame, RenderError> {
    let slide = d.slides().nth(i).unwrap();
    render_slide(fonts, d, slide, w, h)
}

/// Bounding box (x0, y0, x1, y1) of text pixels with alpha above a threshold.
fn ink_bbox(f: &SlideFrame) -> (u32, u32, u32, u32) {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..f.height {
        for x in 0..f.width {
            if f.text[((y * f.width + x) * 4 + 3) as usize] > 64 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    assert!(x0 != u32::MAX, "no text drawn");
    (x0, y0, x1, y1)
}

fn near(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn missing_fonts_are_errors_not_substitutions() {
    let mut f = fonts();
    let d = load("render_1024x768.pro6");
    match render(&mut f, &d, 3, 1024, 768) {
        Err(e @ RenderError::MissingFont { .. }) => assert_eq!(
            e.user_message(),
            "Slide uses font 'Futura PT', which isn't installed on this PC. The slide can't be shown until the font is installed."
        ),
        other => panic!("expected MissingFont, got {other:?}"),
    }
    // Arial isn't bundled. Liberation Sans is metric-compatible but must NOT be used silently.
    let song = load("song_1024x768.pro6");
    assert!(matches!(
        render(&mut f, &song, 0, 1024, 768),
        Err(RenderError::MissingFont { ref font, .. }) if font == "Arial"
    ));
}

#[test]
fn centred_middle_text_is_centred_in_its_box() {
    let mut f = fonts();
    let d = load("render_1024x768.pro6");
    let frame = render(&mut f, &d, 0, 1024, 768).unwrap();
    assert!(frame.warnings.is_empty(), "{:?}", frame.warnings);
    let (x0, y0, x1, y1) = ink_bbox(&frame);
    // Element box is {112 84 0 800 600}.
    assert!(
        x0 >= 112 && x1 < 912 && y0 >= 84 && y1 < 684,
        "{:?}",
        (x0, y0, x1, y1)
    );
    let (left, right) = (x0 as f32 - 112.0, 911.0 - x1 as f32);
    assert!(
        near(left, right, 4.0),
        "horizontal centring {left} vs {right}"
    );
    let (top, bottom) = (y0 as f32 - 84.0, 683.0 - y1 as f32);
    // Ink isn't the line box (cap height, descenders), so allow roughly half a line.
    assert!(near(top, bottom, 24.0), "vertical middle {top} vs {bottom}");
}

#[test]
fn per_paragraph_alignment_fonts_and_colours() {
    let mut f = fonts();
    let d = load("render_1024x768.pro6");
    let frame = render(&mut f, &d, 1, 1024, 768).unwrap();
    let (x0, y0, x1, _) = ink_bbox(&frame);
    assert!(
        x0 <= 116,
        "left-aligned paragraph starts at the box edge: {x0}"
    );
    assert!(
        x1 >= 905,
        "right-aligned paragraph ends at the box edge: {x1}"
    );
    assert!(y0 < 84 + 40, "top-aligned: {y0}");
    // Yellow (255,221,0) text exists for the second paragraph.
    let yellow = frame.text.as_chunks::<4>().0.contains(&[255, 221, 0, 255]);
    assert!(yellow);
}

#[test]
fn bottom_aligned_bold_with_drop_shadow() {
    let mut f = fonts();
    let mut d = load("render_1024x768.pro6");
    let frame = render(&mut f, &d, 2, 1024, 768).unwrap();
    assert!(frame.warnings.is_empty(), "{:?}", frame.warnings);
    // The fixture's shadow: black at 1/3 opacity, 4 px right and down, no blur.
    let shadow = frame
        .text
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[3] > 70 && p[3] < 100 && p[0] < 10)
        .count();
    assert!(shadow > 50, "shadow pixels: {shadow}");
    // Without it, the ink's bottom moves up by the offset.
    let (_, _, _, y1) = ink_bbox(&frame);
    for e in &mut d.groups[0].slides[2].elements {
        if let doc::Element::Text(t) = e {
            t.shadow = None;
        }
    }
    let plain = render(&mut f, &d, 2, 1024, 768).unwrap();
    let (_, _, _, y1_plain) = ink_bbox(&plain);
    assert!(
        y1_plain > 684 - 30 && y1_plain < 684,
        "bottom-aligned: {y1_plain}"
    );
    assert!(y1.abs_diff(y1_plain + 4) <= 1, "{y1} vs {y1_plain}");
}

fn first_box(d: &mut doc::Document, slide: usize) -> &mut doc::TextBox {
    d.groups[0].slides[slide]
        .elements
        .iter_mut()
        .find_map(|e| match e {
            doc::Element::Text(t) => Some(&mut **t),
            _ => None,
        })
        .unwrap()
}

#[test]
fn outline_underline_and_letter_spacing_are_drawn() {
    let mut f = fonts();
    let mut d = load("render_1024x768.pro6");
    let plain = render(&mut f, &d, 0, 1024, 768).unwrap();
    let (x0, y0, x1, y1) = ink_bbox(&plain);

    // A 6 px red outline grows the ink by about 6 px on every side.
    first_box(&mut d, 0).outline = Some(doc::Outline {
        width: 6.0,
        color: doc::Rgba {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        },
    });
    let outlined = render(&mut f, &d, 0, 1024, 768).unwrap();
    assert!(outlined.warnings.is_empty(), "{:?}", outlined.warnings);
    let (a0, b0, a1, b1) = ink_bbox(&outlined);
    for (grown, by) in [(x0 - a0, 6), (y0 - b0, 6), (a1 - x1, 6), (b1 - y1, 6)] {
        assert!(grown.abs_diff(by) <= 2, "grew {grown}, wanted about {by}");
    }
    assert!(
        outlined.text.as_chunks::<4>().0.contains(&[255, 0, 0, 255]),
        "red outline"
    );
    first_box(&mut d, 0).outline = None;

    // Underline: ink reaches lower, but stays inside the line.
    for r in first_box(&mut d, 0)
        .paragraphs
        .iter_mut()
        .flat_map(|p| &mut p.runs)
    {
        r.underline = true;
    }
    let under = render(&mut f, &d, 0, 1024, 768).unwrap();
    assert!(under.warnings.is_empty(), "{:?}", under.warnings);
    let (_, _, _, u1) = ink_bbox(&under);
    assert!(
        u1 >= y1,
        "underline at or below the descenders: {u1} vs {y1}"
    );
    for r in first_box(&mut d, 0)
        .paragraphs
        .iter_mut()
        .flat_map(|p| &mut p.runs)
    {
        r.underline = false;
        r.letter_spacing = 10.0;
    }

    // Letter spacing widens the (centred) lines.
    let spaced = render(&mut f, &d, 0, 1024, 768).unwrap();
    let (s0, _, s1, _) = ink_bbox(&spaced);
    assert!(s1 - s0 > x1 - x0 + 50, "{} vs {}", s1 - s0, x1 - x0);

    // Line spacing 2 roughly doubles the gap between the two lines.
    for r in first_box(&mut d, 0)
        .paragraphs
        .iter_mut()
        .flat_map(|p| &mut p.runs)
    {
        r.letter_spacing = 0.0;
    }
    for p in &mut first_box(&mut d, 0).paragraphs {
        p.line_spacing = 2.0;
    }
    let tall = render(&mut f, &d, 0, 1024, 768).unwrap();
    let (_, t0, _, t1) = ink_bbox(&tall);
    assert!(t1 - t0 > (y1 - y0) + 30, "{} vs {}", t1 - t0, y1 - y0);
}

#[test]
fn overflow_is_flagged() {
    let mut f = fonts();
    let mut d = load("render_1024x768.pro6");
    first_box(&mut d, 0).frame.height = 40.0;
    let frame = render(&mut f, &d, 0, 1024, 768).unwrap();
    assert!(
        frame.warnings.iter().any(|w| w.overflow),
        "{:?}",
        frame.warnings
    );
}

#[test]
fn scaled_output_pillarboxes_and_scales_text() {
    let mut f = fonts();
    let d = load("render_1024x768.pro6");
    let small = render(&mut f, &d, 0, 1024, 768).unwrap();
    let big = render(&mut f, &d, 0, 1920, 1080).unwrap();
    assert_eq!(big.placement.canvas.x, 240);
    let s = big.placement.scale;
    let (a, b) = (ink_bbox(&small), ink_bbox(&big));
    let tol = 6.0;
    assert!(
        near(b.0 as f32, 240.0 + a.0 as f32 * s, tol),
        "{a:?} → {b:?}"
    );
    assert!(
        near(b.2 as f32, 240.0 + a.2 as f32 * s, tol),
        "{a:?} → {b:?}"
    );
    assert!(near(b.1 as f32, a.1 as f32 * s, tol), "{a:?} → {b:?}");
    // Bars are pure black.
    let img = big.composite();
    for y in [0, 539, 1079] {
        for x in [0, 239, 1680, 1919] {
            assert_eq!(img.get_pixel(x, y).0, [0, 0, 0, 255]);
        }
    }
}

#[test]
fn layers_composite_independently() {
    let mut f = fonts();
    let mut d = load("render_1024x768.pro6");
    // Give slide 1 a visible background to tell the layers apart.
    d.groups[0].slides[0].background = Some(doc::Rgba {
        r: 0.0,
        g: 0.0,
        b: 1.0,
        a: 1.0,
    });
    let frame = render(&mut f, &d, 0, 1920, 1080).unwrap();
    let full = frame.composite_layers(true, true);
    let no_text = frame.composite_layers(true, false);
    let no_bg = frame.composite_layers(false, true);
    assert_eq!(no_text.get_pixel(960, 50).0, [0, 0, 255, 255]);
    assert_eq!(
        no_text.get_pixel(100, 540).0,
        [0, 0, 0, 255],
        "bars stay black"
    );
    assert_eq!(no_bg.get_pixel(960, 50).0, [0, 0, 0, 255]);
    let (x0, y0, x1, y1) = ink_bbox(&frame);
    let has_white = |img: &image::RgbaImage| {
        (y0..=y1).any(|y| (x0..=x1).any(|x| img.get_pixel(x, y).0 == [255, 255, 255, 255]))
    };
    assert!(has_white(&full) && has_white(&no_bg) && !has_white(&no_text));
}

#[test]
fn font_resolution() {
    let f = fonts();
    let sans = f.resolve("liberation sans", false, false).unwrap();
    assert_eq!(sans.family, "Liberation Sans");
    assert!(!sans.inexact);
    let bold = f.resolve("Liberation Sans", true, false).unwrap();
    assert_eq!(bold.weight, fontdb::Weight::BOLD);
    // Bundled serif has no bold face: resolves within the family, flagged inexact.
    assert!(f.resolve("Liberation Serif", true, false).unwrap().inexact);
    // PostScript name works too.
    assert_eq!(
        f.resolve("LiberationSans-Bold", false, false)
            .unwrap()
            .weight,
        fontdb::Weight::BOLD
    );
    assert!(f.resolve("Arial", false, false).is_none());
}

#[test]
fn rescaled_document_renders_like_the_original_fitted() {
    let mut f = fonts();
    let original = load("render_1024x768.pro6");
    let mut scaled = pro6::load(&fixtures().join("render_1024x768.pro6")).unwrap();
    pro6::rescale::rescale(
        &mut scaled,
        pro6::CanvasSize {
            width: 1920,
            height: 1080,
        },
        pro6::rescale::Mode::Fit,
        pro6::WinPayloads::Drop,
    );
    let scaled = doc::from_pro6(scaled);
    for i in 0..3 {
        let a = render(&mut f, &original, i, 1920, 1080).unwrap();
        let b = render(&mut f, &scaled, i, 1920, 1080).unwrap();
        let (ba, bb) = (ink_bbox(&a), ink_bbox(&b));
        let close = |x: u32, y: u32| x.abs_diff(y) <= 2;
        assert!(
            close(ba.0, bb.0) && close(ba.1, bb.1) && close(ba.2, bb.2) && close(ba.3, bb.3),
            "slide {}: {ba:?} vs {bb:?}",
            i + 1
        );
        let (ia, ib) = (a.composite(), b.composite());
        let same = ia.pixels().zip(ib.pixels()).filter(|(p, q)| p == q).count();
        let ratio = same as f64 / (1920.0 * 1080.0);
        assert!(
            ratio > 0.995,
            "slide {}: only {:.4} of pixels identical",
            i + 1,
            ratio
        );
    }
}

#[test]
fn rotation_turns_the_box_about_its_centre() {
    let mut f = fonts();
    let mut d = load("render_1024x768.pro6");
    let upright = render(&mut f, &d, 0, 1024, 768).unwrap();
    let (x0, y0, x1, y1) = ink_bbox(&upright);
    let set = |d: &mut doc::Document, deg: f64| {
        for e in &mut d.groups[0].slides[0].elements {
            if let doc::Element::Text(t) = e {
                t.rotation = deg;
            }
        }
    };
    // 360° is no rotation.
    set(&mut d, 360.0);
    let same = render(&mut f, &d, 0, 1024, 768).unwrap();
    assert_eq!(same.text, upright.text);
    assert!(
        same.warnings
            .iter()
            .all(|w| !w.message.contains("rotation"))
    );

    // 90°: the ink's width and height swap around the same centre (one text box).
    set(&mut d, 90.0);
    let turned = render(&mut f, &d, 0, 1024, 768).unwrap();
    assert!(
        turned
            .warnings
            .iter()
            .all(|w| !w.message.contains("rotation"))
    );
    let (a0, b0, a1, b1) = ink_bbox(&turned);
    let near_u = |a: u32, b: u32| a.abs_diff(b) <= 4;
    assert!(
        near_u(a1 - a0, y1 - y0) && near_u(b1 - b0, x1 - x0),
        "{:?} vs {:?}",
        (a0, b0, a1, b1),
        (x0, y0, x1, y1)
    );
    let centre = |p: u32, q: u32| (p + q) / 2;
    let t = d.groups[0].slides[0].text_boxes().next().unwrap().frame;
    let (bcx, bcy) = ((t.x + t.width / 2.0) as u32, (t.y + t.height / 2.0) as u32);
    // Ink centre relative to the box centre turns by 90° clockwise: (dx, dy) -> (-dy, dx).
    let (dx, dy) = (
        centre(x0, x1) as i64 - bcx as i64,
        centre(y0, y1) as i64 - bcy as i64,
    );
    let (ex, ey) = (
        centre(a0, a1) as i64 - bcx as i64,
        centre(b0, b1) as i64 - bcy as i64,
    );
    assert!(
        (ex + dy).abs() <= 3 && (ey - dx).abs() <= 3,
        "({dx},{dy}) -> ({ex},{ey})"
    );
}

#[test]
fn editing_layout_matches_the_drawn_text() {
    use doc::rich::Pos;
    let mut f = fonts();
    let d = load("render_1024x768.pro6");
    let slide = &d.groups[0].slides[0];
    let el = slide.text_boxes().next().unwrap();
    let frame = render(&mut f, &d, 0, 1024, 768).unwrap();
    let (x0, y0, x1, y1) = ink_bbox(&frame);
    let lay = render::layout_box(&mut f, &d.path, &slide.uuid, el, 1.0).unwrap();
    assert!(!lay.overflows());
    // The caret's lines cover the ink (box-local → canvas: add the frame origin).
    let (bx, by) = (el.frame.x as f32, el.frame.y as f32);
    let (cx, ctop, ch) = lay.caret(Pos::new(0, 0));
    assert!(ch > 30.0, "line height {ch}");
    assert!(
        (by + ctop) as u32 <= y0 + 2,
        "{} vs ink top {y0}",
        by + ctop
    );
    assert!(
        (bx + cx) as u32 <= x0 + 2,
        "start of the centred first line"
    );
    let last = doc::rich::end(&el.paragraphs);
    let (ex, etop, eh) = lay.caret(last);
    assert!(
        (by + etop + eh) as u32 + 2 >= y1,
        "last line reaches the ink bottom"
    );
    assert!((bx + ex) as u32 + 4 >= x1 || etop > ctop, "end of a line");
    // Clicking where a caret is drawn puts the caret there.
    let text = doc::rich::text(&el.paragraphs[0]);
    for off in (0..=text.len()).filter(|o| text.is_char_boundary(*o)) {
        let p = Pos::new(0, off);
        let (x, top, h) = lay.caret(p);
        assert_eq!(lay.hit(x + 0.1, top + h / 2.0), p, "offset {off}");
    }
    // A selection of the whole text spans every line.
    let sel = lay.selection(Pos::new(0, 0), last);
    assert!(sel.len() >= 2, "{sel:?}");
}

/// E2 acceptance: every text attribute survives saving and renders the same after
/// reopening (the editor canvas, output window and `pp6tool render` all draw with
/// `render_slide`, so equal documents give equal pixels everywhere).
#[test]
fn styled_text_renders_the_same_after_saving_and_reopening() {
    let mut f = fonts();
    let dir = std::env::temp_dir().join(format!("render-e2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("styled.pro6");
    std::fs::copy(fixtures().join("render_1024x768.pro6"), &file).unwrap();
    let mut d = doc::open(&file).unwrap();
    {
        let t = first_box(&mut d, 0);
        let base = t.paragraphs[0].runs[0].clone();
        t.paragraphs = vec![
            doc::Paragraph {
                align: doc::Align::Left,
                line_spacing: 1.4,
                runs: vec![
                    doc::Run {
                        text: "Bold ".into(),
                        bold: true,
                        ..base.clone()
                    },
                    doc::Run {
                        text: "serif".into(),
                        font: "Liberation Serif".into(),
                        color: Some([255, 200, 0]),
                        size: 60.0,
                        ..base.clone()
                    },
                ],
            },
            doc::Paragraph {
                align: doc::Align::Right,
                line_spacing: 1.0,
                runs: vec![doc::Run {
                    text: "under\nspaced".into(),
                    underline: true,
                    letter_spacing: 6.0,
                    ..base
                }],
            },
        ];
        t.valign = doc::VAlign::Top;
        t.outline = Some(doc::Outline {
            width: 3.0,
            color: doc::Rgba {
                r: 0.0,
                g: 0.3,
                b: 1.0,
                a: 1.0,
            },
        });
        t.shadow = Some(doc::Shadow {
            color: doc::Rgba {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 0.7,
            },
            blur: 4.0,
            offset: (5.0, 5.0),
        });
        t.rotation = 8.0;
    }
    let before = render(&mut f, &d, 0, 1280, 720).unwrap();
    assert!(before.warnings.is_empty(), "{:?}", before.warnings);
    d.mode = doc::Mode::Native;
    let saved = doc::save(&d).unwrap();
    let back = doc::open(&file).unwrap();
    assert_eq!(back.path, saved.path, "reopened as the .opres");
    let after = render(&mut f, &back, 0, 1280, 720).unwrap();
    assert!(after.text == before.text, "same pixels after reopening");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A 64×32 picture: left half red, right half blue.
fn write_test_png(path: &std::path::Path) {
    let img = image::RgbaImage::from_fn(64, 32, |x, _| {
        if x < 32 {
            image::Rgba([220, 40, 40, 255])
        } else {
            image::Rgba([40, 40, 220, 255])
        }
    });
    img.save(path).unwrap();
}

fn px(f: &SlideFrame, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * f.width + x) * 4) as usize;
    [f.text[i], f.text[i + 1], f.text[i + 2], f.text[i + 3]]
}

fn image_box(media: doc::MediaRef, frame: doc::Rect) -> doc::ImageBox {
    doc::ImageBox {
        uuid: doc::new_uuid(),
        name: "test".into(),
        frame,
        rotation: 0.0,
        opacity: 1.0,
        locked: false,
        hidden: false,
        flip_h: false,
        flip_v: false,
        media,
        adjust: doc::Adjust::default(),
        src: None,
    }
}

/// E3 acceptance: an adjusted background and foreground image render the same in
/// every path (all use `render_slide`), including after saving and reopening.
#[test]
fn adjusted_images_render_the_same_after_saving_and_reopening() {
    let mut f = fonts();
    let dir = std::env::temp_dir().join(format!("render-e3-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Media")).unwrap();
    write_test_png(&dir.join("Media/pic.png"));
    let file = dir.join("pics.pro6");
    std::fs::copy(fixtures().join("render_1024x768.pro6"), &file).unwrap();
    let mut d = doc::open(&file).unwrap();
    let media = doc::MediaRef {
        path: "Media/pic.png".into(),
        sha256: None,
    };
    {
        let s = &mut d.groups[0].slides[0];
        s.elements.clear();
        s.background_image = Some(doc::BackgroundImage {
            media: media.clone(),
            fit: doc::Fit::Stretch,
            adjust: doc::Adjust::default(),
        });
        let mut fg = image_box(
            media.clone(),
            doc::Rect {
                x: 412.0,
                y: 284.0,
                width: 200.0,
                height: 100.0,
            },
        );
        fg.flip_h = true;
        s.elements.push(doc::Element::Image(Box::new(fg)));
    }
    let plain = render(&mut f, &d, 0, 1024, 768).unwrap();
    assert!(plain.warnings.is_empty(), "{:?}", plain.warnings);
    // Background stretched: red on the left, blue on the right, edge to edge.
    assert_eq!(px(&plain, 5, 5)[..3], [220, 40, 40]);
    assert_eq!(px(&plain, 1018, 760)[..3], [40, 40, 220]);
    // Foreground flipped: blue on its left.
    assert_eq!(px(&plain, 420, 330)[..3], [40, 40, 220]);
    assert_eq!(px(&plain, 600, 330)[..3], [220, 40, 40]);

    {
        let s = &mut d.groups[0].slides[0];
        if let Some(b) = s.background_image.as_mut() {
            b.adjust = doc::Adjust {
                saturation: -1.0,
                vignette: 1.0,
                ..doc::Adjust::default()
            };
        }
        if let doc::Element::Image(i) = &mut s.elements[0] {
            i.adjust.brightness = 0.5;
            i.opacity = 0.5;
        }
    }
    let before = render(&mut f, &d, 0, 1024, 768).unwrap();
    let bg = px(&before, 200, 384);
    assert_eq!(bg[0], bg[1], "background desaturated: {bg:?}");
    assert!(px(&before, 1, 1)[0] < 40, "vignette darkens the corner");
    assert_ne!(
        px(&before, 420, 330),
        px(&plain, 420, 330),
        "foreground adjusted"
    );
    d.mode = doc::Mode::Native;
    let saved = doc::save(&d).unwrap();
    let back = doc::open(&file).unwrap();
    assert_eq!(back.path, saved.path);
    let after = render(&mut f, &back, 0, 1024, 768).unwrap();
    assert!(after.text == before.text, "same pixels after reopening");
    // Another output size (the editor canvas draws at its on-screen size).
    let small = render(&mut f, &back, 0, 512, 384).unwrap();
    assert_eq!(px(&small, 2, 192)[0], px(&small, 2, 192)[1]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn missing_or_changed_images_are_reported_and_the_rest_still_draws() {
    let mut f = fonts();
    let dir = std::env::temp_dir().join(format!("render-e3m-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    write_test_png(&dir.join("pic.png"));
    let mut d = load("render_1024x768.pro6");
    d.path = dir.join("x.opres");
    let frame = doc::Rect {
        x: 0.0,
        y: 0.0,
        width: 64.0,
        height: 32.0,
    };
    let missing = image_box(
        doc::MediaRef {
            path: r"C:\Users\Someone\Graphics\gone.png".into(),
            sha256: None,
        },
        frame,
    );
    let changed = image_box(
        doc::MediaRef {
            path: "pic.png".into(),
            sha256: Some("00".repeat(32)),
        },
        frame,
    );
    let (mu, cu) = (missing.uuid.clone(), changed.uuid.clone());
    d.groups[0].slides[0]
        .elements
        .push(doc::Element::Image(Box::new(missing)));
    d.groups[0].slides[0]
        .elements
        .push(doc::Element::Image(Box::new(changed)));
    let fr = render(&mut f, &d, 0, 1024, 768).unwrap();
    let w = |u: &str| fr.warnings.iter().find(|w| w.element_uuid == u).unwrap();
    assert!(w(&mu).media);
    assert!(w(&mu).message.contains("gone.png"), "{}", w(&mu).message);
    assert!(!w(&cu).media, "a changed file is still drawn");
    assert!(w(&cu).message.contains("changed"));
    assert_eq!(
        px(&fr, 5, 5)[..3],
        [220, 40, 40],
        "the changed one is drawn"
    );
    ink_bbox(&fr);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "timing, run with --release --ignored --nocapture"]
fn adjust_timing() {
    let mut f = fonts();
    let dir = std::env::temp_dir().join(format!("render-e3t-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let big = image::RgbaImage::from_fn(4000, 3000, |x, y| {
        image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255])
    });
    big.save(dir.join("big.jpg")).ok();
    big.save(dir.join("big.png")).unwrap();
    let mut d = load("render_1024x768.pro6");
    d.path = dir.join("x.opres");
    d.groups[0].slides[0].background_image = Some(doc::BackgroundImage {
        media: doc::MediaRef {
            path: "big.png".into(),
            sha256: None,
        },
        fit: doc::Fit::Fill,
        adjust: doc::Adjust::default(),
    });
    for i in 0..6 {
        if let Some(b) = d.groups[0].slides[0].background_image.as_mut() {
            b.adjust.brightness = f64::from(i) / 10.0;
            b.adjust.vignette = 0.5;
        }
        let t = std::time::Instant::now();
        let fr = render(&mut f, &d, 0, 1920, 1080).unwrap();
        println!(
            "render {i}: {:?} (render_time {:?})",
            t.elapsed(),
            fr.render_time
        );
    }
    d.groups[0].slides[0].background_image = None;
    let t = std::time::Instant::now();
    render(&mut f, &d, 0, 1920, 1080).unwrap();
    println!("text only: {:?}", t.elapsed());
    let img = image::RgbaImage::from_pixel(1920, 1080, image::Rgba([100, 100, 100, 255]));
    let t = std::time::Instant::now();
    let a = doc::Adjust {
        brightness: 0.2,
        vignette: 0.5,
        ..doc::Adjust::default()
    };
    render::adjusted_premultiplied(&img, &a, 1.0);
    println!("adjust only: {:?}", t.elapsed());
    std::fs::remove_dir_all(&dir).unwrap();
}
