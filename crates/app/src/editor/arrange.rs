//! Position and Layers tabs (docs/EDITOR-PLAN.md E4, Selection + Alignment):
//! arrange, align to page, align the selection, distribute, exact fields; the
//! layer list with reorder, lock, hide and rename.

use std::path::Path;

use doc::{Edit as Op, Element};
use egui::{Color32, RichText, Sense};

use super::snap::Bounds;
use super::text::send_checked;
use super::{aabb, edit, edit_merging};
use crate::ui::{UiAction, UiState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Centre,
    Right,
    Top,
    Middle,
    Bottom,
}

const ALIGNS: [(Align, &str); 6] = [
    (Align::Left, "Left"),
    (Align::Centre, "Centre"),
    (Align::Right, "Right"),
    (Align::Top, "Top"),
    (Align::Middle, "Middle"),
    (Align::Bottom, "Bottom"),
];

/// Placeable, unlocked elements of the selection: (index, frame, rotation).
fn movable(d: &doc::Document, g: usize, s: usize, sel: &[usize]) -> Vec<(usize, doc::Rect, f64)> {
    let elements = &d.groups[g].slides[s].elements;
    sel.iter()
        .filter_map(|&e| {
            let el = elements.get(e)?;
            (!el.locked()).then_some((e, el.frame()?, el.rotation()))
        })
        .collect()
}

fn union(items: &[(usize, doc::Rect, f64)]) -> Option<Bounds> {
    items
        .iter()
        .map(|(_, f, r)| aabb(f, *r))
        .reduce(|a, b| a.union(&b))
}

/// How far `b` moves to line up with `target` by `how`.
fn align_delta(b: &Bounds, target: &Bounds, how: Align) -> (f64, f64) {
    match how {
        Align::Left => (target.x0 - b.x0, 0.0),
        Align::Right => (target.x1 - b.x1, 0.0),
        Align::Centre => ((target.x0 + target.x1 - b.x0 - b.x1) / 2.0, 0.0),
        Align::Top => (0.0, target.y0 - b.y0),
        Align::Bottom => (0.0, target.y1 - b.y1),
        Align::Middle => (0.0, (target.y0 + target.y1 - b.y0 - b.y1) / 2.0),
    }
}

fn moved(f: &doc::Rect, (dx, dy): (f64, f64)) -> doc::Rect {
    doc::Rect {
        x: f.x + dx,
        y: f.y + dy,
        ..*f
    }
}

/// One step for any number of frame changes.
fn frames_op(g: usize, s: usize, label: &str, frames: Vec<(usize, doc::Rect)>) -> Option<Op> {
    let mut edits: Vec<Op> = frames
        .into_iter()
        .map(|(e, frame)| Op::SetFrame {
            group: g,
            slide: s,
            element: e,
            frame,
        })
        .collect();
    match edits.len() {
        0 => None,
        1 => edits.pop(),
        _ => Some(Op::Batch {
            label: label.into(),
            edits,
        }),
    }
}

/// Align the selection to the page: the selection moves as one (its layout kept).
pub fn align_to_page(
    d: &doc::Document,
    (g, s): (usize, usize),
    sel: &[usize],
    how: Align,
) -> Option<Op> {
    let items = movable(d, g, s, sel);
    let b = union(&items)?;
    let page = Bounds {
        x0: 0.0,
        y0: 0.0,
        x1: f64::from(d.canvas.width),
        y1: f64::from(d.canvas.height),
    };
    let delta = align_delta(&b, &page, how);
    frames_op(
        g,
        s,
        "Align",
        items
            .iter()
            .map(|(e, f, _)| (*e, moved(f, delta)))
            .collect(),
    )
}

/// Line each selected element up with the selection's bounds.
pub fn align_selection(
    d: &doc::Document,
    (g, s): (usize, usize),
    sel: &[usize],
    how: Align,
) -> Option<Op> {
    let items = movable(d, g, s, sel);
    if items.len() < 2 {
        return None;
    }
    let all = union(&items)?;
    frames_op(
        g,
        s,
        "Align",
        items
            .iter()
            .map(|(e, f, r)| (*e, moved(f, align_delta(&aabb(f, *r), &all, how))))
            .collect(),
    )
}

/// Equal gaps between three or more elements, from the first to the last
/// (horizontally or vertically); the outer two stay put.
pub fn distribute(
    d: &doc::Document,
    (g, s): (usize, usize),
    sel: &[usize],
    horizontal: bool,
) -> Option<Op> {
    let mut items: Vec<(usize, doc::Rect, Bounds)> = movable(d, g, s, sel)
        .into_iter()
        .map(|(e, f, r)| (e, f, aabb(&f, r)))
        .collect();
    if items.len() < 3 {
        return None;
    }
    let span = |b: &Bounds| {
        if horizontal {
            (b.x0, b.x1)
        } else {
            (b.y0, b.y1)
        }
    };
    items.sort_by(|a, b| span(&a.2).0.total_cmp(&span(&b.2).0));
    let first = span(&items[0].2).0;
    let last = span(&items[items.len() - 1].2).1;
    let total: f64 = items.iter().map(|i| span(&i.2).1 - span(&i.2).0).sum();
    let gap = (last - first - total) / (items.len() - 1) as f64;
    let mut at = first;
    let mut frames = Vec::new();
    for (e, f, b) in &items {
        let (a0, a1) = span(b);
        let delta = at - a0;
        frames.push((
            *e,
            if horizontal {
                moved(f, (delta, 0.0))
            } else {
                moved(f, (0.0, delta))
            },
        ));
        at += a1 - a0 + gap;
    }
    frames_op(g, s, "Distribute", frames)
}

/// Move element `e` in the stacking order: `to` its new index.
fn arrange_op(g: usize, s: usize, from: usize, to: usize) -> Option<Op> {
    (from != to).then_some(Op::MoveElement {
        group: g,
        slide: s,
        from,
        to,
    })
}

pub fn position_tab(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    (g, s): (usize, usize),
) {
    let sel = st.edit.selection();
    let n = d.groups[g].slides[s].elements.len();
    let Some(&first) = sel.first() else {
        ui.label(
            RichText::new(
                "Select an element on the canvas (Shift-click or drag a box to select several).",
            )
            .weak(),
        );
        return;
    };
    let mut ops: Vec<Op> = Vec::new();
    ui.label(RichText::new("Arrange").strong());
    ui.horizontal_wrapped(|ui| {
        let single = sel.len() == 1;
        for (label, tip, to) in [
            (
                "Forward",
                "One step closer to the front",
                (first + 1).min(n.saturating_sub(1)),
            ),
            ("Backward", "One step further back", first.saturating_sub(1)),
            ("To Front", "In front of everything", n.saturating_sub(1)),
            ("To Back", "Behind everything", 0),
        ] {
            if ui
                .add_enabled(single, egui::Button::new(label))
                .on_hover_text(tip)
                .on_disabled_hover_text("Select one element")
                .clicked()
                && let Some(op) = arrange_op(g, s, first, to)
            {
                ops.push(op);
                st.edit.element = Some(to);
            }
        }
    });
    ui.add_space(6.0);
    ui.label(RichText::new("Align to page").strong());
    ui.horizontal_wrapped(|ui| {
        for (how, label) in ALIGNS {
            if ui.button(label).clicked()
                && let Some(op) = align_to_page(d, (g, s), &sel, how)
            {
                ops.push(op);
            }
        }
    });
    if sel.len() >= 2 {
        ui.add_space(6.0);
        ui.label(RichText::new("Align selection").strong());
        ui.horizontal_wrapped(|ui| {
            for (how, label) in ALIGNS {
                if ui.button(label).clicked()
                    && let Some(op) = align_selection(d, (g, s), &sel, how)
                {
                    ops.push(op);
                }
            }
        });
    }
    if sel.len() >= 3 {
        ui.add_space(6.0);
        ui.label(RichText::new("Distribute").strong());
        ui.horizontal(|ui| {
            if ui.button("Horizontally").clicked()
                && let Some(op) = distribute(d, (g, s), &sel, true)
            {
                ops.push(op);
            }
            if ui.button("Vertically").clicked()
                && let Some(op) = distribute(d, (g, s), &sel, false)
            {
                ops.push(op);
            }
        });
    }
    for op in ops {
        edit(actions, path, op);
    }
    if sel.len() != 1 {
        return;
    }
    let Some(el) = d.groups[g].slides[s].elements.get(first) else {
        return;
    };
    let Some(f) = el.frame() else { return };
    ui.add_space(8.0);
    ui.label(RichText::new("Position and size").strong());
    let uuid = el.uuid().to_owned();
    let locked = el.locked();
    let mut nf = f;
    let mut rot = el.rotation().rem_euclid(360.0);
    let mut rot_changed = false;
    egui::Grid::new("position-fields")
        .num_columns(4)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            let field = |ui: &mut egui::Ui, label: &str, v: &mut f64, min: f64| {
                ui.label(label);
                ui.add_enabled(
                    !locked,
                    egui::DragValue::new(v)
                        .range(min..=100_000.0)
                        .speed(1.0)
                        .max_decimals(1),
                )
                .changed()
            };
            field(ui, "X", &mut nf.x, -100_000.0);
            field(ui, "Y", &mut nf.y, -100_000.0);
            ui.end_row();
            field(ui, "W", &mut nf.width, 1.0);
            field(ui, "H", &mut nf.height, 1.0);
            ui.end_row();
            ui.label("Rotation");
            rot_changed = ui
                .add_enabled(
                    !locked,
                    egui::DragValue::new(&mut rot)
                        .range(0.0..=359.0)
                        .speed(1.0)
                        .suffix("°"),
                )
                .changed();
            ui.end_row();
        });
    if nf != f {
        edit_merging(
            actions,
            path,
            Op::SetFrame {
                group: g,
                slide: s,
                element: first,
                frame: nf,
            },
            format!("fields:{uuid}"),
        );
    }
    if rot_changed {
        edit_merging(
            actions,
            path,
            Op::SetRotation {
                group: g,
                slide: s,
                element: first,
                degrees: rot,
            },
            format!("rotation:{uuid}"),
        );
    }
    if locked {
        ui.label(
            RichText::new("Locked: unlock it in Layers to change these.").color(Color32::YELLOW),
        );
    }
}

/// Name for the Layers list: the element's name, plus the start of a text box's text.
fn layer_name(e: &Element) -> String {
    match e {
        Element::Text(t) => {
            let text: String = t.plain_text().chars().take(24).collect();
            let text = text.replace('\n', " ");
            match (t.name.trim(), text.trim()) {
                ("", "") => "Text box".to_owned(),
                ("", tx) => format!("“{tx}”"),
                (n, "") => n.to_owned(),
                (n, tx) => format!("{n} — “{tx}”"),
            }
        }
        Element::Image(i) if i.name.trim().is_empty() => i.media.file_name().to_owned(),
        Element::Image(i) => i.name.clone(),
        Element::Pro6(p) => format!("{} (not editable)", p.kind),
    }
}

pub fn layers_tab(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    (g, s): (usize, usize),
) {
    let elements = &d.groups[g].slides[s].elements;
    if elements.is_empty() {
        ui.label(
            RichText::new("This slide has no elements. Add one with Insert in the top bar.").weak(),
        );
        return;
    }
    ui.label(
        RichText::new("Top of the list is in front. Drag to reorder.")
            .small()
            .weak(),
    );
    ui.add_space(4.0);
    let sel = st.edit.selection();
    let mut rows: Vec<(usize, egui::Rect)> = Vec::new();
    let mut ops: Vec<Op> = Vec::new();
    // Front to back.
    for e in (0..elements.len()).rev() {
        let el = &elements[e];
        let placeable = el.frame().is_some();
        let selected = sel.contains(&e);
        let frame = egui::Frame::new()
            .fill(if selected {
                Color32::from_rgb(40, 70, 120)
            } else {
                Color32::from_gray(38)
            })
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(6, 4));
        let resp = frame
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    // Drag handle: three short bars.
                    let (hr, handle) =
                        ui.allocate_exact_size(egui::vec2(12.0, 14.0), Sense::drag());
                    for k in 0..3 {
                        let y = hr.top() + 3.0 + k as f32 * 4.0;
                        ui.painter()
                            .hline(hr.x_range(), y, egui::Stroke::new(1.5, Color32::GRAY));
                    }
                    let handle = handle.on_hover_text("Drag to reorder");
                    if handle.drag_started() {
                        st.edit.layer_drag = Some(e);
                    }
                    // Right to left: the toggles, then the name in the space left.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if placeable {
                            let hidden = el.hidden();
                            if ui
                                .selectable_label(hidden, if hidden { "Hidden" } else { "Hide" })
                                .on_hover_text("Hidden elements aren't drawn anywhere")
                                .clicked()
                            {
                                ops.push(Op::SetHidden {
                                    group: g,
                                    slide: s,
                                    element: e,
                                    hidden: !hidden,
                                });
                            }
                            let locked = el.locked();
                            if ui
                                .selectable_label(locked, if locked { "Locked" } else { "Lock" })
                                .on_hover_text("Locked elements can't be moved or resized")
                                .clicked()
                            {
                                ops.push(Op::SetLocked {
                                    group: g,
                                    slide: s,
                                    element: e,
                                    locked: !locked,
                                });
                            }
                        }
                        let renaming = st.edit.layer_rename.as_ref().is_some_and(|(i, _)| *i == e);
                        if renaming {
                            let mut done = false;
                            if let Some((_, name)) = st.edit.layer_rename.as_mut() {
                                let r = ui.add(
                                    egui::TextEdit::singleline(name)
                                        .desired_width(ui.available_width()),
                                );
                                r.request_focus();
                                done = r.lost_focus();
                            }
                            if done
                                && let Some((_, name)) = st.edit.layer_rename.take()
                                && ui.input(|i| !i.key_pressed(egui::Key::Escape))
                                && name != el.name()
                            {
                                ops.push(Op::RenameElement {
                                    group: g,
                                    slide: s,
                                    element: e,
                                    name,
                                });
                            }
                        } else {
                            let mut text = RichText::new(layer_name(el));
                            if el.hidden() {
                                text = text.weak().italics();
                            }
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    let r = ui
                                        .add(
                                            egui::Label::new(text).truncate().sense(Sense::click()),
                                        )
                                        .on_hover_text("Click to select, double-click to rename");
                                    if r.clicked() && placeable {
                                        if ui.input(|i| i.modifiers.shift) {
                                            st.edit.toggle(e);
                                        } else {
                                            st.edit.element = Some(e);
                                            st.edit.multi.clear();
                                        }
                                    }
                                    if r.double_clicked() && placeable {
                                        st.edit.layer_rename = Some((e, el.name().to_owned()));
                                    }
                                },
                            );
                        }
                    });
                });
            })
            .response;
        rows.push((e, resp.rect));
    }
    // Drop: the row under the pointer takes the dragged layer's place.
    if let Some(from) = st.edit.layer_drag {
        if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
            if let Some((to, r)) = rows
                .iter()
                .find(|(_, r)| p.y >= r.top() && p.y <= r.bottom())
            {
                ui.painter().hline(
                    r.x_range(),
                    if *to > from { r.top() } else { r.bottom() },
                    egui::Stroke::new(2.0, super::SELECT),
                );
            }
            if ui.input(|i| i.pointer.any_released()) {
                if let Some((to, _)) = rows
                    .iter()
                    .find(|(_, r)| p.y >= r.top() && p.y <= r.bottom())
                    && let Some(op) = arrange_op(g, s, from, *to)
                {
                    ops.push(op);
                    if st.edit.element == Some(from) {
                        st.edit.element = Some(*to);
                    }
                    st.edit.multi.clear();
                }
                st.edit.layer_drag = None;
            }
        }
        if !ui.input(|i| i.pointer.any_down()) {
            st.edit.layer_drag = None;
        }
    }
    for op in ops {
        send_checked(st, actions, path, d, op, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with(frames: &[(f64, f64, f64, f64)]) -> doc::Document {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/render_1024x768.pro6");
        let mut d = doc::load(&path).unwrap_or_else(|e| panic!("{e:?}"));
        let template = d.groups[0].slides[0].elements[0].clone();
        d.groups[0].slides[0].elements = frames
            .iter()
            .map(|&(x, y, w, h)| {
                let mut e = template.clone();
                if let Element::Text(t) = &mut e {
                    t.uuid = doc::new_uuid();
                    t.frame = doc::Rect {
                        x,
                        y,
                        width: w,
                        height: h,
                    };
                    t.rotation = 0.0;
                    t.locked = false;
                }
                e
            })
            .collect();
        d
    }

    fn frames_after(d: &doc::Document, op: Op) -> Vec<doc::Rect> {
        let mut d = d.clone();
        op.apply(&mut d).unwrap_or_else(|e| panic!("{e:?}"));
        d.groups[0].slides[0]
            .elements
            .iter()
            .filter_map(Element::frame)
            .collect()
    }

    #[test]
    fn align_to_page_moves_the_selection_as_one() {
        let d = doc_with(&[(100.0, 100.0, 100.0, 50.0), (300.0, 200.0, 50.0, 50.0)]);
        let op = align_to_page(&d, (0, 0), &[0, 1], Align::Right).unwrap_or_else(|| panic!());
        let f = frames_after(&d, op);
        assert_eq!(f[1].x + f[1].width, 1024.0, "right edge on the page edge");
        assert_eq!(f[1].x - f[0].x, 200.0, "layout kept");
        let op = align_to_page(&d, (0, 0), &[0], Align::Middle).unwrap_or_else(|| panic!());
        assert_eq!(frames_after(&d, op)[0].y, (768.0 - 50.0) / 2.0);
    }

    #[test]
    fn align_selection_lines_up_with_the_bounds() {
        let d = doc_with(&[(100.0, 100.0, 100.0, 50.0), (300.0, 200.0, 50.0, 80.0)]);
        let f = frames_after(
            &d,
            align_selection(&d, (0, 0), &[0, 1], Align::Top).unwrap_or_else(|| panic!()),
        );
        assert_eq!((f[0].y, f[1].y), (100.0, 100.0));
        let f = frames_after(
            &d,
            align_selection(&d, (0, 0), &[0, 1], Align::Right).unwrap_or_else(|| panic!()),
        );
        assert_eq!((f[0].x + f[0].width, f[1].x + f[1].width), (350.0, 350.0));
        assert!(
            align_selection(&d, (0, 0), &[0], Align::Top).is_none(),
            "needs two"
        );
    }

    #[test]
    fn distribute_makes_equal_gaps_keeping_the_ends() {
        let d = doc_with(&[
            (0.0, 0.0, 100.0, 50.0),
            (130.0, 0.0, 100.0, 50.0),
            (500.0, 0.0, 100.0, 50.0),
        ]);
        let f = frames_after(
            &d,
            distribute(&d, (0, 0), &[2, 0, 1], true).unwrap_or_else(|| panic!()),
        );
        assert_eq!(f[0].x, 0.0);
        assert_eq!(f[2].x, 500.0);
        assert_eq!(f[1].x, 250.0, "gaps of 150 each side");
        assert!(
            distribute(&d, (0, 0), &[0, 1], true).is_none(),
            "needs three"
        );
    }
}
