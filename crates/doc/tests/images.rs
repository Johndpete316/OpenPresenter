#![allow(clippy::unwrap_used, clippy::expect_used)]
//! E3: image elements from `.pro6`, image edits, background images.

use std::path::PathBuf;

use doc::{
    Adjust, BackgroundImage, Document, Edit, Element, ExportChoice, Fit, ImageBox, MediaRef,
};
use pretty_assertions::assert_eq;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// An `RVImageElement` as PP6 writes it (structure copied from a real file; the
/// path is made up).
const IMAGE_XML: &str = r#"<RVImageElement displayName="Default" UUID="1e1990d2-2b64-4743-a1b9-001d6218a151" typeID="0" displayDelay="0" locked="false" persistent="0" fromTemplate="false" opacity="1" source="C%3A%5CUsers%5CSound%5CDocuments%5CProPresenter6%5CGraphics%5CLogo%20Card.png" bezelRadius="0" rotation="0" drawingFill="false" drawingShadow="false" drawingStroke="false" fillColor="1 1 1 1" scaleBehavior="0" flippedHorizontally="0" flippedVertically="0" scaleSize="{1, 1}" imageOffset="{0, 0}" manufactureURL="" manufactureName="" format="">
              <RVRect3D rvXMLIvarName="position">{0 0 0 1024 768}</RVRect3D>
              <shadow rvXMLIvarName="shadow">0|0 0 0 1|{4.94974746830583, -4.94974746830583}</shadow>
              <dictionary rvXMLIvarName="stroke">
                <NSColor rvXMLDictionaryKey="RVShapeElementStrokeColorKey">0 0 0 1</NSColor>
                <NSNumber rvXMLDictionaryKey="RVShapeElementStrokeWidthKey" hint="double">0</NSNumber>
              </dictionary>
            </RVImageElement>"#;

/// `render_1024x768.pro6` with an image under the first slide's text.
fn with_image() -> (Document, String) {
    let path = fixture("render_1024x768.pro6");
    let xml = std::fs::read_to_string(&path).unwrap();
    let marker = r#"<array rvXMLIvarName="displayElements">"#;
    let at = xml.find(marker).unwrap() + marker.len();
    let xml = format!("{}\n            {IMAGE_XML}{}", &xml[..at], &xml[at..]);
    let p = pro6::parse_str(&xml, &path).unwrap();
    (doc::from_pro6(p), xml)
}

fn through_pro6(d: &Document) -> Document {
    let out = doc::to_pro6(d).unwrap();
    let xml = pro6::write_string(&out);
    doc::from_pro6(pro6::parse_str(&xml, &d.path).unwrap())
}

fn image(d: &Document) -> &ImageBox {
    d.groups[0].slides[0].images().next().unwrap()
}

#[test]
fn pro6_images_import_and_round_trip_unchanged() {
    let (d, xml) = with_image();
    let i = image(&d);
    assert_eq!(
        i.media.path,
        r"C:\Users\Sound\Documents\ProPresenter6\Graphics\Logo Card.png"
    );
    assert_eq!(i.media.file_name(), "Logo Card.png");
    assert_eq!((i.frame.width, i.frame.height), (1024.0, 768.0));
    assert!(matches!(
        d.groups[0].slides[0].elements[0],
        Element::Image(_)
    ));
    assert_eq!(
        pro6::write_string(&doc::to_pro6(&d).unwrap()),
        xml,
        "untouched: byte-identical"
    );
}

#[test]
fn unusual_pro6_images_stay_raw() {
    let path = fixture("render_1024x768.pro6");
    let xml = std::fs::read_to_string(&path).unwrap();
    let marker = r#"<array rvXMLIvarName="displayElements">"#;
    let at = xml.find(marker).unwrap() + marker.len();
    let odd = IMAGE_XML.replace(r#"scaleBehavior="0""#, r#"scaleBehavior="3""#);
    let xml = format!("{}\n{odd}{}", &xml[..at], &xml[at..]);
    let d = doc::from_pro6(pro6::parse_str(&xml, &path).unwrap());
    assert!(
        matches!(&d.groups[0].slides[0].elements[0], Element::Pro6(p) if p.kind == "RVImageElement")
    );
}

#[test]
fn moving_rotating_and_fading_a_pro6_image_is_written_back() {
    let (mut d, _) = with_image();
    let frame = doc::Rect {
        x: 10.0,
        y: 20.0,
        width: 300.0,
        height: 200.0,
    };
    for e in [
        Edit::SetFrame {
            group: 0,
            slide: 0,
            element: 0,
            frame,
        },
        Edit::SetRotation {
            group: 0,
            slide: 0,
            element: 0,
            degrees: 15.0,
        },
        Edit::SetOpacity {
            group: 0,
            slide: 0,
            element: 0,
            opacity: 0.5,
        },
    ] {
        assert_eq!(doc::check(&d, &e), doc::Check::Ok, "{e:?} is .pro6-safe");
        e.apply(&mut d).unwrap();
    }
    let back = through_pro6(&d);
    let i = image(&back);
    assert_eq!(i.frame, frame);
    assert_eq!(i.rotation, 15.0);
    assert_eq!(i.opacity, 0.5);
    assert_eq!(i.media, image(&d).media);
}

#[test]
fn inserting_images_needs_native_and_export_leaves_them_out() {
    let d = doc::load(&fixture("song_1024x768.pro6")).unwrap();
    let new = ImageBox {
        uuid: doc::new_uuid(),
        name: "Logo".into(),
        frame: doc::Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 50.0,
        },
        rotation: 0.0,
        opacity: 1.0,
        locked: false,
        hidden: false,
        flip_h: false,
        flip_v: false,
        media: MediaRef {
            path: "Media/logo-12345678.png".into(),
            sha256: Some("ab".repeat(32)),
        },
        adjust: Adjust::default(),
        src: None,
    };
    let add = Edit::AddElement {
        group: 0,
        slide: 0,
        at: None,
        element: Element::Image(Box::new(new.clone())),
    };
    let doc::Check::NeedsNative { features, slides } = doc::check(&d, &add) else {
        panic!("images are unverified in .pro6");
    };
    assert_eq!(features, vec![doc::Feature::ImageElement]);
    let mut n = d.clone();
    add.apply(&mut n).unwrap();
    doc::mark_native(&mut n, &slides);
    assert_eq!(
        add.apply(&mut n.clone()),
        Err(doc::EditError::NotFound {
            what: "new element ID".into()
        }),
        "the same element twice is refused"
    );
    let adjust = Adjust {
        brightness: 0.3,
        vignette: 0.5,
        ..Adjust::default()
    };
    let last = n.groups[0].slides[0].elements.len() - 1;
    let e = Edit::SetImageAdjust {
        group: 0,
        slide: 0,
        element: last,
        adjust,
    };
    assert!(e.features().contains(&doc::Feature::Vignette));
    e.apply(&mut n).unwrap();
    // .opres keeps everything.
    let json = doc::file::to_json(&n);
    let back = doc::file::from_json(&json, &n.path).unwrap();
    assert_eq!(
        back.groups[0].slides[0].images().next().unwrap().adjust,
        adjust
    );
    // .pro6 export: written without the image.
    let (out, report) = doc::export_pro6(&n, ExportChoice::DropUnsupported).unwrap();
    assert_eq!(report.left_out.len(), 1);
    let back = doc::from_pro6(out);
    assert_eq!(back.groups[0].slides[0].images().count(), 0);
    assert_eq!(
        back.groups[0].slides[0].text_boxes().count(),
        d.groups[0].slides[0].text_boxes().count()
    );
}

#[test]
fn set_as_background_is_one_step_and_removes_the_element() {
    let (d, _) = with_image();
    let i = image(&d).clone();
    let op = Edit::Batch {
        label: "Set as background".into(),
        edits: vec![
            Edit::RemoveElement {
                group: 0,
                slide: 0,
                element: 0,
            },
            Edit::SetBackgroundImage {
                group: 0,
                slide: 0,
                image: Some(BackgroundImage {
                    media: i.media.clone(),
                    fit: Fit::Fill,
                    adjust: Adjust::default(),
                }),
            },
        ],
    };
    assert!(op.features().contains(&doc::Feature::ImageBackground));
    let mut n = d.clone();
    let mut h = doc::History::default();
    h.record(op.label(), std::sync::Arc::new(n.clone()));
    op.apply(&mut n).unwrap();
    let s = &n.groups[0].slides[0];
    assert_eq!(s.images().count(), 0);
    assert_eq!(s.background_image.as_ref().unwrap().media, i.media);
    let before = h.undo(std::sync::Arc::new(n.clone())).unwrap();
    assert_eq!(*before, d, "one undo restores it");
}

#[test]
fn adjustments_are_clamped_and_nan_is_refused() {
    let (mut d, _) = with_image();
    let wild = Adjust {
        brightness: 7.0,
        vignette: -2.0,
        ..Adjust::default()
    };
    Edit::SetImageAdjust {
        group: 0,
        slide: 0,
        element: 0,
        adjust: wild,
    }
    .apply(&mut d)
    .unwrap();
    assert_eq!(image(&d).adjust.brightness, 1.0);
    assert_eq!(image(&d).adjust.vignette, 0.0);
    let bad = Edit::SetImageAdjust {
        group: 0,
        slide: 0,
        element: 0,
        adjust: Adjust {
            contrast: f64::NAN,
            ..Adjust::default()
        },
    };
    assert!(bad.apply(&mut d.clone()).is_err());
}

#[test]
fn media_paths_resolve_next_to_the_document() {
    let m = MediaRef {
        path: "Media/a.png".into(),
        sha256: None,
    };
    assert_eq!(
        m.resolve(std::path::Path::new("/lib/songs/x.opres")),
        PathBuf::from("/lib/songs/Media/a.png")
    );
    let abs = MediaRef {
        path: r"C:\Pics\b.png".into(),
        sha256: None,
    };
    assert_eq!(
        abs.resolve(std::path::Path::new("/lib/x.pro6")),
        PathBuf::from(r"C:\Pics\b.png")
    );
    assert_eq!(abs.file_name(), "b.png");
}

#[test]
fn layer_edits_reorder_lock_hide_and_rename() {
    let (d, _) = with_image();
    // Image 0 (behind), text 1 (in front).
    let mut n = d.clone();
    let front = Edit::MoveElement {
        group: 0,
        slide: 0,
        from: 0,
        to: 1,
    };
    assert_eq!(
        doc::check(&n, &front),
        doc::Check::Ok,
        "order is .pro6-safe"
    );
    front.apply(&mut n).unwrap();
    assert!(matches!(
        n.groups[0].slides[0].elements[1],
        Element::Image(_)
    ));
    let back = through_pro6(&n);
    assert!(
        matches!(back.groups[0].slides[0].elements[1], Element::Image(_)),
        "order written to .pro6"
    );
    let rename = Edit::RenameElement {
        group: 0,
        slide: 0,
        element: 1,
        name: "Logo".into(),
    };
    rename.apply(&mut n).unwrap();
    assert_eq!(image(&through_pro6(&n)).name, "Logo");
    let lock = Edit::SetLocked {
        group: 0,
        slide: 0,
        element: 1,
        locked: true,
    };
    assert!(
        matches!(doc::check(&n, &lock), doc::Check::NeedsNative { .. }),
        "lock is unverified"
    );
    let hide = Edit::SetHidden {
        group: 0,
        slide: 0,
        element: 1,
        hidden: true,
    };
    let doc::Check::NeedsNative { slides, .. } = doc::check(&n, &hide) else {
        panic!("hide is unverified");
    };
    hide.apply(&mut n).unwrap();
    doc::mark_native(&mut n, &slides);
    assert!(n.groups[0].slides[0].elements[1].hidden());
    // A .pro6 export leaves hidden elements out (they aren't drawn either).
    let (out, _) = doc::export_pro6(&n, ExportChoice::DropUnsupported).unwrap();
    assert_eq!(doc::from_pro6(out).groups[0].slides[0].images().count(), 0);
}
