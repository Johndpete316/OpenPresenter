//! Per-slide auto-advance timing.
//!
//! **Assumed, unverified** (docs/PLAN.md Unknowns; needs the real Announcements
//! file): timing lives in `RVTimeline`'s `timeCues`. Each cue carries a
//! `timeStamp` attribute (seconds) and, somewhere below it (an attribute value
//! or an `NSString`), the UUID of the slide it triggers. A slide's duration is
//! the gap to the next cue; the last slide runs until the timeline `duration`.
//! The reading is tolerant (element names aren't checked) and anything it can't
//! interpret is counted, never guessed.

use std::collections::HashMap;
use std::time::Duration;

use crate::model::Document;
use crate::raw::{RawElement, RawNode};

#[derive(Debug, Clone, PartialEq)]
pub struct SlideTimings {
    /// Slide UUID → how long it stays up before advancing.
    pub durations: HashMap<String, Duration>,
    /// `RVTimeline@loop`: wrap from the last slide back to the first.
    pub wrap: bool,
    /// Cues that were understood (time + slide).
    pub cues: usize,
    /// Cues without a usable time or slide reference.
    pub unreadable: usize,
}

impl Document {
    /// Auto-advance timing, if the timeline has any this reader understands.
    pub fn slide_timings(&self) -> Option<SlideTimings> {
        let tl = self.timeline.as_ref()?;
        let cues = tl.extra.find_ivar("timeCues")?;
        let slide_ids: Vec<&str> = self
            .slides()
            .map(|s| s.uuid.as_str())
            .filter(|u| !u.is_empty())
            .collect();
        let mut points: Vec<(f64, String)> = Vec::new();
        let mut unreadable = 0;
        for (_, cue) in cues.elements() {
            let time = cue
                .attr("timeStamp")
                .and_then(|a| a.raw_value.trim().parse::<f64>().ok())
                .filter(|t| t.is_finite() && *t >= 0.0);
            let slide = find_slide_ref(cue, &slide_ids);
            match (time, slide) {
                (Some(t), Some(s)) => points.push((t, s)),
                _ => unreadable += 1,
            }
        }
        if points.is_empty() {
            return None;
        }
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut durations = HashMap::new();
        for (i, (t, uuid)) in points.iter().enumerate() {
            let end = points.get(i + 1).map_or(tl.duration, |n| n.0);
            let d = end - t;
            if d > 0.0 {
                durations
                    .entry(uuid.clone())
                    .or_insert(Duration::from_secs_f64(d));
            }
        }
        if durations.is_empty() {
            return None;
        }
        Some(SlideTimings {
            durations,
            wrap: tl.loop_,
            cues: points.len(),
            unreadable,
        })
    }
}

/// First slide UUID mentioned anywhere in the cue (attribute value or text).
fn find_slide_ref(el: &RawElement, ids: &[&str]) -> Option<String> {
    let hit = |v: &str| {
        ids.iter()
            .find(|id| id.eq_ignore_ascii_case(v.trim()))
            .map(|id| (*id).to_owned())
    };
    for a in &el.attrs {
        if let Some(h) = hit(&a.raw_value) {
            return Some(h);
        }
    }
    for c in &el.children {
        match c {
            RawNode::Text(t) => {
                if let Some(h) = hit(t) {
                    return Some(h);
                }
            }
            RawNode::Element(e) => {
                if let Some(h) = find_slide_ref(e, ids) {
                    return Some(h);
                }
            }
            _ => {}
        }
    }
    None
}
