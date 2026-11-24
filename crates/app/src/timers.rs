//! Slide timers (auto-advance): the Timers panel at the bottom of the operator window
//! (an alternative to Video/Image, chosen in the toolbar), the grid's right-click
//! Timer menu, and the stopwatch badge on grid tiles.
//!
//! The panel shows the selected presentation in play order as a chain,
//! `Title 10s › Verse 1 10s › … ↺ to start`. Clicking a step opens its settings.
//! Every change is one undo step and is saved straight away (a `.pro6` stays a
//! `.pro6`; timers are written as PP6's own "Go to Next" cues). A running loop takes
//! the new times from its next slide.

use std::path::Path;
use std::time::{Duration, Instant};

use doc::{Document, Edit, SlideTimer};
use egui::{Color32, RichText, Sense, Stroke, StrokeKind, Vec2};
use opcore::{ShowState, SlideRef};

use crate::theme::{brand, color, icon, section_header};
use crate::ui::{UiAction, UiState, color32};

/// What the Timers panel and the Timer menu remember between frames.
pub struct TimersState {
    /// The slide whose settings are open in the panel (UUID).
    pub open: Option<String>,
    /// Seconds being typed for the open slide, before they're committed.
    pub draft: Option<f64>,
    /// Seconds a new timer starts with (the last one set).
    pub last_secs: f64,
    /// "Every slide" bulk action: seconds, and loop back after the last slide.
    pub bulk_secs: f64,
    pub bulk_loop: bool,
}

impl Default for TimersState {
    fn default() -> Self {
        TimersState {
            open: None,
            draft: None,
            last_secs: 10.0,
            bulk_secs: 10.0,
            bulk_loop: true,
        }
    }
}

/// `10s`, `7.5s`, `1:30`.
pub fn format_secs(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s >= 60.0 {
        let whole = s.round() as u64;
        format!("{}:{:02}", whole / 60, whole % 60)
    } else if (s - s.round()).abs() < 0.05 {
        format!("{}s", s.round() as u64)
    } else {
        format!("{s:.1}s")
    }
}

/// The text on a grid tile's stopwatch badge (`None` = no timer).
pub fn badge(t: Option<&SlideTimer>) -> Option<String> {
    t.map(|t| {
        let mut s = format!("{} {}", icon::TIMER, format_secs(t.duration));
        if t.loop_to_beginning {
            s.push(' ');
            s.push(icon::REPEAT);
        }
        s
    })
}

fn timer(secs: f64, loop_to_beginning: bool) -> Option<SlideTimer> {
    Some(SlideTimer {
        duration: Duration::from_secs_f64(secs.clamp(doc::MIN_TIMER_SECS, doc::MAX_TIMER_SECS)),
        loop_to_beginning,
    })
}

/// A short name for a slide: its label, else its first line of text, else its group.
fn slide_title(d: &Document, r: SlideRef) -> String {
    let g = &d.groups[r.group];
    let s = &g.slides[r.slide];
    let text = s.plain_text();
    let first = text.lines().map(str::trim).find(|l| !l.is_empty());
    let name = if !s.label.trim().is_empty() {
        s.label.trim().to_owned()
    } else if let Some(l) = first {
        l.to_owned()
    } else if !g.name.is_empty() {
        g.name.clone()
    } else {
        "(blank)".to_owned()
    };
    let mut short: String = name.chars().take(28).collect();
    if name.chars().count() > 28 {
        short.push('…');
    }
    short
}

fn set_op(r: SlideRef, t: Option<SlideTimer>) -> Edit {
    Edit::SetSlideTimer {
        group: r.group,
        slide: r.slide,
        timer: t,
    }
}

/// Why timers can't be changed here right now, if they can't.
fn locked(show: &ShowState, path: &Path, d: &Document) -> Option<&'static str> {
    if d.theme.is_some() {
        Some("Themes don't have timers.")
    } else if show.unsaved.contains(path) {
        Some("Unsaved changes in the editor: save or revert them there first.")
    } else {
        None
    }
}

/// The Timer submenu for slide `r` in the grid's right-click menu.
pub fn slide_menu(
    ui: &mut egui::Ui,
    st: &mut UiState,
    path: &Path,
    d: &Document,
    r: SlideRef,
    actions: &mut Vec<UiAction>,
) {
    let current = d.groups[r.group].slides[r.slide].timer;
    let label = match current {
        Some(t) => format!("{} Timer: {}", icon::TIMER, format_secs(t.duration)),
        None => format!("{} Timer", icon::TIMER),
    };
    ui.menu_button(label, |ui| {
        ui.set_min_width(220.0);
        let mut secs = current.map_or(st.timers.last_secs, |t| t.duration.as_secs_f64());
        ui.horizontal(|ui| {
            ui.label("Go to the next slide after");
            ui.add(
                egui::DragValue::new(&mut secs)
                    .range(doc::MIN_TIMER_SECS..=doc::MAX_TIMER_SECS)
                    .speed(0.5)
                    .max_decimals(1)
                    .suffix(" s"),
            );
        });
        // The value is kept while the menu is open; it applies with the button.
        if current.is_none_or(|t| (t.duration.as_secs_f64() - secs).abs() > 1e-6) {
            st.timers.last_secs = secs;
        }
        let set_label = if current.is_some() {
            "Change timer"
        } else {
            "Set timer"
        };
        if ui.button(set_label).clicked() {
            let loops = current.is_some_and(|t| t.loop_to_beginning);
            crate::grid_menu::send(st, actions, path, d, set_op(r, timer(secs, loops)));
            ui.close();
        }
        if let Some(t) = current {
            let mut loops = t.loop_to_beginning;
            if ui
                .checkbox(&mut loops, "Loop to the first slide after this")
                .on_hover_text(
                    "When this slide's time is up, go back to the start instead of the next slide",
                )
                .changed()
            {
                crate::grid_menu::send(
                    st,
                    actions,
                    path,
                    d,
                    set_op(r, timer(t.duration.as_secs_f64(), loops)),
                );
                ui.close();
            }
            if ui.button("Remove timer").clicked() {
                crate::grid_menu::send(st, actions, path, d, set_op(r, None));
                ui.close();
            }
        }
    });
}

/// The Timers panel for the selected presentation.
pub fn panel(ui: &mut egui::Ui, st: &mut UiState, show: &ShowState, actions: &mut Vec<UiAction>) {
    let doc_path = show.selected.clone();
    let doc = doc_path.as_ref().and_then(|p| show.docs.get(p).cloned());
    let title = doc_path
        .as_deref()
        .map(crate::ui::title)
        .unwrap_or_default();
    section_header(
        ui,
        &if title.is_empty() {
            "Timers".to_owned()
        } else {
            format!("Timers · {title}")
        },
        |_| {},
    );
    // One scroll area filling the panel: the panel keeps the height it's dragged to,
    // and the content sits at the top.
    egui::ScrollArea::vertical()
        .id_salt("timers")
        .auto_shrink(false)
        .show(ui, |ui| body(ui, st, show, actions, doc_path, doc));
}

fn body(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    actions: &mut Vec<UiAction>,
    doc_path: Option<std::path::PathBuf>,
    doc: Option<std::sync::Arc<Document>>,
) {
    let (Some(path), Some(d)) = (doc_path, doc) else {
        hint(ui, "Select a presentation to see and set its slide timers.");
        return;
    };
    let order = opcore::slide_order(&d, show.arrangement_for(&path).as_deref());
    if order.is_empty() {
        hint(ui, "This presentation has no slides.");
        return;
    }
    let lock = locked(show, &path, &d);
    let timer_of = |r: &SlideRef| d.groups[r.group].slides[r.slide].timer;
    let timed = order.iter().filter(|r| timer_of(r).is_some()).count();
    let total: Duration = order
        .iter()
        .filter(|r| r.enabled)
        .filter_map(|r| timer_of(r).map(|t| t.duration))
        .sum();

    // Summary and the "every slide" bulk action.
    ui.horizontal_wrapped(|ui| {
        ui.add_space(8.0);
        let summary = if timed == 0 {
            "No slide has a timer: it waits for the operator.".to_owned()
        } else {
            format!(
                "{timed} of {} slides timed · {} round",
                order.len(),
                format_secs(total)
            )
        };
        ui.label(RichText::new(summary).small().weak());
        ui.separator();
        ui.add_enabled_ui(lock.is_none(), |ui| {
            ui.label("Every slide");
            ui.add(
                egui::DragValue::new(&mut st.timers.bulk_secs)
                    .range(doc::MIN_TIMER_SECS..=doc::MAX_TIMER_SECS)
                    .speed(0.5)
                    .max_decimals(1)
                    .suffix(" s"),
            );
            ui.checkbox(&mut st.timers.bulk_loop, "then loop to start");
            if ui
                .button("Apply to all")
                .on_hover_text("Give every slide this time (one undo step)")
                .clicked()
            {
                let last = order.last().map(|r| (r.group, r.slide));
                let mut seen = std::collections::HashSet::new();
                let edits: Vec<Edit> = order
                    .iter()
                    .filter(|r| seen.insert((r.group, r.slide)))
                    .map(|r| {
                        let loops = st.timers.bulk_loop && Some((r.group, r.slide)) == last;
                        set_op(*r, timer(st.timers.bulk_secs, loops))
                    })
                    .collect();
                let op = Edit::Batch {
                    label: "Set every timer".into(),
                    edits,
                };
                crate::grid_menu::send(st, actions, &path, &d, op);
            }
            if timed > 0 && ui.button("Remove all").clicked() {
                let mut seen = std::collections::HashSet::new();
                let edits: Vec<Edit> = order
                    .iter()
                    .filter(|r| timer_of(r).is_some() && seen.insert((r.group, r.slide)))
                    .map(|r| set_op(*r, None))
                    .collect();
                let op = Edit::Batch {
                    label: "Remove timers".into(),
                    edits,
                };
                crate::grid_menu::send(st, actions, &path, &d, op);
            }
        });
    });
    if let Some(why) = lock {
        hint(ui, why);
    }

    // The live slide's countdown, when this presentation is looping.
    let looping = show.looping.as_ref().filter(|l| l.path == path);
    let live_pos = show
        .live
        .as_ref()
        .filter(|l| l.path == path)
        .and_then(|l| l.position);
    let left = looping
        .and_then(|l| l.next_at)
        .map(|at| at.saturating_duration_since(Instant::now()));
    if looping.is_some() {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }

    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 6.0);
        for (pos, r) in order.iter().enumerate() {
            let s = &d.groups[r.group].slides[r.slide];
            let is_live = live_pos == Some(pos);
            let open = st.timers.open.as_deref() == Some(s.uuid.as_str());
            let time = match (s.timer, is_live.then_some(left).flatten()) {
                (Some(_), Some(l)) => {
                    format!("{} {:.1}s left", icon::TIMER, l.as_secs_f64())
                }
                (Some(t), None) => format!("{} {}", icon::TIMER, format_secs(t.duration)),
                (None, _) => "waits".to_owned(),
            };
            let resp = chip(
                ui,
                &format!("{}. {}", pos + 1, slide_title(&d, *r)),
                &time,
                color32(d.groups[r.group].color),
                is_live,
                open,
                r.enabled,
            );
            if resp.clicked() {
                st.timers.draft = None;
                st.timers.open = if open { None } else { Some(s.uuid.clone()) };
            }
            if s.timer.is_some_and(|t| t.loop_to_beginning) {
                ui.label(RichText::new(format!("{} to start", icon::REPEAT)).color(brand::ACCENT));
            } else if pos + 1 < order.len() {
                ui.label(RichText::new("›").color(Color32::GRAY));
            } else if s.timer.is_some() {
                ui.label(RichText::new(format!("{} stops", icon::STOP)).weak());
            }
        }
    });

    // Settings of the open slide.
    let Some(r) = st.timers.open.clone().and_then(|u| {
        order
            .iter()
            .copied()
            .find(|r| d.groups[r.group].slides[r.slide].uuid == u)
    }) else {
        st.timers.open = None;
        return;
    };
    let pos = order.iter().position(|x| *x == r).unwrap_or(0);
    let current = d.groups[r.group].slides[r.slide].timer;
    ui.separator();
    ui.add_enabled_ui(lock.is_none(), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new(format!("{}. {}", pos + 1, slide_title(&d, r))).strong());
            let mut on = current.is_some();
            if ui
                .checkbox(&mut on, "Timer")
                .on_hover_text("Off: the slide stays up until the operator moves on")
                .changed()
            {
                let t = on.then(|| timer(st.timers.last_secs, false)).flatten();
                crate::grid_menu::send(st, actions, &path, &d, set_op(r, t));
            }
            let Some(t) = current else { return };
            let mut secs = st.timers.draft.unwrap_or(t.duration.as_secs_f64());
            let resp = ui.add(
                egui::DragValue::new(&mut secs)
                    .range(doc::MIN_TIMER_SECS..=doc::MAX_TIMER_SECS)
                    .speed(0.5)
                    .max_decimals(1)
                    .suffix(" s"),
            );
            if resp.changed() {
                st.timers.draft = Some(secs);
            }
            // Saved once the drag ends or the typing is done, not on every frame.
            let done = resp.drag_stopped()
                || resp.lost_focus()
                || (resp.changed() && !resp.dragged() && !resp.has_focus());
            if done
                && let Some(v) = st.timers.draft.take()
                && (v - t.duration.as_secs_f64()).abs() > 1e-6
            {
                st.timers.last_secs = v;
                crate::grid_menu::send(
                    st,
                    actions,
                    &path,
                    &d,
                    set_op(r, timer(v, t.loop_to_beginning)),
                );
            }
            let mut loops = t.loop_to_beginning;
            if ui
                .checkbox(&mut loops, "Loop to the first slide after this")
                .changed()
            {
                crate::grid_menu::send(
                    st,
                    actions,
                    &path,
                    &d,
                    set_op(r, timer(t.duration.as_secs_f64(), loops)),
                );
            }
        });
    });
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(RichText::new(text).small().italics());
    });
}

/// One step of the chain: a group-colour stripe, the slide's name and its time.
fn chip(
    ui: &mut egui::Ui,
    name: &str,
    time: &str,
    group: Color32,
    live: bool,
    open: bool,
    enabled: bool,
) -> egui::Response {
    let font = egui::FontId::proportional(13.0);
    let ink = if enabled {
        Color32::from_gray(230)
    } else {
        Color32::from_gray(120)
    };
    let name_g = ui
        .painter()
        .layout_no_wrap(name.to_owned(), font.clone(), ink);
    let time_c = if live {
        brand::ACCENT
    } else {
        Color32::from_gray(170)
    };
    let time_g = ui.painter().layout_no_wrap(time.to_owned(), font, time_c);
    let size = Vec2::new(
        6.0 + 8.0 + name_g.size().x + 10.0 + time_g.size().x + 8.0,
        26.0,
    );
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = ui.painter();
    let fill = if resp.hovered() {
        Color32::from_gray(58)
    } else {
        Color32::from_gray(44)
    };
    p.rect_filled(rect, 4.0, fill);
    let stripe = egui::Rect::from_min_size(rect.min, Vec2::new(6.0, rect.height()));
    p.rect_filled(
        stripe,
        egui::CornerRadius {
            nw: 4,
            sw: 4,
            ne: 0,
            se: 0,
        },
        group,
    );
    let y = rect.center().y;
    let x = rect.min.x + 14.0;
    p.galley(
        egui::pos2(x, y - name_g.size().y / 2.0),
        name_g.clone(),
        ink,
    );
    p.galley(
        egui::pos2(x + name_g.size().x + 10.0, y - time_g.size().y / 2.0),
        time_g,
        time_c,
    );
    let stroke = if live {
        Stroke::new(2.0, color::LIVE)
    } else if open {
        Stroke::new(2.0, color::ACCENT)
    } else {
        Stroke::new(1.0, Color32::from_gray(20))
    };
    p.rect_stroke(rect, 4.0, stroke, StrokeKind::Inside);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_naturally() {
        let s = |x: f64| format_secs(Duration::from_secs_f64(x));
        assert_eq!(s(10.0), "10s");
        assert_eq!(s(7.5), "7.5s");
        assert_eq!(s(90.0), "1:30");
        assert_eq!(s(600.0), "10:00");
    }
}
