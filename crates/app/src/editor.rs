//! Edit mode for the presentation grid: slide text, slides, groups, arrangements.
//! Clicking a slide selects it for editing; nothing goes live from here.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::Sender;
use egui::{Color32, RichText, Stroke, Vec2};
use pp6core::{Command, ShowState};
use pro6::ops::{Op, new_uuid};

use crate::thumbs::ThumbRequest;
use crate::ui::{UiAction, UiState, color32, tile_image};

const TILE_W: u32 = 160;

#[derive(Default)]
pub struct EditState {
    pub on: bool,
    /// Selected slide (group, slide index).
    sel: Option<(usize, usize)>,
    text: String,
    /// Which slide (path, UUID) `text` was loaded from.
    text_for: Option<(PathBuf, String)>,
    rename: Option<(usize, String)>,
    confirm_delete_group: Option<usize>,
    arrangement: Option<String>,
    arrangement_name: String,
}

impl EditState {
    /// Start in edit mode with a slide selected (development / screenshots).
    pub fn select(&mut self, group: usize, slide: usize) {
        self.on = true;
        self.sel = Some((group, slide));
    }
}

fn edit(actions: &mut Vec<UiAction>, path: &Path, op: Op) {
    actions.push(UiAction::Core(Command::Edit {
        presentation: path.to_path_buf(),
        op,
    }));
}

fn slide_text(doc: &pro6::Document, g: usize, s: usize) -> String {
    doc.groups()
        .get(g)
        .and_then(|gr| gr.slides().get(s))
        .and_then(|sl| sl.text_elements().next())
        // A blank slide's placeholder edits as empty text.
        .filter(|t| !t.is_placeholder(&doc.path))
        .map(|t| match t.rtf_doc(&doc.path) {
            Ok(Some(r)) => r.plain_text(),
            _ => t
                .plain_text
                .as_ref()
                .map(|p| p.value.text())
                .unwrap_or_default(),
        })
        .unwrap_or_default()
}

fn to_rgba(c: Color32) -> pro6::Rgba {
    let f = |v: u8| f64::from(v) / 255.0;
    pro6::Rgba {
        r: f(c.r()),
        g: f(c.g()),
        b: f(c.b()),
        a: 1.0,
    }
}

pub fn grid_edit(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    thumbs: &Sender<ThumbRequest>,
    actions: &mut Vec<UiAction>,
    path: &Path,
    doc: &Arc<pro6::Document>,
) {
    let unsaved = show.unsaved.contains(path);
    ui.horizontal_wrapped(|ui| {
        ui.heading(format!(
            "Editing: {}{}",
            crate::ui::title(path),
            if unsaved { " •" } else { "" }
        ));
        if ui.button("Done editing").clicked() {
            st.edit.on = false;
        }
        let save = egui::Button::new(RichText::new("Save").strong()).fill(if unsaved {
            Color32::from_rgb(40, 90, 40)
        } else {
            Color32::TRANSPARENT
        });
        if ui.add_enabled(unsaved, save).clicked() {
            actions.push(UiAction::Core(Command::Save(path.to_path_buf())));
        }
        if ui
            .add_enabled(unsaved, egui::Button::new("Revert"))
            .on_hover_text("Throw away unsaved changes")
            .clicked()
        {
            actions.push(UiAction::Core(Command::Revert(path.to_path_buf())));
            st.edit.text_for = None;
        }
        if unsaved {
            ui.label(
                RichText::new("unsaved changes")
                    .color(Color32::YELLOW)
                    .small(),
            );
        }
    });

    // Keep the selection valid after edits.
    if let Some((g, s)) = st.edit.sel
        && doc.groups().get(g).is_none_or(|gr| s >= gr.slides().len())
    {
        st.edit.sel = None;
    }

    arrangements(ui, st, actions, path, doc);
    ui.separator();

    let height = TILE_W as f32 * doc.canvas.height as f32 / doc.canvas.width as f32;
    let editor_h = if st.edit.sel.is_some() { 190.0 } else { 0.0 };
    egui::ScrollArea::vertical()
        .id_salt("edit-grid")
        .max_height((ui.available_height() - editor_h).max(120.0))
        .show(ui, |ui| {
            let n_groups = doc.groups().len();
            for (gi, group) in doc.groups().iter().enumerate() {
                group_header(ui, st, actions, path, gi, group, n_groups);
                let spacing = ui.spacing().item_spacing.x;
                let per_row = (((ui.available_width().min(ui.clip_rect().width()) + spacing - 12.0)
                    / (TILE_W as f32 + spacing))
                    .floor() as usize)
                    .max(1);
                let n = group.slides().len();
                let idx: Vec<usize> = (0..n).collect();
                for row in idx.chunks(per_row) {
                    ui.horizontal(|ui| {
                        for &si in row {
                            let flat = doc.groups()[..gi]
                                .iter()
                                .map(|g| g.slides().len())
                                .sum::<usize>()
                                + si;
                            ui.vertical(|ui| {
                                let (_, tex) = st.thumb_public(thumbs, doc, flat);
                                let resp = tile_image(ui, tex, Vec2::new(TILE_W as f32, height));
                                let selected = st.edit.sel == Some((gi, si));
                                let stroke = if selected {
                                    Stroke::new(3.0, Color32::from_rgb(90, 160, 255))
                                } else {
                                    Stroke::new(1.0, color32(group.color))
                                };
                                if !group.slides()[si].enabled {
                                    ui.painter().rect_filled(
                                        resp.rect,
                                        0.0,
                                        Color32::from_black_alpha(170),
                                    );
                                }
                                ui.painter().rect_stroke(
                                    resp.rect,
                                    2.0,
                                    stroke,
                                    egui::StrokeKind::Outside,
                                );
                                if resp.clicked() {
                                    st.edit.sel = Some((gi, si));
                                }
                                ui.horizontal(|ui| {
                                    if si > 0
                                        && ui.small_button("<").on_hover_text("Move left").clicked()
                                    {
                                        edit(
                                            actions,
                                            path,
                                            Op::MoveSlide {
                                                group: gi,
                                                slide: si,
                                                to_group: gi,
                                                to: si - 1,
                                            },
                                        );
                                        st.edit.sel = Some((gi, si - 1));
                                    }
                                    if si + 1 < n
                                        && ui
                                            .small_button(">")
                                            .on_hover_text("Move right")
                                            .clicked()
                                    {
                                        edit(
                                            actions,
                                            path,
                                            Op::MoveSlide {
                                                group: gi,
                                                slide: si,
                                                to_group: gi,
                                                to: si + 1,
                                            },
                                        );
                                        st.edit.sel = Some((gi, si + 1));
                                    }
                                    if ui.small_button("Dup").on_hover_text("Duplicate").clicked() {
                                        edit(
                                            actions,
                                            path,
                                            Op::DuplicateSlide {
                                                group: gi,
                                                slide: si,
                                            },
                                        );
                                    }
                                    if ui
                                        .small_button("Del")
                                        .on_hover_text("Delete slide")
                                        .clicked()
                                    {
                                        edit(
                                            actions,
                                            path,
                                            Op::RemoveSlide {
                                                group: gi,
                                                slide: si,
                                            },
                                        );
                                        st.edit.sel = None;
                                    }
                                });
                            });
                        }
                    });
                }
                if n == 0 {
                    ui.label(RichText::new("(empty group: use + Slide)").small().weak());
                }
                ui.add_space(6.0);
            }
            if ui.button("+ Add group").clicked() {
                edit(
                    actions,
                    path,
                    Op::AddGroup {
                        at: doc.groups().len(),
                        name: "New Group".into(),
                        color: Some(pro6::Rgba {
                            r: 0.5,
                            g: 0.5,
                            b: 0.5,
                            a: 1.0,
                        }),
                    },
                );
            }
        });

    if let Some((g, s)) = st.edit.sel {
        slide_editor(ui, st, actions, path, doc, g, s);
    }
}

fn group_header(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    gi: usize,
    group: &pro6::SlideGroup,
    n_groups: usize,
) {
    ui.horizontal(|ui| {
        let mut c = color32(group.color);
        if egui::color_picker::color_edit_button_srgba(
            ui,
            &mut c,
            egui::color_picker::Alpha::Opaque,
        )
        .changed()
        {
            edit(
                actions,
                path,
                Op::RecolorGroup {
                    group: gi,
                    color: Some(to_rgba(c)),
                },
            );
        }
        match &mut st.edit.rename {
            Some((g, name)) if *g == gi => {
                let r = ui.text_edit_singleline(name);
                let done = r.lost_focus() || ui.small_button("OK").clicked();
                if done {
                    let new = name.trim().to_owned();
                    if !new.is_empty() && new != group.name {
                        edit(
                            actions,
                            path,
                            Op::RenameGroup {
                                group: gi,
                                name: new,
                            },
                        );
                    }
                    st.edit.rename = None;
                }
            }
            _ => {
                ui.strong(if group.name.is_empty() {
                    "(unnamed group)"
                } else {
                    &group.name
                });
                if ui.small_button("Rename").clicked() {
                    st.edit.rename = Some((gi, group.name.clone()));
                }
            }
        }
        if gi > 0
            && ui
                .small_button("^")
                .on_hover_text("Move group up")
                .clicked()
        {
            edit(
                actions,
                path,
                Op::MoveGroup {
                    from: gi,
                    to: gi - 1,
                },
            );
        }
        if gi + 1 < n_groups
            && ui
                .small_button("v")
                .on_hover_text("Move group down")
                .clicked()
        {
            edit(
                actions,
                path,
                Op::MoveGroup {
                    from: gi,
                    to: gi + 1,
                },
            );
        }
        if ui.small_button("+ Slide").clicked() {
            edit(
                actions,
                path,
                Op::AddSlide {
                    group: gi,
                    at: group.slides().len(),
                    text: String::new(),
                },
            );
            st.edit.sel = Some((gi, group.slides().len()));
        }
        if st.edit.confirm_delete_group == Some(gi) {
            let label = format!(
                "Really delete '{}' and its {} slide(s)?",
                group.name,
                group.slides().len()
            );
            if ui
                .small_button(RichText::new(label).color(Color32::LIGHT_RED))
                .clicked()
            {
                edit(actions, path, Op::RemoveGroup { group: gi });
                st.edit.confirm_delete_group = None;
                st.edit.sel = None;
            }
            if ui.small_button("Cancel").clicked() {
                st.edit.confirm_delete_group = None;
            }
        } else if ui.small_button("Delete group").clicked() {
            st.edit.confirm_delete_group = Some(gi);
        }
    });
}

fn slide_editor(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    doc: &pro6::Document,
    g: usize,
    s: usize,
) {
    let Some(slide) = doc.groups().get(g).and_then(|gr| gr.slides().get(s)) else {
        return;
    };
    let key = (path.to_path_buf(), slide.uuid.clone());
    if st.edit.text_for.as_ref() != Some(&key) {
        st.edit.text = slide_text(doc, g, s);
        st.edit.text_for = Some(key);
    }
    ui.separator();
    ui.horizontal(|ui| {
        ui.strong(format!("Slide {} in {}", s + 1, doc.groups()[g].name));
        let has_text = slide.text_elements().next().is_some();
        if !has_text {
            ui.label(RichText::new("This slide has no text box.").color(Color32::YELLOW));
        }
        ui.label("Move to group:");
        egui::ComboBox::from_id_salt("move-to-group")
            .selected_text(doc.groups()[g].name.clone())
            .show_ui(ui, |ui| {
                for (i, other) in doc.groups().iter().enumerate() {
                    if i != g && ui.selectable_label(false, &other.name).clicked() {
                        let to = other.slides().len();
                        edit(
                            actions,
                            path,
                            Op::MoveSlide {
                                group: g,
                                slide: s,
                                to_group: i,
                                to,
                            },
                        );
                        st.edit.sel = Some((i, to));
                    }
                }
            });
    });
    let r = ui.add(
        egui::TextEdit::multiline(&mut st.edit.text)
            .desired_rows(5)
            .desired_width(f32::INFINITY)
            .hint_text("Slide text (Enter = new line)"),
    );
    let ctrl_enter =
        r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
    ui.horizontal(|ui| {
        if ui
            .button("Apply text")
            .on_hover_text("Ctrl+Enter")
            .clicked()
            || ctrl_enter
        {
            let text = st.edit.text.trim_end_matches('\n').to_owned();
            edit(
                actions,
                path,
                Op::SetText {
                    group: g,
                    slide: s,
                    text,
                },
            );
        }
        ui.label(
            RichText::new("Font, size and colour come from the slide's existing text.")
                .small()
                .weak(),
        );
    });
}

fn arrangements(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    doc: &pro6::Document,
) {
    let list = doc.arrangements();
    if st
        .edit
        .arrangement
        .as_ref()
        .is_some_and(|a| !list.iter().any(|x| &x.uuid == a))
    {
        st.edit.arrangement = None;
    }
    egui::CollapsingHeader::new("Arrangements")
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let current = st
                    .edit
                    .arrangement
                    .as_ref()
                    .and_then(|a| list.iter().find(|x| &x.uuid == a));
                egui::ComboBox::from_id_salt("edit-arrangement")
                    .selected_text(
                        current.map_or("Choose an arrangement".to_owned(), |a| a.name.clone()),
                    )
                    .show_ui(ui, |ui| {
                        for a in &list {
                            if ui
                                .selectable_label(
                                    st.edit.arrangement.as_ref() == Some(&a.uuid),
                                    &a.name,
                                )
                                .clicked()
                            {
                                st.edit.arrangement = Some(a.uuid.clone());
                                st.edit.arrangement_name = a.name.clone();
                            }
                        }
                    });
                if ui.button("New arrangement").clicked() {
                    let uuid = new_uuid();
                    let groups: Vec<usize> = (0..doc.groups().len()).collect();
                    edit(
                        actions,
                        path,
                        Op::AddArrangement {
                            uuid: uuid.clone(),
                            name: "New arrangement".into(),
                            groups,
                        },
                    );
                    st.edit.arrangement = Some(uuid);
                    st.edit.arrangement_name = "New arrangement".into();
                }
            });
            let Some(a) = st
                .edit
                .arrangement
                .as_ref()
                .and_then(|a| list.iter().find(|x| &x.uuid == a))
            else {
                return;
            };
            let uuid = a.uuid.clone();
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.text_edit_singleline(&mut st.edit.arrangement_name);
                if ui.small_button("Rename").clicked()
                    && !st.edit.arrangement_name.trim().is_empty()
                {
                    edit(
                        actions,
                        path,
                        Op::RenameArrangement {
                            uuid: uuid.clone(),
                            name: st.edit.arrangement_name.trim().to_owned(),
                        },
                    );
                }
                if ui
                    .small_button(RichText::new("Delete arrangement").color(Color32::LIGHT_RED))
                    .clicked()
                {
                    edit(actions, path, Op::RemoveArrangement { uuid: uuid.clone() });
                    st.edit.arrangement = None;
                }
            });
            if !a.unknown_group_ids.is_empty() {
                ui.label(
                    RichText::new(format!(
                        "{} entry(ies) refer to groups that no longer exist.",
                        a.unknown_group_ids.len()
                    ))
                    .color(Color32::YELLOW)
                    .small(),
                );
            }
            let seq = a.groups.clone();
            ui.horizontal_wrapped(|ui| {
                for (i, &g) in seq.iter().enumerate() {
                    let group = &doc.groups()[g];
                    egui::Frame::new()
                        .stroke(Stroke::new(1.0, color32(group.color)))
                        .inner_margin(3.0)
                        .corner_radius(3.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(&group.name);
                                let mut set = |new: Vec<usize>| {
                                    edit(
                                        actions,
                                        path,
                                        Op::SetArrangementGroups {
                                            uuid: uuid.clone(),
                                            groups: new,
                                        },
                                    )
                                };
                                if i > 0 && ui.small_button("<").clicked() {
                                    let mut n = seq.clone();
                                    n.swap(i, i - 1);
                                    set(n);
                                }
                                if i + 1 < seq.len() && ui.small_button(">").clicked() {
                                    let mut n = seq.clone();
                                    n.swap(i, i + 1);
                                    set(n);
                                }
                                if ui.small_button("x").clicked() {
                                    let mut n = seq.clone();
                                    n.remove(i);
                                    set(n);
                                }
                            });
                        });
                }
                egui::ComboBox::from_id_salt("arr-add")
                    .selected_text("+ add group")
                    .show_ui(ui, |ui| {
                        for (gi, g) in doc.groups().iter().enumerate() {
                            if ui.selectable_label(false, &g.name).clicked() {
                                let mut n = seq.clone();
                                n.push(gi);
                                edit(
                                    actions,
                                    path,
                                    Op::SetArrangementGroups {
                                        uuid: uuid.clone(),
                                        groups: n,
                                    },
                                );
                            }
                        }
                    });
            });
        });
}
