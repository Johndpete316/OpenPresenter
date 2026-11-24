#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Slide timers: set, change and remove them, and write them to `.pro6` in the
//! format verified from a real file (`RVSlideTimerCue`, "Go to Next").

use std::path::PathBuf;
use std::time::Duration;

use doc::{Document, Edit, SlideTimer};

fn open(name: &str) -> Document {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    doc::open(&p).unwrap()
}

/// Export to `.pro6` XML and read it back, as a reopened file would be.
fn through_pro6(d: &Document) -> (String, Document) {
    let xml = pro6::write_string(&doc::to_pro6(d).unwrap());
    let back = pro6::parse_str(&xml, &d.path).unwrap();
    (xml, doc::from_pro6(back))
}

fn timer(secs: f64, loops: bool) -> Option<SlideTimer> {
    Some(SlideTimer {
        duration: Duration::from_secs_f64(secs),
        loop_to_beginning: loops,
    })
}

fn set(d: &mut Document, group: usize, slide: usize, t: Option<SlideTimer>) {
    Edit::SetSlideTimer {
        group,
        slide,
        timer: t,
    }
    .apply(d)
    .unwrap();
}

#[test]
fn a_new_timer_is_written_as_a_go_to_next_cue() {
    let mut d = open("song_1024x768.pro6");
    assert!(d.slide_timings().is_none());
    set(&mut d, 0, 0, timer(10.0, false));
    set(&mut d, 0, 1, timer(7.5, true));
    let (xml, back) = through_pro6(&d);
    assert!(
        xml.contains(r#"displayName="Go to Next" actionType="9" enabled="1" timeStamp="0" delayTime="0" loopToBeginning="false" duration="10""#),
        "the cue matches PP6's own: {xml}"
    );
    assert!(xml.contains(r#"loopToBeginning="true" duration="7.5""#));
    let slides: Vec<_> = back.slides().collect();
    assert_eq!(slides[0].timer, timer(10.0, false));
    assert_eq!(slides[1].timer, timer(7.5, true));
    // Slides that weren't touched keep their empty cues as they were.
    assert!(slides[2..].iter().all(|s| s.timer.is_none()));
    assert!(xml.contains(r#"<array rvXMLIvarName="cues" />"#));
}

#[test]
fn changing_a_timer_keeps_its_cue() {
    let mut d = open("announcements_synthetic.pro6");
    let before = pro6::write_string(&doc::to_pro6(&d).unwrap());
    let uuid_of = |xml: &str| {
        let i = xml.find("<RVSlideTimerCue").unwrap();
        let tag = &xml[i..i + xml[i..].find('>').unwrap()];
        tag.split("UUID=\"").nth(1).unwrap()[..36].to_owned()
    };
    let old = d.slides().next().unwrap().timer.unwrap();
    set(&mut d, 0, 0, timer(old.duration.as_secs_f64() + 5.0, false));
    let (xml, back) = through_pro6(&d);
    assert_eq!(uuid_of(&xml), uuid_of(&before), "same cue, same UUID");
    assert_eq!(
        back.slides().next().unwrap().timer.unwrap().duration,
        old.duration + Duration::from_secs(5)
    );
}

#[test]
fn removing_every_timer_leaves_no_timings() {
    let mut d = open("announcements_synthetic.pro6");
    let timed: Vec<(usize, usize)> = d
        .groups
        .iter()
        .enumerate()
        .flat_map(|(g, gr)| {
            gr.slides
                .iter()
                .enumerate()
                .filter(|(_, s)| s.timer.is_some())
                .map(move |(s, _)| (g, s))
        })
        .collect();
    assert!(!timed.is_empty());
    for (g, s) in timed {
        set(&mut d, g, s, None);
    }
    let (xml, back) = through_pro6(&d);
    assert!(!xml.contains("<RVSlideTimerCue"), "no timer cue left");
    assert!(back.slide_timings().is_none());
}

#[test]
fn unchanged_timers_save_byte_identical() {
    let d = open("announcements_synthetic.pro6");
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/announcements_synthetic.pro6");
    let original = std::fs::read_to_string(p).unwrap();
    assert_eq!(pro6::write_string(&doc::to_pro6(&d).unwrap()), original);
}

#[test]
fn timers_must_be_sensible() {
    let mut d = open("song_1024x768.pro6");
    for secs in [0.0, 0.1, 3601.0] {
        let e = Edit::SetSlideTimer {
            group: 0,
            slide: 0,
            timer: timer(secs, false),
        };
        assert!(e.apply(&mut d).is_err(), "{secs} s is refused");
    }
    assert!(d.slides().next().unwrap().timer.is_none());
    // A timer fits a .pro6: no "save as .opres" needed.
    let ok = Edit::SetSlideTimer {
        group: 0,
        slide: 0,
        timer: timer(10.0, false),
    };
    assert!(matches!(doc::check(&d, &ok), doc::Check::Ok));
}
