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
fn bottom_aligned_bold_and_unsupported_shadow_warning() {
    let mut f = fonts();
    let d = load("render_1024x768.pro6");
    let frame = render(&mut f, &d, 2, 1024, 768).unwrap();
    let (_, _, _, y1) = ink_bbox(&frame);
    assert!(y1 > 684 - 30 && y1 < 684, "bottom-aligned: {y1}");
    let msgs: Vec<_> = frame.warnings.iter().map(|w| w.message.as_str()).collect();
    assert_eq!(msgs, ["text shadow isn't drawn yet"]);
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
