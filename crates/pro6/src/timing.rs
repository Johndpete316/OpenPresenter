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

use crate::model::{Document, Slide};
use crate::raw::{RawElement, RawNode};

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

/// An enabled "Go to Next" timer cue (the only kind that's used).
fn is_go_to_next(e: &RawElement) -> bool {
    e.name == "RVSlideTimerCue" && e.attr("actionType").map(|a| a.raw_value.trim()) == Some("9")
}

/// `10` for whole seconds (as PP6 writes them), otherwise e.g. `7.5`.
fn seconds(d: Duration) -> String {
    let s = (d.as_secs_f64() * 1000.0).round() / 1000.0;
    format!("{s}")
}

impl Slide {
    /// Set or remove the slide's "Go to Next" timer in its `cues`, in the verified
    /// format (see the module docs). An existing timer cue keeps its UUID and any
    /// other attributes; other cues are left alone.
    pub fn set_timer(&mut self, timer: Option<SlideTimer>) {
        let children = &mut self.extra.element.children;
        let pos = children
            .iter()
            .position(|n| matches!(n, RawNode::Element(e) if e.ivar_name() == Some("cues")));
        let pos = match (pos, timer) {
            (Some(p), _) => p,
            (None, None) => return,
            (None, Some(_)) => {
                children.insert(
                    0,
                    RawNode::Element(RawElement::new("array", &[("rvXMLIvarName", "cues")])),
                );
                0
            }
        };
        let Some(RawNode::Element(cues)) = children.get_mut(pos) else {
            return;
        };
        match timer {
            Some(t) => {
                let duration = seconds(t.duration);
                let loops = if t.loop_to_beginning { "true" } else { "false" };
                let existing = cues.children.iter_mut().find_map(|n| match n {
                    RawNode::Element(e) if is_go_to_next(e) => Some(e),
                    _ => None,
                });
                match existing {
                    Some(e) => {
                        e.set_attr("enabled", "1");
                        e.set_attr("loopToBeginning", loops);
                        e.set_attr("duration", &duration);
                    }
                    None => {
                        let uuid = crate::ops::new_uuid();
                        cues.children.push(RawNode::Element(RawElement::new(
                            "RVSlideTimerCue",
                            &[
                                ("UUID", uuid.as_str()),
                                ("displayName", "Go to Next"),
                                ("actionType", "9"),
                                ("enabled", "1"),
                                ("timeStamp", "0"),
                                ("delayTime", "0"),
                                ("loopToBeginning", loops),
                                ("duration", &duration),
                            ],
                        )));
                        cues.self_closing = false;
                    }
                }
            }
            None => {
                cues.children
                    .retain(|n| !matches!(n, RawNode::Element(e) if is_go_to_next(e)));
                // Back to `<array rvXMLIvarName="cues" />` when nothing is left.
                if !cues
                    .children
                    .iter()
                    .any(|n| matches!(n, RawNode::Element(_)))
                {
                    cues.children.clear();
                    cues.self_closing = true;
                }
            }
        }
    }
}
