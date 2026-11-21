//! The arrangement editor (docs/PLAN.md, Backlog): a window with the presentation's
//! arrangements on the left and, for the chosen one, its order of groups as coloured
//! chips. Drag groups from the pool into the order (a group can be used any number of
//! times), drag chips to reorder, drag one back to the pool (or right-click) to take
//! it out. Underneath, the slides the order plays.
//!
//! Opened from the editor, changes go to the editor copy (saved with Save, like any
//! edit). Opened from the presentation grid, each change is saved at once under the
//! usual save rules (a `.pro6` stays a `.pro6`), as with the grid's right-click menu.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::Sender;
use doc::{Document, Edit as Op, new_uuid};
use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};
use opcore::{Command, ShowState};

use crate::thumbs::ThumbRequest;
use crate::ui::{UiAction, UiState, color32, text_on};

pub struct ArrangeWindow {
    path: PathBuf,
    /// Opened from the grid: save each change.
    direct: bool,
    /// Arrangement being edited (`None` = the natural order, read-only).
    selected: Option<String>,
    /// Name field, and the arrangement it belongs to.
    name: (Option<String>, String),
    confirm_delete: bool,
}

/// What's being dragged: a chip in the order (its index) or a group from the pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    Order(usize),
    Pool(usize),
}

/// Open for the presentation at `path`, on the arrangement the show uses.
pub fn open(st: &mut UiState, show: &ShowState, path: &Path, direct: bool) {
    st.arrange = Some(ArrangeWindow {
        path: path.to_path_buf(),
        direct,
        selected: show.arrangement_for(path),
        name: (None, String::new()),
        confirm_delete: false,
    });
}

/// `seq` after dropping `drag` before position `at` (an index into `seq` as it is).
fn dropped(seq: &[usize], drag: Drag, at: usize) -> Vec<usize> {
    let mut n = seq.to_vec();
    let at = at.min(n.len());
    match drag {
        Drag::Pool(g) => n.insert(at, g),
        Drag::Order(i) if i < n.len() => {
            let g = n.remove(i);
            n.insert(if at > i { at - 1 } else { at }, g);
        }
        Drag::Order(_) => {}
    }
    n
}

/// Apply an edit (and save it when opened from the grid).
fn send(actions: &mut Vec<UiAction>, w: &ArrangeWindow, op: Op) {
    actions.push(UiAction::Core(Command::Edit {
        presentation: w.path.clone(),
        edit: op,
        accept_native: false,
    }));
    if w.direct {
        actions.push(UiAction::Core(Command::Save(w.path.clone())));
    }
}

/// Where chips were drawn (tests drag them): `("order" | "pool", index, rect)`.
#[cfg(test)]
mod probe {
    use std::cell::RefCell;
    thread_local! {
        pub static CHIPS: RefCell<Vec<(&'static str, usize, egui::Rect)>> = const { RefCell::new(Vec::new()) };
    }
    pub fn record(kind: &'static str, i: usize, r: egui::Rect) {
        CHIPS.with(|c| c.borrow_mut().push((kind, i, r)));
    }
    pub fn take() -> Vec<(&'static str, usize, egui::Rect)> {
        CHIPS.with(|c| std::mem::take(&mut *c.borrow_mut()))
    }
}

/// A group chip; returns its response (clicks and drags).
fn chip(ui: &mut egui::Ui, name: &str, color: Color32, sense: Sense) -> egui::Response {
    let font = FontId::proportional(13.0);
    let label = if name.is_empty() { "(no name)" } else { name };
    let w = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), Color32::WHITE)
        .size()
        .x;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w + 20.0, 26.0), sense);
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect, 4.0, color);
        if resp.hovered() {
            p.rect_stroke(
                rect,
                4.0,
                Stroke::new(1.5, Color32::WHITE),
                StrokeKind::Inside,
            );
        }
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            font,
            text_on(color),
        );
    }
    resp
}

/// A chip that can be clicked, right-clicked and dragged (carrying `payload`); while
/// dragged, a copy follows the pointer.
fn drag_chip(ui: &mut egui::Ui, payload: Drag, name: &str, color: Color32) -> egui::Response {
    let resp = chip(ui, name, color, Sense::click_and_drag());
    if resp.dragged() {
        egui::DragAndDrop::set_payload(ui.ctx(), payload);
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if let Some(p) = ui.ctx().pointer_interact_pos() {
            let layer = egui::LayerId::new(egui::Order::Tooltip, resp.id);
            let r = Rect::from_center_size(p, resp.rect.size());
            let painter = ui.ctx().layer_painter(layer);
            painter.rect_filled(r, 4.0, color.gamma_multiply(0.85));
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                if name.is_empty() { "(no name)" } else { name },
                FontId::proportional(13.0),
                text_on(color),
            );
        }
    } else if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    resp
}

/// Where a drop at the pointer lands for a chip at `rect` with index `i`.
fn insert_at(ui: &egui::Ui, rect: Rect, i: usize) -> usize {
    match ui.ctx().pointer_interact_pos() {
        Some(p) if p.x > rect.center().x => i + 1,
        _ => i,
    }
}

/// The window, over whichever view is showing.
pub fn window(
    ctx: &egui::Context,
    st: &mut UiState,
    show: &ShowState,
    thumbs: &Sender<ThumbRequest>,
    actions: &mut Vec<UiAction>,
) {
    // Taken out while drawing (the body needs `st` for thumbnails too).
    let Some(mut w) = st.arrange.take() else {
        return;
    };
    let Some(d) = show.edited_doc(&w.path).cloned() else {
        return;
    };
    let mut open = true;
    egui::Window::new(format!("Arrangements · {}", crate::ui::title(&w.path)))
        .id(egui::Id::unique("arrangements"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size(Vec2::new(860.0, 560.0))
        .pivot(Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| body(ui, st, &mut w, show, thumbs, actions, &d));
    if open && st.arrange.is_none() {
        st.arrange = Some(w);
    }
}

fn body(
    ui: &mut egui::Ui,
    st: &mut UiState,
    w: &mut ArrangeWindow,
    show: &ShowState,
    thumbs: &Sender<ThumbRequest>,
    actions: &mut Vec<UiAction>,
    d: &Arc<Document>,
) {
    let list = d.arrangements();
    if w.selected
        .as_ref()
        .is_some_and(|u| !list.iter().any(|a| &a.uuid == u))
    {
        w.selected = None;
    }
    // A change from the grid saves the file, which would also save the editor's edits.
    let locked = w.direct && show.unsaved.contains(&w.path);
    let in_show = show.arrangement_for(&w.path);
    egui::Panel::left("arr-list")
        .resizable(false)
        .exact_size(210.0)
        .show(ui, |ui| {
            ui.label(RichText::new("Arrangements").strong());
            ui.add_space(4.0);
            let mut row = |ui: &mut egui::Ui, id: Option<String>, name: &str| {
                let used = in_show == id;
                let text = if used {
                    RichText::new(format!("{name}  (in the show)"))
                        .color(crate::theme::color::LIVE_TEXT)
                } else {
                    RichText::new(name)
                };
                let r = ui
                    .selectable_label(w.selected == id, text)
                    .on_hover_text(if used { "Used in the show" } else { "" });
                if r.clicked() {
                    w.selected = id;
                    w.confirm_delete = false;
                }
            };
            row(ui, None, "Natural order");
            for a in &list {
                row(ui, Some(a.uuid.clone()), &a.name);
            }
            ui.add_space(8.0);
            ui.add_enabled_ui(!locked, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button("New")
                        .on_hover_text("A new arrangement with every group once, in order")
                        .clicked()
                    {
                        let uuid = new_uuid();
                        let name = unique_name(&list, "New arrangement");
                        send(
                            actions,
                            w,
                            Op::AddArrangement {
                                uuid: uuid.clone(),
                                name,
                                groups: (0..d.groups.len()).collect(),
                            },
                        );
                        w.selected = Some(uuid);
                    }
                    let current = w
                        .selected
                        .as_ref()
                        .and_then(|u| list.iter().find(|a| &a.uuid == u));
                    if ui
                        .add_enabled(current.is_some(), egui::Button::new("Duplicate"))
                        .clicked()
                        && let Some(a) = current
                    {
                        let uuid = new_uuid();
                        send(
                            actions,
                            w,
                            Op::AddArrangement {
                                uuid: uuid.clone(),
                                name: unique_name(&list, &format!("{} copy", a.name)),
                                groups: a.groups.clone(),
                            },
                        );
                        w.selected = Some(uuid);
                    }
                    if ui
                        .add_enabled(current.is_some(), egui::Button::new("Delete"))
                        .clicked()
                    {
                        w.confirm_delete = true;
                    }
                });
                if w.confirm_delete
                    && let Some(uuid) = w.selected.clone()
                {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Delete it?").color(Color32::from_rgb(255, 150, 150)),
                        );
                        if ui.button("Delete").clicked() {
                            send(actions, w, Op::RemoveArrangement { uuid });
                            w.selected = None;
                            w.confirm_delete = false;
                        }
                        if ui.button("Keep").clicked() {
                            w.confirm_delete = false;
                        }
                    });
                }
            });
        });
    egui::CentralPanel::default().show(ui, |ui| {
        if locked {
            ui.label(
                RichText::new("The editor has unsaved changes to this presentation: save or revert them there first.")
                    .color(Color32::from_rgb(255, 190, 120)),
            );
        }
        let current = w
            .selected
            .as_ref()
            .and_then(|u| list.iter().find(|a| &a.uuid == u))
            .cloned();
        // Name, and whether the show uses it.
        ui.horizontal(|ui| {
            match &current {
                Some(a) => {
                    if w.name.0.as_ref() != Some(&a.uuid) {
                        w.name = (Some(a.uuid.clone()), a.name.clone());
                    }
                    ui.label("Name");
                    let r = ui.add_enabled(
                        !locked,
                        egui::TextEdit::singleline(&mut w.name.1).desired_width(220.0),
                    );
                    let name = w.name.1.trim().to_owned();
                    if r.lost_focus() && !name.is_empty() && name != a.name {
                        send(
                            actions,
                            w,
                            Op::RenameArrangement {
                                uuid: a.uuid.clone(),
                                name,
                            },
                        );
                    }
                }
                None => {
                    ui.label(RichText::new("Natural order").strong());
                    ui.label(
                        RichText::new("every group once, as they come. Make a New arrangement to change the order.")
                            .weak(),
                    );
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if in_show == w.selected {
                    ui.label(RichText::new("Used in the show").color(crate::theme::color::LIVE_TEXT));
                } else if ui
                    .button("Use in the show")
                    .on_hover_text("Play the presentation in this order")
                    .clicked()
                {
                    actions.push(UiAction::Core(Command::SelectArrangement {
                        presentation: w.path.clone(),
                        arrangement: w.selected.clone(),
                    }));
                }
            });
        });
        ui.separator();
        let editable = current.is_some() && !locked;
        let seq: Vec<usize> = match &current {
            Some(a) => a.groups.clone(),
            None => (0..d.groups.len()).collect(),
        };
        let mut new_seq: Option<Vec<usize>> = None;
        ui.label(RichText::new("Order").strong());
        if editable {
            ui.label(
                RichText::new("Drag to reorder. Drag a chip back to the groups below, or right-click it, to take it out.")
                    .small()
                    .weak(),
            );
        }
        // The order: chips, each a drag source and a drop target.
        let frame = egui::Frame::new()
            .fill(crate::theme::brand::BG_BOTTOM)
            .corner_radius(4.0)
            .inner_margin(8.0);
        frame.show(ui, |ui| {
            ui.set_min_height(44.0);
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                for (i, &g) in seq.iter().enumerate() {
                    let Some(group) = d.groups.get(g) else { continue };
                    let c = color32(group.color);
                    let resp = if editable {
                        drag_chip(ui, Drag::Order(i), &group.name, c)
                    } else {
                        chip(ui, &group.name, c, Sense::hover())
                    };
                    #[cfg(test)]
                    probe::record("order", i, resp.rect);
                    if !editable {
                        continue;
                    }
                    if let Some(drag) = resp.dnd_hover_payload::<Drag>() {
                        let at = insert_at(ui, resp.rect, i);
                        let x = if at == i { resp.rect.left() - 3.0 } else { resp.rect.right() + 3.0 };
                        ui.painter().vline(
                            x,
                            resp.rect.y_range(),
                            Stroke::new(3.0, crate::theme::brand::ACCENT),
                        );
                        if let Some(d2) = resp.dnd_release_payload::<Drag>() {
                            new_seq = Some(dropped(&seq, *d2, at));
                        }
                        let _ = drag;
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Repeat it after this").clicked() {
                            new_seq = Some(dropped(&seq, Drag::Pool(g), i + 1));
                            ui.close();
                        }
                        if ui.button("Take it out").clicked() {
                            let mut n = seq.clone();
                            n.remove(i);
                            new_seq = Some(n);
                            ui.close();
                        }
                    });
                }
                if editable {
                    // Room at the end to drop onto.
                    let (r, resp) = ui.allocate_exact_size(Vec2::new(60.0, 26.0), Sense::hover());
                    #[cfg(test)]
                    probe::record("end", 0, r);
                    if resp.dnd_hover_payload::<Drag>().is_some() {
                        ui.painter().vline(
                            r.left() + 3.0,
                            r.y_range(),
                            Stroke::new(3.0, crate::theme::brand::ACCENT),
                        );
                    }
                    if let Some(d2) = resp.dnd_release_payload::<Drag>() {
                        new_seq = Some(dropped(&seq, *d2, seq.len()));
                    }
                }
            });
        });
        if let Some(a) = &current
            && !a.unknown_group_ids.is_empty()
        {
            ui.label(
                RichText::new(format!(
                    "{} entry(ies) point at groups that no longer exist; the next change here drops them.",
                    a.unknown_group_ids.len()
                ))
                .small()
                .color(Color32::YELLOW),
            );
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Groups").strong());
        if editable {
            ui.label(
                RichText::new("Click a group to add it at the end, or drag it into the order. A group can be used as many times as it's sung.")
                    .small()
                    .weak(),
            );
        }
        let (pool, dropped_back) = ui.dnd_drop_zone::<Drag, _>(
            egui::Frame::new().corner_radius(4.0).inner_margin(8.0),
            |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                    for (gi, g) in d.groups.iter().enumerate() {
                        let c = color32(g.color);
                        let n = g.slides.len();
                        let resp = if editable {
                            drag_chip(ui, Drag::Pool(gi), &g.name, c)
                        } else {
                            chip(ui, &g.name, c, Sense::hover())
                        };
                        #[cfg(test)]
                        probe::record("pool", gi, resp.rect);
                        let resp = resp.on_hover_text(format!("{n} slide(s)"));
                        if editable && resp.clicked() {
                            new_seq = Some(dropped(&seq, Drag::Pool(gi), seq.len()));
                        }
                    }
                });
            },
        );
        let _ = pool;
        if let Some(drag) = dropped_back
            && let Drag::Order(i) = *drag
            && editable
        {
            let mut n = seq.clone();
            if i < n.len() {
                n.remove(i);
            }
            new_seq = Some(n);
        }
        if let (Some(n), Some(a)) = (new_seq, &current)
            && n != seq
        {
            send(
                actions,
                w,
                Op::SetArrangementGroups {
                    uuid: a.uuid.clone(),
                    groups: n,
                },
            );
        }
        ui.add_space(6.0);
        ui.separator();
        // What it plays.
        let order = opcore::slide_order(d, w.selected.as_deref());
        let direct = w.direct;
        ui.label(RichText::new(format!("Plays {} slide(s)", order.len())).strong());
        egui::ScrollArea::vertical()
            .id_salt("arr-preview")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                    let tw = 120.0;
                    let th = (tw * d.canvas.height as f32 / d.canvas.width.max(1) as f32).round();
                    for (i, r) in order.iter().enumerate() {
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(tw, th + 14.0), Sense::hover());
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let img = Rect::from_min_size(rect.min, Vec2::new(tw, th));
                        let (_, tex) = st.thumb_any(thumbs, d, r.flat, direct);
                        match tex {
                            Some(Ok(t)) => {
                                egui::Image::new(t).paint_at(ui, img);
                            }
                            _ => {
                                ui.painter().rect_filled(img, 0.0, Color32::from_gray(18));
                            }
                        }
                        let g = &d.groups[r.group];
                        let c = color32(g.color);
                        let bar = Rect::from_min_max(egui::pos2(rect.min.x, img.max.y), rect.max);
                        ui.painter().rect_filled(bar, 0.0, c);
                        ui.painter().with_clip_rect(bar).text(
                            bar.left_center() + Vec2::new(4.0, 0.0),
                            Align2::LEFT_CENTER,
                            format!("{}. {}", i + 1, g.name),
                            FontId::proportional(10.0),
                            text_on(c),
                        );
                        if !r.enabled {
                            ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha(150));
                        }
                    }
                });
            });
    });
}

/// `base`, or `base 2`, `base 3`… if an arrangement already has that name.
fn unique_name(list: &[doc::ResolvedArrangement], base: &str) -> String {
    let taken = |n: &str| list.iter().any(|a| a.name.eq_ignore_ascii_case(n));
    if !taken(base) {
        return base.to_owned();
    }
    (2..)
        .map(|i| format!("{base} {i}"))
        .find(|n| !taken(n))
        .unwrap_or_else(|| base.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_move_insert_and_repeat() {
        let seq = [0, 1, 2];
        assert_eq!(dropped(&seq, Drag::Order(0), 3), [1, 2, 0], "to the end");
        assert_eq!(dropped(&seq, Drag::Order(2), 0), [2, 0, 1], "to the start");
        assert_eq!(dropped(&seq, Drag::Order(0), 1), [0, 1, 2], "onto itself");
        assert_eq!(dropped(&seq, Drag::Order(0), 2), [1, 0, 2]);
        assert_eq!(dropped(&seq, Drag::Pool(1), 3), [0, 1, 2, 1], "a repeat");
        assert_eq!(dropped(&seq, Drag::Pool(2), 1), [0, 2, 1, 2]);
        assert_eq!(dropped(&[], Drag::Pool(0), 5), [0]);
    }

    use egui::{Event, PointerButton, Pos2, RawInput};

    struct Harness {
        ctx: egui::Context,
        st: UiState,
        show: ShowState,
        thumbs: Sender<ThumbRequest>,
        time: f64,
        path: PathBuf,
        uuid: String,
    }

    impl Harness {
        /// The fixture song with an arrangement playing groups 0, 1, 2 (or as many as it has).
        fn new() -> Harness {
            let path =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/song_1024x768.pro6");
            let mut d = doc::load(&path).unwrap();
            let uuid = new_uuid();
            Op::AddArrangement {
                uuid: uuid.clone(),
                name: "Test".into(),
                groups: (0..d.groups.len().min(3)).collect(),
            }
            .apply(&mut d)
            .unwrap();
            let mut docs = std::collections::HashMap::new();
            docs.insert(path.clone(), Arc::new(d));
            let show = ShowState {
                docs: Arc::new(docs),
                selected: Some(path.clone()),
                ..Default::default()
            };
            let mut st = UiState::default();
            open(&mut st, &show, &path, true);
            if let Some(w) = st.arrange.as_mut() {
                w.selected = Some(uuid.clone());
            }
            let (thumbs, _rx) = crossbeam_channel::unbounded();
            Harness {
                ctx: egui::Context::default(),
                st,
                show,
                thumbs,
                time: 0.0,
                path,
                uuid,
            }
        }

        fn seq(&self) -> Vec<usize> {
            let d = &self.show.docs[&self.path];
            d.arrangements()
                .into_iter()
                .find(|a| a.uuid == self.uuid)
                .unwrap()
                .groups
        }

        fn frame(
            &mut self,
            events: Vec<Event>,
        ) -> (Vec<UiAction>, Vec<(&'static str, usize, Rect)>) {
            self.time += 0.05;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let _ = probe::take();
            let mut actions = Vec::new();
            let (st, show, thumbs) = (&mut self.st, &self.show, &self.thumbs);
            let mut out = self.ctx.run_ui(input, |ui| {
                window(ui.ctx(), st, show, thumbs, &mut actions);
            });
            out.textures_delta.clear();
            (actions, probe::take())
        }

        fn chips(&mut self) -> Vec<(&'static str, usize, Rect)> {
            self.frame(vec![]);
            self.frame(vec![]).1
        }

        /// Press at `from`, move to `to` in steps, release; the actions on the way.
        fn drag(&mut self, from: Pos2, to: Pos2) -> Vec<UiAction> {
            let mut out = Vec::new();
            out.extend(self.frame(vec![Event::PointerMoved(from)]).0);
            out.extend(self.frame(vec![press(from, true)]).0);
            for k in 1..=8 {
                let p = from + (to - from) * (k as f32 / 8.0);
                out.extend(self.frame(vec![Event::PointerMoved(p)]).0);
            }
            out.extend(self.frame(vec![press(to, false)]).0);
            out.extend(self.frame(vec![]).0);
            out
        }
    }

    fn press(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    fn set_groups(actions: &[UiAction]) -> Vec<Vec<usize>> {
        actions
            .iter()
            .filter_map(|a| match a {
                UiAction::Core(Command::Edit {
                    edit: Op::SetArrangementGroups { groups, .. },
                    ..
                }) => Some(groups.clone()),
                _ => None,
            })
            .collect()
    }

    fn saves(actions: &[UiAction]) -> usize {
        actions
            .iter()
            .filter(|a| matches!(a, UiAction::Core(Command::Save(_))))
            .count()
    }

    fn find(chips: &[(&'static str, usize, Rect)], kind: &str, i: usize) -> Rect {
        chips
            .iter()
            .find(|c| c.0 == kind && c.1 == i)
            .map(|c| c.2)
            .unwrap_or_else(|| panic!("no {kind} chip {i} in {chips:?}"))
    }

    #[test]
    fn dragging_chips_reorders_repeats_and_removes() {
        let mut h = Harness::new();
        let seq = h.seq();
        assert!(seq.len() >= 2, "the fixture has at least two groups");
        let chips = h.chips();
        // Drag the first chip to the end.
        let a = h.drag(
            find(&chips, "order", 0).center(),
            find(&chips, "end", 0).center(),
        );
        let mut want = seq.clone();
        let g = want.remove(0);
        want.push(g);
        assert_eq!(set_groups(&a), [want], "moved to the end");
        assert_eq!(saves(&a), 1, "opened from the grid: saved");
        // Drag a pool group onto the first chip's left half: inserted before it.
        let first = find(&chips, "order", 0);
        let a = h.drag(
            find(&chips, "pool", 1).center(),
            first.left_center() + Vec2::new(2.0, 0.0),
        );
        let mut want = seq.clone();
        want.insert(0, 1);
        assert_eq!(set_groups(&a), [want], "a repeat, before the first");
        // Drag the second chip back to the pool: taken out.
        let a = h.drag(
            find(&chips, "order", 1).center(),
            find(&chips, "pool", 0).center(),
        );
        let mut want = seq.clone();
        want.remove(1);
        assert_eq!(set_groups(&a), [want], "taken out");
        // A click on a pool chip adds it at the end.
        let p = find(&chips, "pool", 0).center();
        let mut a = h.frame(vec![Event::PointerMoved(p)]).0;
        a.extend(h.frame(vec![press(p, true)]).0);
        a.extend(h.frame(vec![press(p, false)]).0);
        let mut want = seq.clone();
        want.push(0);
        assert_eq!(set_groups(&a), [want], "clicked: added at the end");
    }

    #[test]
    fn names_dont_clash() {
        let a = |n: &str| doc::ResolvedArrangement {
            uuid: n.into(),
            name: n.into(),
            color: None,
            groups: Vec::new(),
            unknown_group_ids: Vec::new(),
        };
        let list = vec![a("Song"), a("Song copy"), a("Song copy 2")];
        assert_eq!(unique_name(&list, "Short"), "Short");
        assert_eq!(unique_name(&list, "Song copy"), "Song copy 3");
    }
}
