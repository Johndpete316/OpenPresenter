#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Rescaling: geometry, RTF sizes, effects, and clean round-trips.

use std::path::PathBuf;

use pro6::diff::semantic_diff;
use pro6::rescale::{Mode, rescale};
use pro6::{CanvasSize, WinPayloads};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

#[test]
fn fit_to_wider_canvas_centres_and_scales() {
    let mut doc = pro6::load(&fixture("render_1024x768.pro6")).unwrap();
    let original = doc.clone();
    let r = rescale(
        &mut doc,
        CanvasSize {
            width: 1920,
            height: 1080,
        },
        Mode::Fit,
        WinPayloads::Drop,
    );
    assert_eq!(
        (r.scale_x, r.scale_y, r.offset_x, r.offset_y),
        (1.40625, 1.40625, 240.0, 0.0)
    );
    assert_eq!(
        doc.canvas,
        CanvasSize {
            width: 1920,
            height: 1080
        }
    );
    let p = doc.text_elements().next().unwrap().position.value;
    assert_eq!(
        (p.x, p.y, p.width, p.height),
        (
            112.0 * 1.40625 + 240.0,
            84.0 * 1.40625,
            800.0 * 1.40625,
            600.0 * 1.40625
        )
    );
    assert_eq!(r.font_sizes, [(64, 90), (72, 101), (80, 113), (96, 135)]);
    let fs: Vec<u32> = doc
        .text_elements()
        .next()
        .unwrap()
        .rtf_doc(&doc.path)
        .unwrap()
        .unwrap()
        .runs()
        .map(|r| r.size_half_pt)
        .collect();
    assert_eq!(fs, [135, 135]);
    assert_eq!(r.shadows, 4);
    let s = doc
        .text_elements()
        .next()
        .unwrap()
        .shadow
        .as_ref()
        .unwrap()
        .value
        .clone();
    assert_eq!(s.offset, Some((4.0 * 1.40625, -4.0 * 1.40625)));

    // Round-trip clean: writing and re-reading is stable, and only scaled things changed.
    let out = pro6::write_string(&doc);
    let back = pro6::parse_str(&out, &doc.path).unwrap();
    assert_eq!(pro6::write_string(&back), out, "idempotent");
    let diffs = semantic_diff(&pro6::to_raw(&original).root, &pro6::to_raw(&back).root);
    for d in &diffs {
        let ok = d.xml_path == "RVPresentationDocument"
            || d.xml_path.ends_with("RVRect3D[position]")
            || d.xml_path.ends_with("shadow[shadow]")
            || d.xml_path.ends_with("NSString[RTFData]");
        assert!(ok, "unexpected change: {d}");
    }
    assert!(diffs.iter().any(|d| d.message.contains("`width`")));
}

#[test]
fn same_aspect_is_proportional_and_effects_scale() {
    let mut doc = pro6::load(&fixture("two_fonts_1920x1080.pro6")).unwrap();
    let r = rescale(
        &mut doc,
        CanvasSize {
            width: 1280,
            height: 720,
        },
        Mode::Fit,
        WinPayloads::Drop,
    );
    assert_eq!((r.offset_x, r.offset_y), (0.0, 0.0));
    assert!((r.scale_text - 2.0 / 3.0).abs() < 1e-12);
    assert_eq!(r.font_sizes, [(80, 53), (96, 64), (120, 80)]);
    let t = doc.text_elements().next().unwrap();
    assert!((t.stroke.as_ref().unwrap().width.as_ref().unwrap().value - 4.0 / 3.0).abs() < 1e-12);
    assert_eq!(r.strokes, 2);
}

#[test]
fn stretch_scales_axes_independently_and_text_by_the_smaller() {
    let mut doc = pro6::load(&fixture("render_1024x768.pro6")).unwrap();
    let r = rescale(
        &mut doc,
        CanvasSize {
            width: 2048,
            height: 768,
        },
        Mode::Stretch,
        WinPayloads::Keep,
    );
    assert_eq!((r.scale_x, r.scale_y, r.scale_text), (2.0, 1.0, 1.0));
    let p = doc.text_elements().next().unwrap().position.value;
    assert_eq!((p.x, p.width, p.y, p.height), (224.0, 1600.0, 84.0, 600.0));
    assert!(
        r.font_sizes.is_empty(),
        "text factor 1.0 leaves sizes alone"
    );
}

#[test]
fn unknowns_and_win_payloads_are_reported() {
    let mut doc = pro6::load(&fixture("unknowns.pro6")).unwrap();
    let r = rescale(
        &mut doc,
        CanvasSize {
            width: 1920,
            height: 1080,
        },
        Mode::Fit,
        WinPayloads::Drop,
    );
    assert_eq!(r.win_payloads_dropped, 1);
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("SyntheticImageElement")),
        "{:?}",
        r.warnings
    );
    let mut same = pro6::load(&fixture("unknowns.pro6")).unwrap();
    let canvas = same.canvas;
    assert!(rescale(&mut same, canvas, Mode::Fit, WinPayloads::Drop).unchanged);
}
