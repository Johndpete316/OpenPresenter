//! Per-slide auto-advance timers.
//!
//! **Verified against real PP6 files** (2026-11-20, `AA001 Welcome.pro6`): each
//! slide's `cues` array holds an
//! `<RVSlideTimerCue displayName="Go to Next" actionType="9" enabled="1" timeStamp="0"
//! delayTime="0" loopToBeginning="false" duration="10" />`. The slide stays up for
//! `duration` seconds and then goes to the next slide; with `loopToBeginning="true"`
//! (set on the last slide) it goes back to the first slide instead. The timeline
//! (`RVTimeline`) is empty in that file.
//!
//! Only enabled "Go to Next" timers (`actionType="9"`) are used. `delayTime` was 0
//! in every sample, so its meaning is unverified; a non-zero value is reported.
//! Other cue types are counted, never guessed.

use std::collections::HashMap;
use std::time::Duration;

use crate::model::Document;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlideTimer {
    pub duration: Duration,
    /// After this slide, go back to the first slide instead of the next.
    pub loop_to_beginning: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SlideTimings {
    /// Slide UUID → its timer.
    pub timers: HashMap<String, SlideTimer>,
    /// Cues that aren't enabled "Go to Next" timers (other cue types, disabled, bad values).
    pub other_cues: usize,
    /// Timers with a non-zero `delayTime` (meaning unverified; the delay is not applied).
    pub with_delay: usize,
}

impl Document {
    /// Slide timers, if any slide has one.
    pub fn slide_timings(&self) -> Option<SlideTimings> {
        let mut t = SlideTimings::default();
        for slide in self.slides() {
            let Some(cues) = slide.cues_raw() else {
                continue;
            };
            for (_, cue) in cues.elements() {
                let attr = |n: &str| cue.attr(n).map(|a| a.raw_value.trim().to_owned());
                let is_timer = cue.name == "RVSlideTimerCue"
                    && attr("actionType").as_deref() == Some("9")
                    && matches!(attr("enabled").as_deref(), Some("1" | "true") | None);
                let duration = attr("duration")
                    .and_then(|d| d.parse::<f64>().ok())
                    .filter(|d| d.is_finite() && *d > 0.0);
                match (is_timer, duration) {
                    (true, Some(d)) => {
                        if attr("delayTime")
                            .and_then(|v| v.parse::<f64>().ok())
                            .is_some_and(|v| v != 0.0)
                        {
                            t.with_delay += 1;
                        }
                        let loop_to_beginning = attr("loopToBeginning").as_deref() == Some("true");
                        t.timers.entry(slide.uuid.clone()).or_insert(SlideTimer {
                            duration: Duration::from_secs_f64(d),
                            loop_to_beginning,
                        });
                    }
                    _ => t.other_cues += 1,
                }
            }
        }
        (!t.timers.is_empty()).then_some(t)
    }
}
