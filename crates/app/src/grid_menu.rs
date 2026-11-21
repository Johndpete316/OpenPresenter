//! Right-click menu on a slide in the presentation grid, and the quick edit window
//! (docs/PLAN.md, Backlog). Changes go through the normal edit path (one undo step in
//! the presentation's history) and are saved straight away under the usual save
//! rules, so a `.pro6` stays a `.pro6`. A change a `.pro6` can't hold is refused here
//! with a pointer to the editor, which can ask about saving as `.opres`.

use std::path::{Path, PathBuf};

use doc::{Document, Edit as Op};
use egui::{Color32, RichText, Sense, Vec2};
use opcore::{Command, ShowState, SlideRef};

use crate::ui::{UiAction, UiState, color32};

/// The quick edit window: just the words of one slide.
pub struct QuickEdit {
    path: PathBuf,
    group: usize,
    slide: usize,
    /// The slide's UUID, to notice it moved or went away.
    uuid: String,
    title: String,
    boxes: Vec<QuickBox>,
    notes: String,
    notes_orig: String,
    focus: bool,
}

struct QuickBox {
    element: usize,
    name: String,
    orig: String,
    text: String,
}

/// Colours offered for groups besides the group colours in the settings.
const EXTRA_COLORS: &[[u8; 3]] = &[
    [230, 200, 40],
    [60, 170, 70],
    [230, 90, 160],
    [240, 240, 240],
    [70, 70, 70],
];

fn rgba(c: [u8; 3]) -> doc::Rgba {
    let f = |v: u8| f64::from(v) / 255.0;
    doc::Rgba {
        r: f(c[0]),
        g: f(c[1]),
        b: f(c[2]),
        a: 1.0,
    }
}

/// Apply and save, or tell the operator why not.
fn send(st: &mut UiState, actions: &mut Vec<UiAction>, path: &Path, d: &Document, op: Op) {
    match doc::check(d, &op) {
        doc::Check::Ok => {
            actions.push(UiAction::Core(Command::Edit {
                presentation: path.to_path_buf(),
                edit: op,
                accept_native: false,
            }));
            actions.push(UiAction::Core(Command::Save(path.to_path_buf())));
        }
        doc::Check::NeedsNative { features, .. } => {
            let names: Vec<&str> = features.iter().map(|f| f.name()).collect();
            st.toast(format!(
                "{} can't be saved in this ProPresenter 6 file, so nothing was changed. Make the change in the editor, which can save it as .opres.",
                names.join(", ")
            ));
        }
    }
}

/// A small coloured square, as a button.
fn swatch(ui: &mut egui::Ui, c: Color32, selected: bool, hover: &str) -> bool {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
    ui.painter().rect_filled(r, 3.0, c);
    let stroke = if selected || resp.hovered() {
        egui::Stroke::new(2.0, Color32::WHITE)
    } else {
        egui::Stroke::new(1.0, Color32::from_gray(30))
    };
    ui.painter()
        .rect_stroke(r, 3.0, stroke, egui::StrokeKind::Outside);
    resp.on_hover_text(hover).clicked()
}

/// The menu for slide `r` (at `pos` in the play order) of the presentation at `path`.
#[allow(clippy::too_many_arguments)]
pub fn slide_menu(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    path: &Path,
    d: &Document,
    r: SlideRef,
    pos: usize,
    actions: &mut Vec<UiAction>,
) {
    let Some(g) = d.groups.get(r.group) else {
        return;
    };
    ui.set_min_width(230.0);
    ui.label(
        RichText::new(format!(
            "Slide {}{}",
            pos + 1,
            if g.name.is_empty() {
                String::new()
            } else {
                format!(" · {}", g.name)
            }
        ))
        .small()
        .weak(),
    );
    if ui.button("Edit in editor").clicked() {
        st.edit.open_at(r.group, r.slide);
        ui.close();
    }
    if d.theme.is_some() {
        return;
    }
    // Changes here save the file, which would also save the editor's unsaved edits.
    if show.unsaved.contains(path) {
        ui.label(
            RichText::new("Unsaved changes in the editor: save or revert them there first.")
                .small()
                .color(Color32::from_rgb(255, 190, 120)),
        );
        return;
    }
    if ui
        .button("Quick edit text…")
        .on_hover_text("Fix the words (spelling, a changed line) without the editor")
        .clicked()
    {
        open_quick(st, path, d, r);
        ui.close();
    }
    let mut enabled = r.enabled;
    if ui
        .checkbox(&mut enabled, "Enabled")
        .on_hover_text("A disabled slide is skipped by Next (it can still be clicked)")
        .changed()
    {
        send(
            st,
            actions,
            path,
            d,
            Op::SetSlideEnabled {
                group: r.group,
                slide: r.slide,
                enabled,
            },
        );
        ui.close();
    }
    ui.separator();
    let names: Vec<(String, [u8; 3])> = st
        .edit
        .group_colors
        .iter()
        .map(|c| (c.name.clone(), c.color))
        .collect();
    ui.menu_button(
        format!(
            "Group{}",
            if g.name.is_empty() {
                String::new()
            } else {
                format!(": {}", g.name)
            }
        ),
        |ui| group_menu(ui, st, actions, path, d, r.group, &names),
    );
    ui.menu_button("Move slide to", |ui| {
        ui.set_min_width(200.0);
        for (i, og) in d.groups.iter().enumerate() {
            if i == r.group {
                continue;
            }
            let label = if og.name.is_empty() {
                "(no name)"
            } else {
                og.name.as_str()
            };
            let resp = ui.horizontal(|ui| {
                let (sw, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                ui.painter().rect_filled(sw, 2.0, color32(og.color));
                ui.button(label)
            });
            if resp
                .inner
                .on_hover_text("Added at the end of that group")
                .clicked()
            {
                send(
                    st,
                    actions,
                    path,
                    d,
                    Op::MoveSlide {
                        group: r.group,
                        slide: r.slide,
                        to_group: i,
                        to: og.slides.len(),
                    },
                );
                ui.close();
            }
        }
        ui.separator();
        let first = r.slide == 0;
        ui.add_enabled_ui(!first, |ui| {
            ui.menu_button("A new group from this slide", |ui| {
                ui.label(
                    RichText::new("This slide and the ones after it in its group")
                        .small()
                        .weak(),
                );
                for (name, c) in &names {
                    if ui.button(name).clicked() {
                        send(
                            st,
                            actions,
                            path,
                            d,
                            split_group(d, r, name, Some(rgba(*c))),
                        );
                        ui.close();
                    }
                }
            })
            .response
            .on_disabled_hover_text(
                "This is the group's first slide: use Group to rename or recolour the whole group",
            );
        });
    });
}

/// Rename, retype (name + colour from the settings) and recolour group `gi`.
fn group_menu(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &Document,
    gi: usize,
    names: &[(String, [u8; 3])],
) {
    let g = &d.groups[gi];
    ui.set_min_width(220.0);
    ui.label(RichText::new("Call it (name and colour)").small().weak());
    ui.horizontal_wrapped(|ui| {
        for (name, c) in names {
            let b = egui::Button::new(
                RichText::new(name).color(crate::ui::text_on(Color32::from_rgb(c[0], c[1], c[2]))),
            )
            .fill(Color32::from_rgb(c[0], c[1], c[2]));
            if ui.add(b).clicked() {
                let op = Op::Batch {
                    label: format!("Group {name}"),
                    edits: vec![
                        Op::RenameGroup {
                            group: gi,
                            name: name.clone(),
                        },
                        Op::RecolorGroup {
                            group: gi,
                            color: Some(rgba(*c)),
                        },
                    ],
                };
                send(st, actions, path, d, op);
                ui.close();
            }
        }
    });
    ui.separator();
    ui.label(RichText::new("Name").small().weak());
    ui.horizontal(|ui| {
        let key = (path.to_path_buf(), g.uuid.clone());
        if st.grid_rename.as_ref().map(|(k, _)| k) != Some(&key) {
            st.grid_rename = Some((key, g.name.clone()));
        }
        let Some((_, text)) = st.grid_rename.as_mut() else {
            return;
        };
        let r = ui.add(egui::TextEdit::singleline(text).desired_width(150.0));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let name = text.trim().to_owned();
        if (ui.button("Rename").clicked() || enter) && !name.is_empty() && name != g.name {
            send(st, actions, path, d, Op::RenameGroup { group: gi, name });
            st.grid_rename = None;
            ui.close();
        }
    });
    ui.separator();
    ui.label(RichText::new("Colour").small().weak());
    ui.horizontal_wrapped(|ui| {
        let current = g.color.map(doc::Rgba::to_rgba8);
        let mut pick: Option<Option<doc::Rgba>> = None;
        for (name, c) in names {
            if swatch(
                ui,
                Color32::from_rgb(c[0], c[1], c[2]),
                current.is_some_and(|k| k[..3] == c[..]),
                name,
            ) {
                pick = Some(Some(rgba(*c)));
            }
        }
        for c in EXTRA_COLORS {
            if swatch(
                ui,
                Color32::from_rgb(c[0], c[1], c[2]),
                current.is_some_and(|k| k[..3] == c[..]),
                "",
            ) {
                pick = Some(Some(rgba(*c)));
            }
        }
        if ui.small_button("None").clicked() {
            pick = Some(None);
        }
        if let Some(color) = pick {
            send(st, actions, path, d, Op::RecolorGroup { group: gi, color });
            ui.close();
        }
    });
}

/// Slide `r` and the ones after it in its group move to a new group right after it.
/// Arrangements play the new group wherever they played the old one, so the show's
/// order doesn't change.
fn split_group(d: &Document, r: SlideRef, name: &str, color: Option<doc::Rgba>) -> Op {
    let at = r.group + 1;
    let mut edits = vec![Op::AddGroup {
        at,
        name: name.to_owned(),
        color,
    }];
    let n = d.groups[r.group].slides.len();
    for k in 0..n.saturating_sub(r.slide) {
        edits.push(Op::MoveSlide {
            group: r.group,
            slide: r.slide,
            to_group: at,
            to: k,
        });
    }
    for a in d.arrangements() {
        let mut groups = Vec::with_capacity(a.groups.len() + 1);
        for &g in &a.groups {
            groups.push(if g >= at { g + 1 } else { g });
            if g == r.group {
                groups.push(at);
            }
        }
        edits.push(Op::SetArrangementGroups {
            uuid: a.uuid.clone(),
            groups,
        });
    }
    Op::Batch {
        label: format!("New group {name}"),
        edits,
    }
}

fn open_quick(st: &mut UiState, path: &Path, d: &Document, r: SlideRef) {
    let Some(slide) = d.groups.get(r.group).and_then(|g| g.slides.get(r.slide)) else {
        return;
    };
    let boxes: Vec<QuickBox> = slide
        .elements
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            doc::Element::Text(t) if t.problem.is_none() && t.runs().next().is_some() => {
                let text = t.plain_text();
                Some(QuickBox {
                    element: i,
                    name: t.name.clone(),
                    orig: text.clone(),
                    text,
                })
            }
            _ => None,
        })
        .collect();
    let g = &d.groups[r.group];
    st.quick = Some(QuickEdit {
        path: path.to_path_buf(),
        group: r.group,
        slide: r.slide,
        uuid: slide.uuid.clone(),
        title: format!(
            "{} · {} {}",
            crate::ui::title(path),
            if g.name.is_empty() { "Slide" } else { &g.name },
            r.slide + 1
        ),
        boxes,
        notes: slide.notes.clone(),
        notes_orig: slide.notes.clone(),
        focus: true,
    });
}

/// The quick edit window (over the operator view).
pub fn quick_edit_window(
    ctx: &egui::Context,
    st: &mut UiState,
    show: &ShowState,
    actions: &mut Vec<UiAction>,
) {
    let Some(q) = &st.quick else {
        return;
    };
    let Some(d) = show.edited_doc(&q.path).cloned() else {
        st.quick = None;
        return;
    };
    let same = d
        .groups
        .get(q.group)
        .and_then(|g| g.slides.get(q.slide))
        .is_some_and(|s| s.uuid == q.uuid);
    if !same {
        st.quick = None;
        st.toast("The slide being edited moved or was removed, so the quick edit was closed. Nothing was changed.");
        return;
    }
    let mut open = true;
    let mut save = false;
    let mut cancel = false;
    egui::Window::new("Quick edit")
        .id(egui::Id::unique("quick-edit"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(480.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            let Some(q) = st.quick.as_mut() else { return };
            ui.label(RichText::new(&q.title).strong());
            if q.boxes.is_empty() {
                ui.label(RichText::new("This slide has no text to edit.").weak());
            }
            let several = q.boxes.len() > 1;
            let font = egui::FontId::proportional(16.0);
            for (i, b) in q.boxes.iter_mut().enumerate() {
                if several {
                    ui.label(RichText::new(if b.name.is_empty() { "Text" } else { &b.name }).small().weak());
                }
                let rows = b.text.lines().count().clamp(3, 12);
                let r = ui.add(
                    egui::TextEdit::multiline(&mut b.text)
                        .font(font.clone())
                        .desired_rows(rows)
                        .desired_width(f32::INFINITY),
                );
                if i == 0 && q.focus {
                    r.request_focus();
                    q.focus = false;
                }
            }
            egui::CollapsingHeader::new("Notes")
                .id_salt("quick-notes")
                .default_open(!q.notes.is_empty())
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut q.notes)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                });
            ui.label(
                RichText::new("Only the words change: fonts, colours and layout stay as they are. Ctrl+Enter saves, Esc cancels.")
                    .small()
                    .weak(),
            );
            let changed = q.notes != q.notes_orig || q.boxes.iter().any(|b| b.text != b.orig);
            ui.horizontal(|ui| {
                let key = ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
                if ui
                    .add_enabled(changed, egui::Button::new(RichText::new("Save").strong()))
                    .clicked()
                    || (key && changed)
                {
                    save = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            });
        });
    if save && let Some(q) = st.quick.take() {
        let path = q.path.clone();
        if let Some(op) = quick_op(&d, &q) {
            send(st, actions, &path, &d, op);
        }
    } else if cancel || !open {
        st.quick = None;
    }
}

/// The edits for a quick edit: each changed text box keeps its styles where the words
/// didn't change; one undo step.
fn quick_op(d: &Document, q: &QuickEdit) -> Option<Op> {
    let slide = d.groups.get(q.group)?.slides.get(q.slide)?;
    let mut edits = Vec::new();
    for b in &q.boxes {
        if b.text == b.orig {
            continue;
        }
        let Some(doc::Element::Text(t)) = slide.elements.get(b.element) else {
            continue;
        };
        let rich = Op::SetRichText {
            group: q.group,
            slide: q.slide,
            element: b.element,
            paragraphs: doc::rich::replace_plain(&t.paragraphs, &t.plain_text(), &b.text),
        };
        // If the restyled box can't go in a .pro6 (unusual spacing, say), plain text
        // in the box's own style can.
        let op = if matches!(doc::check(d, &rich), doc::Check::Ok) {
            rich
        } else {
            Op::SetBoxText {
                group: q.group,
                slide: q.slide,
                element: b.element,
                text: b.text.clone(),
            }
        };
        edits.push(op);
    }
    if q.notes != q.notes_orig {
        edits.push(Op::SetSlideNotes {
            group: q.group,
            slide: q.slide,
            notes: q.notes.clone(),
        });
    }
    match edits.len() {
        0 => None,
        1 => edits.pop(),
        _ => Some(Op::Batch {
            label: "Quick edit".into(),
            edits,
        }),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn song() -> Document {
        let src =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/song_1024x768.pro6");
        doc::load(&src).unwrap()
    }

    #[test]
    fn a_new_group_from_a_slide_keeps_the_play_order() {
        let mut d = song();
        // Find a group with at least two slides.
        let gi = d.groups.iter().position(|g| g.slides.len() >= 2).unwrap();
        let before: Vec<String> = opcore::slide_order(&d, None)
            .iter()
            .map(|r| d.groups[r.group].slides[r.slide].uuid.clone())
            .collect();
        let arr_before: Vec<Vec<String>> = d
            .arrangements()
            .iter()
            .map(|a| {
                opcore::slide_order(&d, Some(&a.uuid))
                    .iter()
                    .map(|r| d.groups[r.group].slides[r.slide].uuid.clone())
                    .collect()
            })
            .collect();
        let r = opcore::slide_order(&d, None)
            .into_iter()
            .find(|r| r.group == gi && r.slide == 1)
            .unwrap();
        let n = d.groups[gi].slides.len();
        split_group(&d, r, "Chorus", None).apply(&mut d).unwrap();
        assert_eq!(d.groups[gi].slides.len(), 1);
        assert_eq!(d.groups[gi + 1].name, "Chorus");
        assert_eq!(d.groups[gi + 1].slides.len(), n - 1);
        let after: Vec<String> = opcore::slide_order(&d, None)
            .iter()
            .map(|r| d.groups[r.group].slides[r.slide].uuid.clone())
            .collect();
        assert_eq!(before, after);
        let arr_after: Vec<Vec<String>> = d
            .arrangements()
            .iter()
            .map(|a| {
                opcore::slide_order(&d, Some(&a.uuid))
                    .iter()
                    .map(|r| d.groups[r.group].slides[r.slide].uuid.clone())
                    .collect()
            })
            .collect();
        assert_eq!(arr_before, arr_after, "arrangements play the same slides");
    }

    #[test]
    fn quick_edit_changes_only_the_words_and_stays_pro6() {
        let d = song();
        let r = opcore::slide_order(&d, None)
            .into_iter()
            .find(|r| {
                d.groups[r.group].slides[r.slide]
                    .text_boxes()
                    .any(|t| !t.plain_text().trim().is_empty())
            })
            .unwrap();
        let mut st = UiState::default();
        open_quick(&mut st, Path::new("/x.pro6"), &d, r);
        let mut q = st.quick.take().unwrap();
        let b = q
            .boxes
            .iter_mut()
            .find(|b| !b.text.trim().is_empty())
            .unwrap();
        b.text.push_str(" (fixed)");
        let element = b.element;
        let want = b.text.clone();
        let op = quick_op(&d, &q).unwrap();
        assert!(
            matches!(doc::check(&d, &op), doc::Check::Ok),
            "a .pro6 can hold it"
        );
        let mut after = d.clone();
        op.apply(&mut after).unwrap();
        let doc::Element::Text(t) = &after.groups[r.group].slides[r.slide].elements[element] else {
            panic!("not text");
        };
        assert_eq!(t.plain_text(), want);
        let doc::Element::Text(t0) = &d.groups[r.group].slides[r.slide].elements[element] else {
            panic!("not text");
        };
        assert_eq!(
            t.runs().next().unwrap().font,
            t0.runs().next().unwrap().font
        );
        assert_eq!(t.frame, t0.frame);
    }
}
