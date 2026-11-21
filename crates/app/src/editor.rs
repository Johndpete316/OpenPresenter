//! The slide editor (docs/EDITOR-PLAN.md E1). Layout modelled on ProPresenter:
//!
//! ```text
//! ┌ top bar: title • | Undo Redo | Save Revert |              Close editor ┐
//! ├ contextual bar (what's selected)                                       ┤
//! │ slide strip   │ rulers + canvas (rendered by `render`)   │ inspector   │
//! │ (groups)      │                                          │ Text/Slide  │
//! ├ bottom bar: X Y W H R | zoom | notes | background colour               ┤
//! ```
//!
//! Every change is a `doc::Edit` sent to the core, which applies it to the editor
//! copy (`ShowState::editing`): the output and the operator's grid only change on
//! Save. A drag is one edit, sent on release; while dragging, the canvas renders a
//! preview copy so what you see is what the output will show.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::Sender;
use doc::{Edit as Op, Element, new_uuid};
use egui::{
    Align2, Color32, CursorIcon, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2,
};
use pp6core::{Command, ShowState};

use crate::thumbs::ThumbRequest;
use crate::ui::{UiAction, UiState, color32, paint_error};

mod arrange;
mod image;
mod snap;
mod text;

const RULER: f32 = 18.0;
const HANDLE: f32 = 4.5;
/// Rotation handle distance above the box, in screen points.
const ROTATE_ARM: f32 = 22.0;
/// Smallest element size, in canvas pixels.
const MIN_SIZE: f64 = 8.0;
const SELECT: Color32 = Color32::from_rgb(90, 160, 255);

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    #[default]
    Fit,
    /// Percent: 100 = one canvas pixel per screen point.
    Pct(u32),
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Tab {
    #[default]
    Text,
    /// Outline and shadow (opens from the text toolbar, Canva-style).
    Effects,
    Image,
    /// Image adjustments (opens from the image toolbar or the background image).
    Adjust,
    /// Arrange, align, distribute, exact position.
    Position,
    Layers,
    Slide,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Grab {
    Move,
    /// Resize by a handle: -1 / 0 / 1 for left-centre-right and top-middle-bottom.
    Resize(i8, i8),
    Rotate,
}

struct Drag {
    grab: Grab,
    element: usize,
    /// Pointer at the start, canvas pixels.
    start: Pos2,
    frame: doc::Rect,
    rotation: f64,
    now_frame: doc::Rect,
    now_rotation: f64,
    /// Resizing keeps the shape (images by default; Shift switches).
    keep_aspect: bool,
    /// Elements moved (several when a group is dragged): index, frame and rotation
    /// at the start. With several, `frame` is their combined bounds.
    members: Vec<(usize, doc::Rect, f64)>,
    /// Preview document for the canvas while dragging.
    preview: Option<(doc::Rect, f64, Arc<doc::Document>)>,
}

#[derive(Default)]
pub struct EditState {
    pub on: bool,
    /// Asked to leave the editor (asks about unsaved edits first).
    pub close_requested: bool,
    /// Selected slide (group, slide index).
    sel: Option<(usize, usize)>,
    /// Selected element (index into the slide's elements); the primary one when
    /// several are selected.
    element: Option<usize>,
    /// The other selected elements (Shift-click, marquee).
    multi: Vec<usize>,
    /// Marquee being dragged on the empty canvas (canvas pixels).
    marquee: Option<(Pos2, Pos2)>,
    /// Snap guides of the drag in progress.
    guides: Vec<snap::Guide>,
    /// Bottom bar toggles: snapping (on unless `no_snap`), guides, safe area, grid.
    no_snap: bool,
    hide_guides: bool,
    safe_area: bool,
    grid: bool,
    /// Tab (`false`) / Shift+Tab (`true`) pressed with nothing focused this frame.
    tab_pressed: Option<bool>,
    /// Layer being dragged in the Layers tab.
    layer_drag: Option<usize>,
    /// Layer being renamed: element index and the name typed so far.
    layer_rename: Option<(usize, String)>,
    text: String,
    /// Which text box (path, slide UUID, element) `text` was loaded from.
    text_for: Option<(PathBuf, String, usize)>,
    focus_text: bool,
    notes: String,
    notes_for: Option<(PathBuf, String)>,
    rename: Option<(usize, String)>,
    confirm_delete_group: Option<usize>,
    arrangement: Option<String>,
    arrangement_name: String,
    zoom: Zoom,
    /// Canvas offset when zoomed in beyond the view (screen points).
    pan: Vec2,
    tab: Tab,
    drag: Option<Drag>,
    /// Slide being dragged in the strip.
    strip_drag: Option<(usize, usize)>,
    /// Canvas rectangle on screen and points per canvas pixel, last frame.
    view: Option<(Rect, f32)>,
    /// Slide tiles in the strip, last frame.
    tiles: Vec<((usize, usize), Rect)>,
    /// Text being edited in place on the canvas.
    session: Option<text::TextSession>,
    /// The pointer is selecting text in the session's box.
    session_drag: bool,
    /// An edit waiting for "Continue anyway" (won't work in ProPresenter 6).
    pending_native: Option<text::PendingNative>,
    recent_colors: Vec<[u8; 3]>,
    font_search: String,
    font_previews: std::collections::HashMap<String, Option<egui::TextureHandle>>,
    imports: image::Imports,
    /// The Adjust panel is for the slide's background image (else the selected image).
    adjust_background: bool,
}

impl EditState {
    /// Start in edit mode with a slide selected (development / screenshots).
    pub fn select(&mut self, group: usize, slide: usize) {
        self.on = true;
        self.sel = Some((group, slide));
    }

    /// Select an element too; again to add more (development / screenshots).
    pub fn select_element(&mut self, element: usize) {
        if self.element.is_none() {
            self.element = Some(element);
        } else {
            self.multi.push(element);
        }
    }

    fn select_slide(&mut self, g: usize, s: usize) {
        if self.sel != Some((g, s)) {
            self.sel = Some((g, s));
            self.element = None;
            self.multi.clear();
            self.drag = None;
        }
    }

    /// Selected elements, the primary one first.
    fn selection(&self) -> Vec<usize> {
        self.element
            .into_iter()
            .chain(self.multi.iter().copied())
            .collect()
    }

    /// Add `e` to the selection, or take it out if it's in.
    fn toggle(&mut self, e: usize) {
        if self.element == Some(e) {
            self.element = (!self.multi.is_empty()).then(|| self.multi.remove(0));
        } else if let Some(i) = self.multi.iter().position(|m| *m == e) {
            self.multi.remove(i);
        } else if self.element.is_none() {
            self.element = Some(e);
        } else {
            self.multi.push(e);
        }
    }
}

fn edit(actions: &mut Vec<UiAction>, path: &Path, op: Op) {
    actions.push(UiAction::Core(Command::Edit {
        presentation: path.to_path_buf(),
        edit: op,
        accept_native: false,
    }));
}

/// An edit that merges with the previous one of the same `key` into one undo step.
fn edit_merging(actions: &mut Vec<UiAction>, path: &Path, op: Op, key: String) {
    actions.push(UiAction::Core(Command::EditMerging {
        presentation: path.to_path_buf(),
        edit: op,
        key,
    }));
}

fn to_rgba(c: Color32) -> doc::Rgba {
    let f = |v: u8| f64::from(v) / 255.0;
    doc::Rgba {
        r: f(c.r()),
        g: f(c.g()),
        b: f(c.b()),
        a: 1.0,
    }
}

fn flat_index(d: &doc::Document, g: usize, s: usize) -> usize {
    d.groups[..g].iter().map(|g| g.slides.len()).sum::<usize>() + s
}

fn text_box(d: &doc::Document, g: usize, s: usize, e: usize) -> Option<&doc::TextBox> {
    match d.groups.get(g)?.slides.get(s)?.elements.get(e)? {
        Element::Text(t) => Some(t),
        _ => None,
    }
}

fn element(d: &doc::Document, g: usize, s: usize, e: usize) -> Option<&Element> {
    d.groups.get(g)?.slides.get(s)?.elements.get(e)
}

/// A text box to insert on slide `(g, s)`: a copy of the nearest existing one
/// (its style, outline and shadow; a `.pro6` can hold that), or a plain one.
fn new_text_box(d: &doc::Document, g: usize, s: usize) -> doc::TextBox {
    let template = d.groups[g].slides[s]
        .text_boxes()
        .chain(d.text_boxes())
        .find(|t| t.problem.is_none() && t.runs().next().is_some());
    let (cw, ch) = (f64::from(d.canvas.width), f64::from(d.canvas.height));
    let frame = doc::Rect {
        x: (cw * 0.2).round(),
        y: (ch * 0.4).round(),
        width: (cw * 0.6).round(),
        height: (ch * 0.2).round(),
    };
    match template {
        Some(t) => {
            let style = t.runs().next().cloned();
            let align = t.paragraphs.first().map(|p| p.align).unwrap_or_default();
            let mut n = t.clone();
            n.uuid = new_uuid();
            n.frame = frame;
            n.rotation = 0.0;
            n.locked = false;
            n.hidden = false;
            n.paragraphs = vec![doc::Paragraph {
                align,
                line_spacing: 1.0,
                runs: style
                    .map(|r| doc::Run {
                        text: "Text".into(),
                        ..r
                    })
                    .into_iter()
                    .collect(),
            }];
            n
        }
        None => doc::TextBox {
            uuid: new_uuid(),
            name: "Text".into(),
            frame,
            rotation: 0.0,
            opacity: 1.0,
            locked: false,
            hidden: false,
            valign: doc::VAlign::Middle,
            fill: None,
            corner_radius: 0.0,
            outline: None,
            shadow: None,
            paragraphs: vec![doc::Paragraph {
                align: doc::Align::Center,
                line_spacing: 1.0,
                runs: vec![doc::Run {
                    text: "Text".into(),
                    font: "Arial".into(),
                    size: (ch / 10.0).round(),
                    color: Some([255, 255, 255]),
                    bold: false,
                    italic: false,
                    underline: false,
                    letter_spacing: 0.0,
                }],
            }],
            problem: None,
            src: None,
        },
    }
}

/// Close the editor, asking first about unsaved edits.
fn close_prompt(
    ctx: &egui::Context,
    st: &mut UiState,
    show: &ShowState,
    path: &Path,
    actions: &mut Vec<UiAction>,
    pending_edit: bool,
) {
    if !st.edit.close_requested {
        return;
    }
    if !show.unsaved.contains(path) && !pending_edit {
        st.edit.close_requested = false;
        st.edit.on = false;
        return;
    }
    let mut close = None;
    egui::Modal::new(egui::Id::unique("editor-close")).show(ctx, |ui| {
        ui.set_max_width(380.0);
        ui.heading(format!("Save changes to {}?", crate::ui::title(path)));
        ui.label("Your edits haven't been saved yet.");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(RichText::new("Save").strong()).clicked() {
                close = Some(Some(true));
            }
            if ui.button("Don't save").clicked() {
                close = Some(Some(false));
            }
            if ui.button("Cancel").clicked() {
                close = Some(None);
            }
        });
    });
    match close {
        Some(Some(true)) => {
            actions.push(UiAction::Core(Command::Save(path.to_path_buf())));
            st.edit.on = false;
            st.edit.close_requested = false;
        }
        Some(Some(false)) => {
            actions.push(UiAction::Core(Command::Revert(path.to_path_buf())));
            st.edit.on = false;
            st.edit.close_requested = false;
        }
        Some(None) => st.edit.close_requested = false,
        None => {}
    }
}

/// The whole editor, filling `ui` (below the app toolbar).
pub fn editor(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    thumbs: &Sender<ThumbRequest>,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &Arc<doc::Document>,
) {
    // Keep the selection valid after edits (and pick a slide if there's none).
    if let Some((g, s)) = st.edit.sel
        && d.groups.get(g).is_none_or(|gr| s >= gr.slides.len())
    {
        st.edit.sel = None;
    }
    if st.edit.sel.is_none() {
        st.edit.element = None;
        st.edit.drag = None;
        st.edit.sel = d
            .groups
            .iter()
            .position(|g| !g.slides.is_empty())
            .map(|g| (g, 0));
    }
    if let Some((g, s)) = st.edit.sel {
        st.edit
            .multi
            .retain(|&e| element(d, g, s, e).and_then(Element::frame).is_some());
        if st.edit.element.is_none() {
            st.edit.multi.clear();
        }
    }
    // Tab with nothing focused cycles through the slide's elements instead of
    // moving keyboard focus to the first button.
    if st.edit.session.is_none() && ui.memory(|m| m.focused().is_none()) {
        let tab = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key: egui::Key::Tab,
                    pressed: true,
                    modifiers,
                    ..
                } => Some(modifiers.shift),
                _ => None,
            })
        });
        if tab.is_some() {
            ui.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
            st.edit.tab_pressed = tab;
        }
    }
    if let (Some((g, s)), Some(e)) = (st.edit.sel, st.edit.element)
        && element(d, g, s, e).and_then(Element::frame).is_none()
    {
        st.edit.element = None;
        st.edit.drag = None;
    }

    // The inspector follows the selection: Image for images, Text for text boxes.
    if let (Some((g, s)), Some(e)) = (st.edit.sel, st.edit.element) {
        match element(d, g, s, e) {
            Some(Element::Image(_)) if matches!(st.edit.tab, Tab::Text | Tab::Effects) => {
                st.edit.tab = Tab::Image;
            }
            Some(Element::Text(_))
                if st.edit.tab == Tab::Image
                    || (st.edit.tab == Tab::Adjust && !st.edit.adjust_background) =>
            {
                st.edit.tab = Tab::Text;
            }
            _ => {}
        }
    }
    text::check_session(st, actions, path, d);
    image::poll(ui, st, actions, path, d);
    top_bar(ui, st, show, actions, path, d);
    context_bar(ui, st, actions, path, d);
    bottom_bar(ui, st, actions, path, d);
    egui::Panel::left("editor-strip")
        .resizable(true)
        .default_size(230.0)
        .size_range(170.0..=420.0)
        .frame(
            egui::Frame::new()
                .fill(crate::theme::color::PANEL)
                .inner_margin(6.0),
        )
        .show(ui, |ui| strip(ui, st, thumbs, actions, path, d));
    egui::Panel::right("editor-inspector")
        .resizable(true)
        .default_size(290.0)
        .size_range(220.0..=480.0)
        .frame(
            egui::Frame::new()
                .fill(crate::theme::color::PANEL)
                .inner_margin(8.0),
        )
        .show(ui, |ui| inspector(ui, st, actions, path, d));
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(Color32::from_gray(24)))
        .show(ui, |ui| canvas(ui, st, actions, path, d));
    text::native_prompt(ui.ctx(), st, actions, path);
    let mut pending = false;
    if st.edit.close_requested {
        pending = st.edit.session.as_ref().is_some_and(|s| s.dirty());
        text::commit(st, actions, path, d);
    }
    close_prompt(ui.ctx(), st, show, path, actions, pending);
}

// ───────────────────────────── bars ─────────────────────────────

fn top_bar(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
) {
    let unsaved =
        show.unsaved.contains(path) || st.edit.session.as_ref().is_some_and(|s| s.dirty());
    egui::Panel::top("editor-top")
        .frame(egui::Frame::new().fill(crate::theme::color::HEADER).inner_margin(6.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(format!(
                    "{}{}",
                    crate::ui::title(path),
                    if unsaved { " •" } else { "" }
                ))
                .on_hover_text(if unsaved { "Unsaved changes" } else { "Saved" });
                ui.separator();
                let editing = show.editing.get(path);
                let undo = editing.and_then(|e| e.undo.as_deref());
                let redo = editing.and_then(|e| e.redo.as_deref());
                // Shortcuts only when no text field has focus (those have their own undo).
                let (key_undo, key_redo) = if ui.memory(|m| m.focused().is_none()) {
                    ui.input(|i| {
                        let z = i.key_pressed(egui::Key::Z) && i.modifiers.command;
                        (
                            z && !i.modifiers.shift,
                            (z && i.modifiers.shift)
                                || (i.key_pressed(egui::Key::Y) && i.modifiers.command),
                        )
                    })
                } else {
                    (false, false)
                };
                let r = ui
                    .add_enabled(undo.is_some(), egui::Button::new("Undo"))
                    .on_hover_text(undo.map_or("Nothing to undo".to_owned(), |l| {
                        format!("Undo {l} (Ctrl+Z)")
                    }));
                if (r.clicked() || key_undo) && undo.is_some() {
                    text::commit(st, actions, path, d);
                    actions.push(UiAction::Core(Command::Undo(path.to_path_buf())));
                    st.edit.text_for = None;
                    st.edit.notes_for = None;
                }
                let r = ui
                    .add_enabled(redo.is_some(), egui::Button::new("Redo"))
                    .on_hover_text(redo.map_or("Nothing to redo".to_owned(), |l| {
                        format!("Redo {l} (Ctrl+Shift+Z)")
                    }));
                if (r.clicked() || key_redo) && redo.is_some() {
                    text::commit(st, actions, path, d);
                    actions.push(UiAction::Core(Command::Redo(path.to_path_buf())));
                    st.edit.text_for = None;
                    st.edit.notes_for = None;
                }
                ui.separator();
                ui.label("Insert");
                if let Some((g, s)) = st.edit.sel {
                    if ui
                        .button("Text")
                        .on_hover_text("A new text box on this slide")
                        .clicked()
                    {
                        text::commit(st, actions, path, d);
                        let t = new_text_box(d, g, s);
                        let at = d.groups[g].slides[s].elements.len();
                        text::send_checked(
                            st,
                            actions,
                            path,
                            d,
                            Op::AddElement {
                                group: g,
                                slide: s,
                                at: None,
                                element: Element::Text(Box::new(t)),
                            },
                            None,
                        );
                        st.edit.element = Some(at);
                        st.edit.tab = Tab::Text;
                    }
                    let busy = st.edit.imports.running > 0;
                    if ui
                        .add_enabled(!busy, egui::Button::new(if busy { "Image…" } else { "Image" }))
                        .on_hover_text("A PNG or JPEG picture on this slide (you can also drop files on the editor or paste one)")
                        .clicked()
                    {
                        text::commit(st, actions, path, d);
                        image::start(st, ui.ctx(), path, image::Source::Pick, image::Target::Insert(g, s));
                    }
                }
                ui.separator();
                let save = egui::Button::new(RichText::new("Save").strong()).fill(if unsaved {
                    Color32::from_rgb(40, 90, 40)
                } else {
                    Color32::TRANSPARENT
                });
                let key_save = ui.input(|i| i.key_pressed(egui::Key::S) && i.modifiers.command);
                if (ui.add_enabled(unsaved, save).on_hover_text("Save (Ctrl+S)").clicked() || key_save)
                    && unsaved
                {
                    text::commit(st, actions, path, d);
                    actions.push(UiAction::Core(Command::Save(path.to_path_buf())));
                }
                if ui
                    .add_enabled(unsaved, egui::Button::new("Revert"))
                    .on_hover_text("Throw away unsaved changes (the saved file is untouched)")
                    .clicked()
                {
                    st.edit.session = None;
                    actions.push(UiAction::Core(Command::Revert(path.to_path_buf())));
                    st.edit.text_for = None;
                    st.edit.notes_for = None;
                }
                if d.mode == doc::Mode::Native {
                    ui.label(
                        RichText::new(format!(
                            "saves as {}.{} (OpenPresent only)",
                            crate::ui::title(path),
                            doc::NATIVE_EXT
                        ))
                        .small()
                        .weak(),
                    )
                    .on_hover_text("This presentation uses something a ProPresenter 6 file can't hold. The original .pro6 is left as it was.");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close editor").clicked() {
                        st.edit.close_requested = true;
                    }
                });
            });
        });
}

fn context_bar(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
) {
    egui::Panel::top("editor-context")
        .frame(egui::Frame::new().fill(crate::theme::color::PANEL).inner_margin(egui::Margin::symmetric(8, 5)))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let Some((g, s)) = st.edit.sel else {
                    ui.label(RichText::new("This presentation has no slides yet: add one with + Slide in the strip.").weak());
                    return;
                };
                let Some(e) = st.edit.element else {
                    ui.label(
                        RichText::new(format!(
                            "Slide {} in {} — click a text box on the canvas to select it.",
                            s + 1,
                            d.groups[g].name
                        ))
                        .weak(),
                    );
                    return;
                };
                let count = st.edit.selection().len();
                if count > 1 {
                    ui.label(RichText::new(format!("{count} selected")).strong());
                    ui.separator();
                    if ui
                        .button("Align…")
                        .on_hover_text("Align, distribute and arrange the selection")
                        .clicked()
                    {
                        st.edit.tab = Tab::Position;
                    }
                    ui.label(
                        RichText::new("Drag to move them together; Shift-click to add or remove one.")
                            .weak(),
                    );
                    return;
                }
                let Some(el) = element(d, g, s, e) else { return };
                match el {
                    Element::Text(_) => text::toolbar(ui, st, actions, path, d, (g, s, e)),
                    Element::Image(_) => image::toolbar(ui, st, actions, path, d, (g, s, e)),
                    Element::Pro6(_) => return,
                }
                let rotation = el.rotation().rem_euclid(360.0);
                if rotation != 0.0
                    && ui
                        .button(format!("Straighten ({rotation:.0}°)"))
                        .clicked()
                {
                    edit(
                        actions,
                        path,
                        Op::SetRotation {
                            group: g,
                            slide: s,
                            element: e,
                            degrees: 0.0,
                        },
                    );
                }
                if el.locked() {
                    ui.label(RichText::new("Locked: it can't be moved or resized here.").color(Color32::YELLOW));
                }
            });
        });
}

fn bottom_bar(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
) {
    egui::Panel::bottom("editor-bottom")
        .frame(
            egui::Frame::new()
                .fill(crate::theme::color::HEADER)
                .inner_margin(egui::Margin::symmetric(8, 5)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Position readout (live while dragging).
                let geom = match (&st.edit.drag, st.edit.sel, st.edit.element) {
                    (Some(dr), _, _) => Some((dr.now_frame, dr.now_rotation)),
                    (None, Some((g, s)), Some(e)) => element(d, g, s, e)
                        .and_then(|el| el.frame().map(|f| (f, el.rotation()))),
                    _ => None,
                };
                let readout = match geom {
                    Some((f, r)) => format!(
                        "X {:.0}  Y {:.0}  W {:.0}  H {:.0}  R {:.0}°",
                        f.x,
                        f.y,
                        f.width,
                        f.height,
                        r.rem_euclid(360.0)
                    ),
                    None => format!("Canvas {} × {}", d.canvas.width, d.canvas.height),
                };
                ui.add_sized(
                    [250.0, 18.0],
                    egui::Label::new(RichText::new(readout).monospace()),
                );
                ui.separator();
                let zoom_name = |z: Zoom| match z {
                    Zoom::Fit => "Fit".to_owned(),
                    Zoom::Pct(p) => format!("{p}%"),
                };
                egui::ComboBox::from_id_salt("editor-zoom")
                    .width(64.0)
                    .selected_text(zoom_name(st.edit.zoom))
                    .show_ui(ui, |ui| {
                        for z in [Zoom::Fit, Zoom::Pct(50), Zoom::Pct(100), Zoom::Pct(200)] {
                            if ui
                                .selectable_value(&mut st.edit.zoom, z, zoom_name(z))
                                .clicked()
                            {
                                st.edit.pan = Vec2::ZERO;
                            }
                        }
                    });
                ui.separator();
                let mut snap_on = !st.edit.no_snap;
                ui.checkbox(&mut snap_on, "Snap")
                    .on_hover_text("Snap to the canvas, safe area and other elements while dragging (hold Alt to drag freely)");
                st.edit.no_snap = !snap_on;
                let mut guides = !st.edit.hide_guides;
                ui.checkbox(&mut guides, "Guides")
                    .on_hover_text("Show the magenta snap guides");
                st.edit.hide_guides = !guides;
                ui.checkbox(&mut st.edit.safe_area, "Safe area")
                    .on_hover_text("Show the title-safe area (5% in from each edge)");
                ui.checkbox(&mut st.edit.grid, "Grid")
                    .on_hover_text("Show a grid and snap to it");
                ui.separator();
                let Some((g, s)) = st.edit.sel else { return };
                let slide = &d.groups[g].slides[s];
                sync_notes(st, path, slide);
                ui.label("Notes");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut st.edit.notes)
                        .desired_width(260.0)
                        .hint_text("Slide notes (not shown on the output)"),
                );
                if r.lost_focus() && st.edit.notes != slide.notes {
                    edit(
                        actions,
                        path,
                        Op::SetSlideNotes {
                            group: g,
                            slide: s,
                            notes: st.edit.notes.clone(),
                        },
                    );
                }
                ui.separator();
                background_picker(ui, actions, path, d, g, s);
            });
        });
}

fn sync_notes(st: &mut UiState, path: &Path, slide: &doc::Slide) {
    let key = (path.to_path_buf(), slide.uuid.clone());
    if st.edit.notes_for.as_ref() != Some(&key) {
        st.edit.notes.clone_from(&slide.notes);
        st.edit.notes_for = Some(key);
    }
}

fn background_picker(
    ui: &mut egui::Ui,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    g: usize,
    s: usize,
) {
    let slide = &d.groups[g].slides[s];
    let mut on = slide.background.is_some();
    if ui
        .checkbox(&mut on, "Background")
        .on_hover_text("This slide's own background colour")
        .changed()
    {
        let color = on.then(|| {
            slide.background.or(d.background).unwrap_or(doc::Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            })
        });
        edit(
            actions,
            path,
            Op::SetSlideBackground {
                group: g,
                slide: s,
                color,
            },
        );
    }
    if let Some(c) = slide.background {
        let [r, gg, b, _] = c.to_rgba8();
        let mut col = Color32::from_rgb(r, gg, b);
        if egui::color_picker::color_edit_button_srgba(
            ui,
            &mut col,
            egui::color_picker::Alpha::Opaque,
        )
        .changed()
        {
            edit_merging(
                actions,
                path,
                Op::SetSlideBackground {
                    group: g,
                    slide: s,
                    color: Some(to_rgba(col)),
                },
                format!("background:{}", slide.uuid),
            );
        }
    }
}

// ───────────────────────────── slide strip ─────────────────────────────

/// Where a dragged slide would go: insert at `index` in `group` (before removal).
#[derive(Clone, Copy)]
struct Slot {
    group: usize,
    index: usize,
    y: f32,
}

fn strip(
    ui: &mut egui::Ui,
    st: &mut UiState,
    thumbs: &Sender<ThumbRequest>,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &Arc<doc::Document>,
) {
    egui::ScrollArea::vertical()
        .id_salt("editor-strip-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            arrangements(ui, st, actions, path, d);
            ui.separator();
            let mut slots: Vec<Slot> = Vec::new();
            st.edit.tiles.clear();
            let n_groups = d.groups.len();
            let tile_w = (ui.available_width() - 4.0).max(60.0);
            let tile_h = tile_w * d.canvas.height as f32 / d.canvas.width.max(1) as f32;
            for (gi, group) in d.groups.iter().enumerate() {
                let header = group_header(ui, st, actions, path, d, gi, n_groups);
                slots.push(Slot {
                    group: gi,
                    index: 0,
                    y: header.bottom() + 1.0,
                });
                for (si, slide) in group.slides.iter().enumerate() {
                    let flat = flat_index(d, gi, si);
                    let (rect, resp) =
                        ui.allocate_exact_size(Vec2::new(tile_w, tile_h), Sense::click_and_drag());
                    st.edit.tiles.push(((gi, si), rect));
                    let (key, tex) = st.thumb_public(thumbs, d, flat);
                    match tex {
                        Some(Ok(t)) => {
                            egui::Image::new(t).paint_at(ui, rect);
                        }
                        Some(Err(e)) => paint_error(ui, rect, e),
                        None => {
                            ui.painter().rect_filled(rect, 0.0, Color32::from_gray(18));
                        }
                    }
                    let p = ui.painter();
                    if !slide.enabled {
                        p.rect_filled(rect, 0.0, Color32::from_black_alpha(170));
                    }
                    let selected = st.edit.sel == Some((gi, si));
                    p.rect_stroke(
                        rect,
                        2.0,
                        if selected {
                            Stroke::new(3.0, SELECT)
                        } else {
                            Stroke::new(1.0, color32(group.color))
                        },
                        StrokeKind::Outside,
                    );
                    let label = if slide.label.is_empty() {
                        format!("{}", si + 1)
                    } else {
                        format!("{}  {}", si + 1, slide.label)
                    };
                    let badge = Rect::from_min_size(
                        rect.min,
                        Vec2::new(22.0 + 6.5 * label.len() as f32, 16.0),
                    );
                    p.rect_filled(badge, 2.0, Color32::from_black_alpha(160));
                    p.text(
                        badge.left_center() + Vec2::new(4.0, 0.0),
                        Align2::LEFT_CENTER,
                        label,
                        FontId::proportional(11.0),
                        Color32::WHITE,
                    );
                    if st.thumb_overflow.contains(&key) {
                        let b = Rect::from_min_size(
                            rect.left_bottom() + Vec2::new(2.0, -17.0),
                            Vec2::new(78.0, 15.0),
                        );
                        p.rect_filled(b, 3.0, Color32::from_rgb(190, 30, 30));
                        p.text(
                            b.center(),
                            Align2::CENTER_CENTER,
                            "doesn't fit",
                            FontId::proportional(10.0),
                            Color32::WHITE,
                        );
                    }
                    if slide.pro6_incompatible {
                        let b = Rect::from_min_size(
                            rect.right_top() + Vec2::new(-52.0, 2.0),
                            Vec2::new(50.0, 15.0),
                        );
                        p.rect_filled(b, 3.0, Color32::from_rgb(170, 40, 40));
                        p.text(
                            b.center(),
                            Align2::CENTER_CENTER,
                            "not PP6",
                            FontId::proportional(10.0),
                            Color32::WHITE,
                        );
                    }
                    if resp.clicked() {
                        st.edit.select_slide(gi, si);
                    }
                    if resp.drag_started() {
                        st.edit.strip_drag = Some((gi, si));
                        st.edit.select_slide(gi, si);
                    }
                    let resp = if slide.pro6_incompatible {
                        resp.on_hover_text("Uses something a ProPresenter 6 file can't hold")
                    } else {
                        resp
                    };
                    resp.context_menu(|ui| slide_menu(ui, st, actions, path, d, gi, si));
                    slots.push(Slot {
                        group: gi,
                        index: si + 1,
                        y: rect.bottom() + 2.0,
                    });
                    ui.add_space(4.0);
                }
                if group.slides.is_empty() {
                    ui.label(RichText::new("(empty group)").small().weak());
                }
                ui.add_space(8.0);
            }
            if ui.button("+ Add group").clicked() {
                edit(
                    actions,
                    path,
                    Op::AddGroup {
                        at: d.groups.len(),
                        name: "New Group".into(),
                        color: Some(doc::Rgba {
                            r: 0.5,
                            g: 0.5,
                            b: 0.5,
                            a: 1.0,
                        }),
                    },
                );
            }
            strip_drop(ui, st, actions, path, &slots);
        });
}

/// Drag a slide to reorder it, within its group or into another one.
fn strip_drop(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    slots: &[Slot],
) {
    let Some((sg, ss)) = st.edit.strip_drag else {
        return;
    };
    let (pointer, released) = ui.input(|i| (i.pointer.interact_pos(), i.pointer.any_released()));
    let target = pointer.and_then(|p| {
        slots
            .iter()
            .min_by(|a, b| (a.y - p.y).abs().total_cmp(&(b.y - p.y).abs()))
            .copied()
    });
    if let Some(t) = target {
        let x = ui.min_rect().x_range();
        ui.painter().hline(x, t.y, Stroke::new(3.0, SELECT));
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    }
    if released {
        st.edit.strip_drag = None;
        if let Some(t) = target {
            let to = if t.group == sg && t.index > ss {
                t.index - 1
            } else {
                t.index
            };
            if !(t.group == sg && to == ss) {
                edit(
                    actions,
                    path,
                    Op::MoveSlide {
                        group: sg,
                        slide: ss,
                        to_group: t.group,
                        to,
                    },
                );
                st.edit.select_slide(t.group, to);
            }
        }
    }
}

fn slide_menu(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    gi: usize,
    si: usize,
) {
    let slide = &d.groups[gi].slides[si];
    if ui.button("Duplicate").clicked() {
        edit(
            actions,
            path,
            Op::DuplicateSlide {
                group: gi,
                slide: si,
            },
        );
        st.edit.select_slide(gi, si + 1);
        ui.close();
    }
    if ui.button("Add slide after").clicked() {
        edit(
            actions,
            path,
            Op::AddSlide {
                group: gi,
                at: si + 1,
                text: String::new(),
            },
        );
        st.edit.select_slide(gi, si + 1);
        ui.close();
    }
    let toggle = if slide.enabled { "Disable" } else { "Enable" };
    if ui.button(toggle).clicked() {
        edit(
            actions,
            path,
            Op::SetSlideEnabled {
                group: gi,
                slide: si,
                enabled: !slide.enabled,
            },
        );
        ui.close();
    }
    ui.menu_button("Move to group", |ui| {
        for (i, other) in d.groups.iter().enumerate() {
            if i != gi && ui.button(&other.name).clicked() {
                let to = other.slides.len();
                edit(
                    actions,
                    path,
                    Op::MoveSlide {
                        group: gi,
                        slide: si,
                        to_group: i,
                        to,
                    },
                );
                st.edit.select_slide(i, to);
                ui.close();
            }
        }
    });
    if ui.button("New group after this one").clicked() {
        edit(
            actions,
            path,
            Op::AddGroup {
                at: gi + 1,
                name: "New Group".into(),
                color: Some(doc::Rgba {
                    r: 0.5,
                    g: 0.5,
                    b: 0.5,
                    a: 1.0,
                }),
            },
        );
        ui.close();
    }
    ui.separator();
    if ui
        .button(RichText::new("Delete slide").color(Color32::LIGHT_RED))
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
        ui.close();
    }
}

/// Group name with its PP6 colour and a menu. Returns the header's rect.
fn group_header(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    gi: usize,
    n_groups: usize,
) -> Rect {
    let group = &d.groups[gi];
    ui.horizontal(|ui| {
        let mut c = color32(group.color);
        if egui::color_picker::color_edit_button_srgba(
            ui,
            &mut c,
            egui::color_picker::Alpha::Opaque,
        )
        .on_hover_text("Group colour")
        .changed()
        {
            edit_merging(
                actions,
                path,
                Op::RecolorGroup {
                    group: gi,
                    color: Some(to_rgba(c)),
                },
                format!("recolor:{}", group.uuid),
            );
        }
        match &mut st.edit.rename {
            Some((g, name)) if *g == gi => {
                let r = ui.add(
                    egui::TextEdit::singleline(name).desired_width(ui.available_width() - 30.0),
                );
                r.request_focus();
                if r.lost_focus() {
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
                let name = if group.name.is_empty() {
                    "(unnamed group)"
                } else {
                    &group.name
                };
                if ui
                    .add(egui::Label::new(RichText::new(name).strong()).sense(Sense::click()))
                    .on_hover_text("Double-click to rename")
                    .double_clicked()
                {
                    st.edit.rename = Some((gi, group.name.clone()));
                }
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(
                crate::theme::icon_text(crate::theme::icon::LIST, 14.0),
                |ui| {
                    if ui.button("Rename").clicked() {
                        st.edit.rename = Some((gi, group.name.clone()));
                        ui.close();
                    }
                    if ui.button("Add slide").clicked() {
                        edit(
                            actions,
                            path,
                            Op::AddSlide {
                                group: gi,
                                at: group.slides.len(),
                                text: String::new(),
                            },
                        );
                        st.edit.select_slide(gi, group.slides.len());
                        ui.close();
                    }
                    if gi > 0 && ui.button("Move up").clicked() {
                        edit(
                            actions,
                            path,
                            Op::MoveGroup {
                                from: gi,
                                to: gi - 1,
                            },
                        );
                        st.edit.sel = None;
                        ui.close();
                    }
                    if gi + 1 < n_groups && ui.button("Move down").clicked() {
                        edit(
                            actions,
                            path,
                            Op::MoveGroup {
                                from: gi,
                                to: gi + 1,
                            },
                        );
                        st.edit.sel = None;
                        ui.close();
                    }
                    ui.separator();
                    if st.edit.confirm_delete_group == Some(gi) {
                        let label = format!(
                            "Really delete '{}' and its {} slide(s)?",
                            group.name,
                            group.slides.len()
                        );
                        if ui
                            .button(RichText::new(label).color(Color32::LIGHT_RED))
                            .clicked()
                        {
                            edit(actions, path, Op::RemoveGroup { group: gi });
                            st.edit.confirm_delete_group = None;
                            st.edit.sel = None;
                            ui.close();
                        }
                    } else if ui
                        .button(RichText::new("Delete group…").color(Color32::LIGHT_RED))
                        .clicked()
                    {
                        st.edit.confirm_delete_group = Some(gi);
                    }
                },
            );
            if ui
                .small_button("+")
                .on_hover_text("Add a slide to this group")
                .clicked()
            {
                edit(
                    actions,
                    path,
                    Op::AddSlide {
                        group: gi,
                        at: group.slides.len(),
                        text: String::new(),
                    },
                );
                st.edit.select_slide(gi, group.slides.len());
            }
        });
    })
    .response
    .rect
}

// ───────────────────────────── inspector ─────────────────────────────

fn inspector(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
) {
    ui.horizontal(|ui| {
        // Effects and Adjust open from the toolbars (side panels with a Back button).
        ui.selectable_value(&mut st.edit.tab, Tab::Text, "Text");
        ui.selectable_value(&mut st.edit.tab, Tab::Image, "Image");
        ui.selectable_value(&mut st.edit.tab, Tab::Position, "Position");
        ui.selectable_value(&mut st.edit.tab, Tab::Layers, "Layers");
        ui.selectable_value(&mut st.edit.tab, Tab::Slide, "Slide");
    });
    ui.separator();
    let Some((g, s)) = st.edit.sel else {
        ui.label(RichText::new("No slide selected.").weak());
        return;
    };
    match st.edit.tab {
        Tab::Text => text_tab(ui, st, actions, path, d, g, s),
        Tab::Effects => {
            let e = st.edit.element.or_else(|| {
                d.groups[g].slides[s]
                    .elements
                    .iter()
                    .position(|e| matches!(e, Element::Text(_)))
            });
            match e {
                Some(e) => text::effects_tab(ui, st, actions, path, d, (g, s, e)),
                None => {
                    ui.label(RichText::new("This slide has no text box.").weak());
                }
            }
        }
        Tab::Image => {
            let e = st.edit.element.or_else(|| {
                d.groups[g].slides[s]
                    .elements
                    .iter()
                    .position(|e| matches!(e, Element::Image(_)))
            });
            image::image_tab(ui, st, actions, path, d, (g, s, e.unwrap_or(usize::MAX)));
        }
        Tab::Adjust => image::adjust_tab(ui, st, actions, path, d, (g, s)),
        Tab::Position => arrange::position_tab(ui, st, actions, path, d, (g, s)),
        Tab::Layers => arrange::layers_tab(ui, st, actions, path, d, (g, s)),
        Tab::Slide => slide_tab(ui, st, actions, path, d, g, s),
    }
}

fn text_tab(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    g: usize,
    s: usize,
) {
    let slide = &d.groups[g].slides[s];
    // No box selected: the slide's first text box, as before.
    let e = st.edit.element.or_else(|| {
        slide
            .elements
            .iter()
            .position(|e| matches!(e, Element::Text(_)))
    });
    let Some((e, t)) = e.and_then(|e| text_box(d, g, s, e).map(|t| (e, t))) else {
        ui.label(RichText::new("This slide has no text box.").weak());
        return;
    };
    let key = (path.to_path_buf(), slide.uuid.clone(), e);
    if st.edit.text_for.as_ref() != Some(&key) {
        st.edit.text = t.plain_text();
        st.edit.text_for = Some(key);
    }
    if let Some(problem) = &t.problem {
        let why = match problem {
            doc::TextProblem::Unreadable { message } => message.clone(),
            doc::TextProblem::FontNotInTable { .. } => {
                "Its text refers to a font that isn't listed in the slide.".to_owned()
            }
            doc::TextProblem::NoStyledText => "It has no font or size set.".to_owned(),
        };
        ui.label(
            RichText::new(format!("This text box can't be edited: {why}")).color(Color32::YELLOW),
        );
        return;
    }
    ui.label(
        RichText::new("Double-click the text on the canvas to edit it there, with styles.")
            .small()
            .weak(),
    );
    let mixed = {
        let mut runs = t.runs();
        let first = runs.next();
        first.is_some_and(|f| runs.any(|r| !r.same_style(f)))
    };
    if mixed {
        ui.label(
            RichText::new("This box mixes styles: text applied here takes the first line's style for all of it.")
                .small()
                .color(Color32::YELLOW),
        );
    }
    let r = ui.add(
        egui::TextEdit::multiline(&mut st.edit.text)
            .desired_rows(6)
            .desired_width(f32::INFINITY)
            .hint_text("Slide text (Enter = new line)"),
    );
    if std::mem::take(&mut st.edit.focus_text) {
        r.request_focus();
    }
    let ctrl_enter =
        r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
    let changed = st.edit.text.trim_end_matches('\n') != t.plain_text();
    ui.horizontal(|ui| {
        if (ui
            .add_enabled(changed, egui::Button::new("Apply text"))
            .on_hover_text("Ctrl+Enter")
            .clicked()
            || ctrl_enter)
            && changed
        {
            edit(
                actions,
                path,
                Op::SetBoxText {
                    group: g,
                    slide: s,
                    element: e,
                    text: st.edit.text.trim_end_matches('\n').to_owned(),
                },
            );
        }
        if changed && ui.button("Discard").clicked() {
            st.edit.text_for = None;
        }
    });
    ui.add_space(8.0);
    // Style summary (from the first run; rich text editing replaces this).
    if let Some(run) = t.runs().next() {
        let colour = run.color.map_or("default (black)".to_owned(), |[r, g, b]| {
            format!("#{r:02X}{g:02X}{b:02X}")
        });
        let mut style = Vec::new();
        if run.bold {
            style.push("bold");
        }
        if run.italic {
            style.push("italic");
        }
        if run.underline {
            style.push("underline");
        }
        egui::Grid::new("text-style").num_columns(2).show(ui, |ui| {
            ui.label(RichText::new("Font").weak());
            ui.label(&run.font);
            ui.end_row();
            ui.label(RichText::new("Size").weak());
            ui.label(format!("{:.0}", run.size));
            ui.end_row();
            ui.label(RichText::new("Colour").weak());
            ui.label(colour);
            ui.end_row();
            if !style.is_empty() {
                ui.label(RichText::new("Style").weak());
                ui.label(style.join(", "));
                ui.end_row();
            }
        });
        ui.label(
            RichText::new("New text takes the font, size and colour of the box's first line.")
                .small()
                .weak(),
        );
    }
}

fn slide_tab(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    g: usize,
    s: usize,
) {
    let slide = &d.groups[g].slides[s];
    ui.label(RichText::new(format!("Slide {} in {}", s + 1, d.groups[g].name)).strong());
    if !slide.label.is_empty() {
        ui.label(format!("Label: {}", slide.label));
    }
    let mut enabled = slide.enabled;
    if ui
        .checkbox(&mut enabled, "Enabled")
        .on_hover_text("Disabled slides are skipped by Next / Prev")
        .changed()
    {
        edit(
            actions,
            path,
            Op::SetSlideEnabled {
                group: g,
                slide: s,
                enabled,
            },
        );
    }
    ui.horizontal(|ui| {
        ui.label("Group");
        egui::ComboBox::from_id_salt("slide-group")
            .selected_text(d.groups[g].name.clone())
            .show_ui(ui, |ui| {
                for (i, other) in d.groups.iter().enumerate() {
                    if i != g && ui.selectable_label(false, &other.name).clicked() {
                        let to = other.slides.len();
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
                        st.edit.select_slide(i, to);
                    }
                }
            });
    });
    if let Some(t) = slide.timer {
        ui.label(format!(
            "Advances after {:.1} s{}",
            t.duration.as_secs_f64(),
            if t.loop_to_beginning {
                ", then back to the first slide"
            } else {
                ""
            }
        ));
    }
    ui.add_space(6.0);
    ui.label("Notes");
    sync_notes(st, path, slide);
    let r = ui.add(
        egui::TextEdit::multiline(&mut st.edit.notes)
            .desired_rows(4)
            .desired_width(f32::INFINITY),
    );
    if r.lost_focus() && st.edit.notes != slide.notes {
        edit(
            actions,
            path,
            Op::SetSlideNotes {
                group: g,
                slide: s,
                notes: st.edit.notes.clone(),
            },
        );
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| background_picker(ui, actions, path, d, g, s));
    ui.add_space(6.0);
    image::background_section(ui, st, actions, path, d, (g, s));
    if slide.pro6_incompatible {
        ui.add_space(6.0);
        ui.label(RichText::new("This slide uses something a ProPresenter 6 file can't hold; it only opens correctly in OpenPresent.").color(Color32::from_rgb(255, 130, 130)));
    }
}

// ───────────────────────────── canvas ─────────────────────────────

/// What the canvas needs to know about an element it can place.
#[derive(Clone, Copy)]
struct Item<'a> {
    frame: doc::Rect,
    rotation: f64,
    locked: bool,
    hidden: bool,
    uuid: &'a str,
    text: bool,
}

impl<'a> Item<'a> {
    fn of(e: &'a Element) -> Option<Item<'a>> {
        Some(Item {
            frame: e.frame()?,
            rotation: e.rotation(),
            locked: e.locked(),
            hidden: e.hidden(),
            uuid: e.uuid(),
            text: matches!(e, Element::Text(_)),
        })
    }
}

fn centre(f: &doc::Rect) -> Pos2 {
    Pos2::new((f.x + f.width / 2.0) as f32, (f.y + f.height / 2.0) as f32)
}

fn rotate(v: Vec2, degrees: f64) -> Vec2 {
    let (sin, cos) = (degrees as f32).to_radians().sin_cos();
    Vec2::new(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
}

/// `p` (canvas pixels) in the box's own unrotated axes, relative to its centre.
fn to_local(f: &doc::Rect, rotation: f64, p: Pos2) -> Vec2 {
    rotate(p - centre(f), -rotation)
}

fn hit(f: &doc::Rect, rotation: f64, p: Pos2) -> bool {
    let l = to_local(f, rotation, p);
    l.x.abs() <= f.width as f32 / 2.0 && l.y.abs() <= f.height as f32 / 2.0
}

/// A handle's position (canvas pixels): `(hx, hy)` in -1..=1 from the centre.
fn handle_pos(f: &doc::Rect, rotation: f64, hx: i8, hy: i8) -> Pos2 {
    centre(f)
        + rotate(
            Vec2::new(
                f32::from(hx) * f.width as f32 / 2.0,
                f32::from(hy) * f.height as f32 / 2.0,
            ),
            rotation,
        )
}

const HANDLES: [(i8, i8); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
];

/// Axis-aligned bounds of a (possibly rotated) box, canvas pixels.
fn aabb(f: &doc::Rect, rotation: f64) -> snap::Bounds {
    if rotation.rem_euclid(360.0) == 0.0 {
        return snap::Bounds::of(f);
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (hx, hy) in [(-1, -1), (1, -1), (1, 1), (-1, 1)] {
        let p = handle_pos(f, rotation, hx, hy);
        x0 = x0.min(f64::from(p.x));
        y0 = y0.min(f64::from(p.y));
        x1 = x1.max(f64::from(p.x));
        y1 = y1.max(f64::from(p.y));
    }
    snap::Bounds { x0, y0, x1, y1 }
}

fn bounds_rect(b: &snap::Bounds) -> doc::Rect {
    doc::Rect {
        x: b.x0,
        y: b.y0,
        width: b.x1 - b.x0,
        height: b.y1 - b.y0,
    }
}

/// `r` moved and scaled the way `from` became `to`.
fn map_rect(r: &doc::Rect, from: &doc::Rect, to: &doc::Rect) -> doc::Rect {
    let sx = if from.width > 0.0 {
        to.width / from.width
    } else {
        1.0
    };
    let sy = if from.height > 0.0 {
        to.height / from.height
    } else {
        1.0
    };
    doc::Rect {
        x: (to.x + (r.x - from.x) * sx).round(),
        y: (to.y + (r.y - from.y) * sy).round(),
        width: (r.width * sx).round().max(1.0),
        height: (r.height * sy).round().max(1.0),
    }
}

impl Drag {
    /// Each dragged element's frame and rotation as the drag now has them.
    fn member_frames(&self) -> Vec<(usize, doc::Rect, f64)> {
        if self.members.len() <= 1 {
            return vec![(self.element, self.now_frame, self.now_rotation)];
        }
        self.members
            .iter()
            .map(|(e, r, rot)| (*e, map_rect(r, &self.frame, &self.now_frame), *rot))
            .collect()
    }

    fn changed(&self) -> bool {
        self.now_frame != self.frame || self.now_rotation != self.rotation
    }
}

/// Where the drag would put the element(s), given the pointer (canvas pixels).
/// `snap`: the targets when snapping is on (and Alt isn't held); `one_axis`: Shift
/// while moving. Returns the snap guides to show.
fn drag_update(
    dr: &mut Drag,
    p: Pos2,
    snap: Option<&snap::Targets>,
    one_axis: bool,
) -> Vec<snap::Guide> {
    let f = dr.frame;
    let mut guides = Vec::new();
    match dr.grab {
        Grab::Move => {
            let mut delta = p - dr.start;
            if one_axis {
                if delta.x.abs() >= delta.y.abs() {
                    delta.y = 0.0;
                } else {
                    delta.x = 0.0;
                }
            }
            let mut nf = doc::Rect {
                x: (f.x + f64::from(delta.x)).round(),
                y: (f.y + f64::from(delta.y)).round(),
                ..f
            };
            if let Some(t) = snap {
                let (mut sx, mut sy, g) = snap::snap_move(&aabb(&nf, dr.rotation), t);
                let keep_x = !one_axis || delta.y == 0.0;
                let keep_y = !one_axis || delta.x == 0.0;
                if !keep_x {
                    sx = 0.0;
                }
                if !keep_y {
                    sy = 0.0;
                }
                nf.x += sx;
                nf.y += sy;
                guides =
                    g.into_iter()
                        .filter(|g| {
                            let axis = match g {
                                snap::Guide::Line { axis, .. }
                                | snap::Guide::Spacing { axis, .. } => *axis,
                            };
                            if axis == snap::Axis::X {
                                keep_x
                            } else {
                                keep_y
                            }
                        })
                        .collect();
            }
            dr.now_frame = nf;
        }
        Grab::Resize(hx, hy) => {
            // In the box's own axes: the grabbed edges follow the pointer, the
            // opposite edges stay put (also when rotated).
            let d = rotate(p - dr.start, -dr.rotation);
            let mut dw = f64::from(hx) * f64::from(d.x);
            let mut dh = f64::from(hy) * f64::from(d.y);
            dw = dw.max(MIN_SIZE - f.width).round();
            dh = dh.max(MIN_SIZE - f.height).round();
            if dr.keep_aspect && f.width > 0.0 && f.height > 0.0 {
                let ratio = f.width / f.height;
                let by_width = if hx != 0 && hy != 0 {
                    (dw / f.width).abs() >= (dh / f.height).abs()
                } else {
                    hx != 0
                };
                if by_width {
                    dh = ((f.width + dw) / ratio - f.height).round();
                } else {
                    dw = ((f.height + dh) * ratio - f.width).round();
                }
                dw = dw.max(MIN_SIZE - f.width);
                dh = dh.max(MIN_SIZE - f.height);
            }
            let shift = rotate(
                Vec2::new(
                    (f64::from(hx) * dw / 2.0) as f32,
                    (f64::from(hy) * dh / 2.0) as f32,
                ),
                dr.rotation,
            );
            let c = centre(&f) + shift;
            let (w, h) = (f.width + dw, f.height + dh);
            let mut nf = doc::Rect {
                x: (f64::from(c.x) - w / 2.0).round(),
                y: (f64::from(c.y) - h / 2.0).round(),
                width: w,
                height: h,
            };
            // Edges snap when the box is upright and free to change shape.
            if let Some(t) = snap
                && dr.rotation.rem_euclid(360.0) == 0.0
                && !dr.keep_aspect
            {
                let pts = |h: i8| match h {
                    1 => Some(snap::Points::High),
                    -1 => Some(snap::Points::Low),
                    _ => None,
                };
                let (sx, sy, g) = snap::snap_resize(&snap::Bounds::of(&nf), pts(hx), pts(hy), t);
                let mut s = nf;
                match hx {
                    1 => s.width += sx,
                    -1 => {
                        s.x += sx;
                        s.width -= sx;
                    }
                    _ => {}
                }
                match hy {
                    1 => s.height += sy,
                    -1 => {
                        s.y += sy;
                        s.height -= sy;
                    }
                    _ => {}
                }
                if s.width >= MIN_SIZE && s.height >= MIN_SIZE {
                    nf = s;
                    guides = g;
                }
            }
            dr.now_frame = nf;
        }
        Grab::Rotate => {
            let v = p - centre(&f);
            if v.length() > 1.0 {
                let deg = f64::from(v.y.atan2(v.x).to_degrees()) + 90.0;
                dr.now_rotation = deg.rem_euclid(360.0).round();
            }
        }
    }
    guides
}

/// Magenta, for snap guides (Canva-style).
const GUIDE: Color32 = Color32::from_rgb(255, 40, 200);
/// Grid spacing when the grid is on, canvas pixels.
const GRID: f64 = 40.0;

fn canvas(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &Arc<doc::Document>,
) {
    let full = ui.max_rect();
    let area = Rect::from_min_max(full.min + Vec2::splat(RULER), full.max);
    let (cw, ch) = (d.canvas.width.max(1) as f32, d.canvas.height.max(1) as f32);
    let fit = ((area.width() - 32.0) / cw)
        .min((area.height() - 32.0) / ch)
        .max(0.02);
    let scale = match st.edit.zoom {
        Zoom::Fit => fit,
        Zoom::Pct(p) => p as f32 / 100.0,
    };
    let size = Vec2::new(cw, ch) * scale;
    let resp = ui.interact(
        area,
        ui.scope_id().with("editor-canvas"),
        Sense::click_and_drag(),
    );
    if st.edit.zoom != Zoom::Fit && resp.hovered() {
        st.edit.pan += ui.input(|i| i.smooth_scroll_delta);
    }
    let max_pan = ((size - area.size()) / 2.0 + Vec2::splat(16.0)).max(Vec2::ZERO);
    st.edit.pan = st.edit.pan.clamp(-max_pan, max_pan);
    let rect = Rect::from_center_size(area.center() + st.edit.pan, size);
    st.edit.view = Some((rect, scale));
    let painter = ui.painter_at(area);

    let Some((g, s)) = st.edit.sel else {
        painter.text(
            area.center(),
            Align2::CENTER_CENTER,
            "No slides",
            FontId::proportional(16.0),
            Color32::GRAY,
        );
        return;
    };
    let slide = &d.groups[g].slides[s];
    let to_screen = |p: Pos2| rect.min + p.to_vec2() * scale;
    let to_canvas = |p: Pos2| Pos2::ZERO + (p - rect.min) / scale;

    let ppp = ui.ctx().pixels_per_point();
    let now = ui.input(|i| i.time);
    if let Some(sess) = st.edit.session.as_mut() {
        sess.scale = scale * ppp;
    }

    // Text being edited owns the pointer inside its box: place the caret, select.
    let sess_box = st
        .edit
        .session
        .as_ref()
        .filter(|x| x.g == g && x.s == s)
        .and_then(|x| text_box(d, g, s, x.e).map(|t| (t.frame, t.rotation)));
    let local_of = |f: &doc::Rect, r: f64, p: Pos2| {
        let l = to_local(f, r, p);
        (l.x + f.width as f32 / 2.0, l.y + f.height as f32 / 2.0)
    };
    let (pressed, down, shift, alt) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_down(),
            i.modifiers.shift,
            i.modifiers.alt,
        )
    });
    let mut session_owns = st.edit.session_drag;
    if let Some((f, r)) = sess_box
        && let Some(p) = resp.hover_pos().map(to_canvas)
    {
        if pressed {
            if hit(&f, r, p) {
                text::pointer(
                    st,
                    d,
                    local_of(&f, r, p),
                    text::Mouse::Press { extend: shift },
                    now,
                );
                st.edit.session_drag = true;
                session_owns = true;
            } else {
                text::commit(st, actions, path, d);
            }
        } else if st.edit.session_drag && down {
            text::pointer(st, d, local_of(&f, r, p), text::Mouse::Drag, now);
        }
        if resp.double_clicked() && hit(&f, r, p) {
            text::pointer(st, d, local_of(&f, r, p), text::Mouse::Double, now);
            session_owns = true;
        }
    }
    if !down {
        st.edit.session_drag = false;
    }

    // The image: the editor copy, the text being edited, or a drag in progress.
    let session_preview = st
        .edit
        .session
        .as_mut()
        .filter(|x| x.g == g && x.s == s)
        .map(|x| text::preview(x, d));
    let shown = match st.edit.drag.as_mut() {
        _ if session_preview.is_some() => session_preview.clone().unwrap_or_else(|| d.clone()),
        Some(dr) if dr.changed() => {
            if dr
                .preview
                .as_ref()
                .is_none_or(|(f, r, _)| *f != dr.now_frame || *r != dr.now_rotation)
            {
                let mut p = (**d).clone();
                for (e, f, r) in dr.member_frames() {
                    match p.groups[g].slides[s].elements.get_mut(e) {
                        Some(Element::Text(t)) => {
                            t.frame = f;
                            t.rotation = r;
                        }
                        Some(Element::Image(i)) => {
                            i.frame = f;
                            i.rotation = r;
                        }
                        _ => {}
                    }
                }
                dr.preview = Some((dr.now_frame, dr.now_rotation, Arc::new(p)));
            }
            dr.preview
                .as_ref()
                .map_or_else(|| d.clone(), |(_, _, p)| p.clone())
        }
        _ => d.clone(),
    };
    painter.rect_filled(rect, 0.0, Color32::BLACK);
    if let Some(c) = &mut st.canvas {
        c.request(
            &shown,
            flat_index(d, g, s),
            (size.x * ppp).round() as u32,
            (size.y * ppp).round() as u32,
        );
        let (tex, err) = c.image();
        if let Some(t) = tex {
            painter.image(
                t.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        if let Some(e) = err {
            let band = Rect::from_min_size(
                rect.left_bottom() - Vec2::new(0.0, 40.0),
                Vec2::new(rect.width(), 40.0),
            );
            painter.rect_filled(band, 0.0, Color32::from_rgba_unmultiplied(90, 10, 10, 220));
            painter.text(
                band.center(),
                Align2::CENTER_CENTER,
                e,
                FontId::proportional(13.0),
                Color32::WHITE,
            );
        }
    }

    // Grid and safe area (bottom bar toggles).
    if st.edit.grid {
        let step = GRID as f32 * scale;
        if step >= 4.0 {
            let stroke = Stroke::new(1.0, Color32::from_white_alpha(28));
            let mut x = rect.left() + step;
            while x < rect.right() {
                painter.line_segment(
                    [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                    stroke,
                );
                x += step;
            }
            let mut y = rect.top() + step;
            while y < rect.bottom() {
                painter.line_segment(
                    [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                    stroke,
                );
                y += step;
            }
        }
    }
    if st.edit.safe_area {
        let inset = Vec2::new(cw, ch) * snap::SAFE_INSET as f32 * scale;
        painter.rect_stroke(
            rect.shrink2(inset),
            0.0,
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 220, 80, 150)),
            StrokeKind::Middle,
        );
    }

    // Selection (primary first).
    let selection: Vec<usize> = st.edit.selection();
    // Element geometry as shown (dragged elements at their new place).
    let dragged: Vec<(usize, doc::Rect, f64)> = st
        .edit
        .drag
        .as_ref()
        .map(Drag::member_frames)
        .unwrap_or_default();
    let geom = |e: usize, t: &Item<'_>| {
        dragged
            .iter()
            .find(|(de, _, _)| *de == e)
            .map_or((t.frame, t.rotation), |(_, f, r)| (*f, *r))
    };
    let all: Vec<(usize, Item<'_>)> = slide
        .elements
        .iter()
        .enumerate()
        .filter_map(|(i, e)| Item::of(e).map(|it| (i, it)))
        .collect();
    // Hidden elements can't be clicked on the canvas (select them in Layers).
    let boxes: Vec<(usize, Item<'_>)> = all.iter().copied().filter(|(_, t)| !t.hidden).collect();
    let pointer = resp.hover_pos().map(to_canvas);
    let topmost_at = |p: Pos2| {
        boxes
            .iter()
            .rev()
            .find(|(e, t)| {
                let (f, r) = geom(*e, t);
                hit(&f, r, p)
            })
            .map(|(e, _)| *e)
    };
    let hovered = if st.edit.drag.is_none() && st.edit.marquee.is_none() {
        pointer.and_then(topmost_at)
    } else {
        None
    };
    let item = |e: usize| all.iter().find(|(i, _)| *i == e).map(|(_, t)| *t);
    let multi = selection.len() > 1;
    // The selection's bounds when several are selected (group handles).
    let group_bounds = multi
        .then(|| {
            selection
                .iter()
                .filter_map(|&e| item(e).map(|t| aabb(&geom(e, &t).0, geom(e, &t).1)))
                .reduce(|a, b| a.union(&b))
        })
        .flatten();

    // What a press at `p` (screen) would grab on the selection.
    let grab_at = |p: Pos2| -> Option<Grab> {
        if multi {
            let b = bounds_rect(&group_bounds?);
            for (hx, hy) in HANDLES {
                if p.distance(to_screen(handle_pos(&b, 0.0, hx, hy))) <= HANDLE + 3.0 {
                    return Some(Grab::Resize(hx, hy));
                }
            }
            let c = to_canvas(p);
            return selection
                .iter()
                .filter_map(|&e| item(e))
                .any(|t| hit(&t.frame, t.rotation, c))
                .then_some(Grab::Move);
        }
        let t = item(*selection.first()?)?;
        if t.locked {
            return None;
        }
        let (f, r) = (t.frame, t.rotation);
        let top = to_screen(handle_pos(&f, r, 0, -1));
        let arm = top + rotate(Vec2::new(0.0, -ROTATE_ARM), r);
        if p.distance(arm) <= HANDLE + 3.0 {
            return Some(Grab::Rotate);
        }
        for (hx, hy) in HANDLES {
            if p.distance(to_screen(handle_pos(&f, r, hx, hy))) <= HANDLE + 3.0 {
                return Some(Grab::Resize(hx, hy));
            }
        }
        hit(&f, r, to_canvas(p)).then_some(Grab::Move)
    };

    // Outlines and handles.
    for (e, t) in &all {
        let selected = selection.contains(e);
        if t.hidden && !selected {
            continue;
        }
        let (f, r) = geom(*e, t);
        let pts: Vec<Pos2> = [(-1, -1), (1, -1), (1, 1), (-1, 1)]
            .iter()
            .map(|&(hx, hy)| to_screen(handle_pos(&f, r, hx, hy)))
            .collect();
        let stroke = if selected {
            Stroke::new(1.5, SELECT)
        } else if hovered == Some(*e) {
            Stroke::new(1.0, Color32::from_rgb(150, 190, 255))
        } else {
            Stroke::new(1.0, Color32::from_white_alpha(45))
        };
        if t.hidden {
            let mut closed = pts.clone();
            closed.push(pts[0]);
            painter.add(egui::Shape::dashed_line(&closed, stroke, 5.0, 4.0));
            painter.text(
                pts[0] - Vec2::new(0.0, 4.0),
                Align2::LEFT_BOTTOM,
                "hidden",
                FontId::proportional(11.0),
                Color32::LIGHT_GRAY,
            );
        } else {
            painter.add(egui::Shape::closed_line(pts.clone(), stroke));
        }
        // Text taller than its box: a red badge (the strip tile gets one too).
        if st
            .canvas
            .as_ref()
            .is_some_and(|c| c.overflow().iter().any(|u| u == t.uuid))
        {
            let at = pts[3];
            let badge = Rect::from_min_size(at + Vec2::new(0.0, 2.0), Vec2::new(118.0, 18.0));
            painter.rect_filled(badge, 3.0, Color32::from_rgb(190, 30, 30));
            painter.text(
                badge.center(),
                Align2::CENTER_CENTER,
                "Text doesn't fit",
                FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
        // A picture that can't be shown: say so where it would be.
        if !t.text
            && let Some(msg) = st.canvas.as_ref().and_then(|c| c.problem(t.uuid))
        {
            let r = Rect::from_points(&pts);
            painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(80, 20, 20, 160));
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                if msg.contains("wasn't found") {
                    "Image not found"
                } else {
                    "Image can't be shown"
                },
                FontId::proportional(12.0),
                Color32::WHITE,
            );
        }
        if selected && !multi && !t.locked {
            let top = to_screen(handle_pos(&f, r, 0, -1));
            let arm = top + rotate(Vec2::new(0.0, -ROTATE_ARM), r);
            painter.line_segment([top, arm], Stroke::new(1.0, SELECT));
            painter.circle(arm, HANDLE, Color32::WHITE, Stroke::new(1.0, SELECT));
            for (hx, hy) in HANDLES {
                let c = to_screen(handle_pos(&f, r, hx, hy));
                painter.rect(
                    Rect::from_center_size(c, Vec2::splat(HANDLE * 2.0)),
                    1.0,
                    Color32::WHITE,
                    Stroke::new(1.0, SELECT),
                    StrokeKind::Middle,
                );
            }
        }
        if selected && t.locked {
            painter.text(
                pts[0] - Vec2::new(0.0, 4.0),
                Align2::LEFT_BOTTOM,
                "locked",
                FontId::proportional(11.0),
                Color32::YELLOW,
            );
        }
    }
    if let Some(b) = group_bounds {
        let b = bounds_rect(&b);
        let r = Rect::from_min_max(
            to_screen(Pos2::new(b.x as f32, b.y as f32)),
            to_screen(Pos2::new((b.x + b.width) as f32, (b.y + b.height) as f32)),
        );
        let pts = [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
            r.left_top(),
        ];
        painter.add(egui::Shape::dashed_line(
            &pts,
            Stroke::new(1.0, SELECT),
            6.0,
            4.0,
        ));
        for (hx, hy) in HANDLES {
            let c = to_screen(handle_pos(&b, 0.0, hx, hy));
            painter.rect(
                Rect::from_center_size(c, Vec2::splat(HANDLE * 2.0)),
                1.0,
                Color32::WHITE,
                Stroke::new(1.0, SELECT),
                StrokeKind::Middle,
            );
        }
    }
    if let Some(dr) = &st.edit.drag
        && dr.grab == Grab::Rotate
        && let Some(p) = resp.hover_pos()
    {
        painter.text(
            p + Vec2::new(14.0, -14.0),
            Align2::LEFT_BOTTOM,
            format!("{:.0}°", dr.now_rotation),
            FontId::proportional(13.0),
            Color32::WHITE,
        );
    }
    // Snap guides (only while dragging).
    if st.edit.drag.is_some() && !st.edit.hide_guides {
        paint_guides(&painter, &st.edit.guides, &to_screen);
    }
    // Marquee.
    if let Some((a, b)) = st.edit.marquee {
        let r = Rect::from_two_pos(to_screen(a), to_screen(b));
        painter.rect(
            r,
            0.0,
            Color32::from_rgba_unmultiplied(90, 160, 255, 30),
            Stroke::new(1.0, SELECT),
            StrokeKind::Middle,
        );
    }

    text::paint(ui, &painter, st, d, &to_screen);

    // Cursor.
    if let Some((f, r)) = sess_box
        && let Some(p) = resp.hover_pos()
        && hit(&f, r, to_canvas(p))
    {
        ui.ctx().set_cursor_icon(CursorIcon::Text);
    } else if let Some(p) = resp.hover_pos() {
        let icon = match st.edit.drag.as_ref().map(|d| d.grab).or_else(|| grab_at(p)) {
            Some(Grab::Move) => Some(CursorIcon::Move),
            Some(Grab::Rotate) => Some(CursorIcon::Grab),
            Some(Grab::Resize(hx, hy)) => Some(match (hx, hy) {
                (0, _) => CursorIcon::ResizeVertical,
                (_, 0) => CursorIcon::ResizeHorizontal,
                (x, y) if x == y => CursorIcon::ResizeNwSe,
                _ => CursorIcon::ResizeNeSw,
            }),
            None => hovered.map(|_| CursorIcon::PointingHand),
        };
        if let Some(i) = icon {
            ui.ctx().set_cursor_icon(i);
        }
    }

    // Press / drag / release.
    if resp.drag_started()
        && !session_owns
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        let grab = grab_at(origin).or_else(|| {
            // An element that isn't selected: select it (Shift adds it) and move.
            let e = topmost_at(to_canvas(origin))?;
            if shift {
                st.edit.toggle(e);
            } else {
                st.edit.element = Some(e);
                st.edit.multi.clear();
            }
            Some(Grab::Move)
        });
        let members: Vec<(usize, doc::Rect, f64)> = st
            .edit
            .selection()
            .into_iter()
            .filter_map(|e| {
                item(e)
                    .filter(|t| !t.locked)
                    .map(|t| (e, t.frame, t.rotation))
            })
            .collect();
        match grab {
            Some(grab) if !members.is_empty() => {
                let single = members.len() == 1;
                let (frame, rotation) = if single {
                    (members[0].1, members[0].2)
                } else {
                    let b = members
                        .iter()
                        .map(|(_, f, r)| aabb(f, *r))
                        .reduce(|a, b| a.union(&b))
                        .unwrap_or(snap::Bounds::of(&members[0].1));
                    (bounds_rect(&b), 0.0)
                };
                if single || grab != Grab::Rotate {
                    let text = single && item(members[0].0).is_some_and(|t| t.text);
                    st.edit.drag = Some(Drag {
                        grab,
                        element: members[0].0,
                        start: to_canvas(origin),
                        frame,
                        rotation,
                        now_frame: frame,
                        now_rotation: rotation,
                        keep_aspect: !text,
                        members,
                        preview: None,
                    });
                }
            }
            Some(_) => {}
            None => {
                // Empty canvas: a marquee.
                let c = to_canvas(origin);
                st.edit.marquee = Some((c, c));
            }
        }
    }
    if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
        if let Some(dr) = st.edit.drag.as_mut() {
            let text = dr.members.len() == 1 && item(dr.element).is_some_and(|t| t.text);
            // Images and groups keep their shape unless Shift is held; text boxes
            // the other way round.
            dr.keep_aspect = shift ^ !text;
            let targets = (!st.edit.no_snap && !alt).then(|| {
                let others: Vec<snap::Bounds> = boxes
                    .iter()
                    .filter(|(e, _)| !dr.members.iter().any(|(m, _, _)| m == e))
                    .map(|(_, t)| aabb(&t.frame, t.rotation))
                    .collect();
                snap::Targets::new(
                    (f64::from(cw), f64::from(ch)),
                    others,
                    st.edit.grid.then_some(GRID),
                    scale,
                )
            });
            let one_axis = shift && dr.grab == Grab::Move;
            st.edit.guides = drag_update(dr, to_canvas(p), targets.as_ref(), one_axis);
        }
        if let Some((_, b)) = st.edit.marquee.as_mut() {
            *b = to_canvas(p);
        }
    }
    if resp.drag_stopped() {
        st.edit.guides.clear();
        if let Some(dr) = st.edit.drag.take()
            && dr.changed()
        {
            if dr.members.len() == 1 {
                if dr.now_frame != dr.frame {
                    edit(
                        actions,
                        path,
                        Op::SetFrame {
                            group: g,
                            slide: s,
                            element: dr.element,
                            frame: dr.now_frame,
                        },
                    );
                }
                if dr.now_rotation != dr.rotation {
                    edit(
                        actions,
                        path,
                        Op::SetRotation {
                            group: g,
                            slide: s,
                            element: dr.element,
                            degrees: dr.now_rotation,
                        },
                    );
                }
            } else {
                let edits = dr
                    .member_frames()
                    .into_iter()
                    .map(|(e, frame, _)| Op::SetFrame {
                        group: g,
                        slide: s,
                        element: e,
                        frame,
                    })
                    .collect();
                edit(
                    actions,
                    path,
                    Op::Batch {
                        label: if dr.grab == Grab::Move {
                            "Move"
                        } else {
                            "Resize"
                        }
                        .into(),
                        edits,
                    },
                );
            }
        }
        if let Some((a, b)) = st.edit.marquee.take() {
            let m = snap::Bounds {
                x0: f64::from(a.x.min(b.x)),
                y0: f64::from(a.y.min(b.y)),
                x1: f64::from(a.x.max(b.x)),
                y1: f64::from(a.y.max(b.y)),
            };
            let picked: Vec<usize> = boxes
                .iter()
                .filter(|(_, t)| {
                    let b = aabb(&t.frame, t.rotation);
                    b.x0 < m.x1 && b.x1 > m.x0 && b.y0 < m.y1 && b.y1 > m.y0
                })
                .map(|(e, _)| *e)
                .collect();
            if !shift {
                st.edit.element = None;
                st.edit.multi.clear();
            }
            for e in picked {
                if !st.edit.selection().contains(&e) {
                    st.edit.toggle(e);
                }
            }
        }
    }
    // (While editing text, the canvas has keyboard focus and egui reports Enter /
    // Space as clicks; the session handles the pointer itself.)
    if resp.clicked() && !session_owns && st.edit.session.is_none() {
        let e = resp
            .interact_pointer_pos()
            .map(to_canvas)
            .and_then(topmost_at);
        match e {
            Some(e) if shift => st.edit.toggle(e),
            _ => {
                st.edit.element = e;
                st.edit.multi.clear();
            }
        }
    }
    // Double-click a text box: edit its text in place.
    if resp.double_clicked()
        && !session_owns
        && let Some(p) = resp.interact_pointer_pos().map(to_canvas)
        && let Some(e) = topmost_at(p)
        && let Some(t) = item(e)
        && t.text
    {
        st.edit.drag = None;
        st.edit.multi.clear();
        let at = local_of(&t.frame, t.rotation, p);
        text::begin(st, d, (g, s, e), Some(at), scale * ppp, now);
    }

    // Keys go to the text being edited (focus stays on the canvas while it is).
    if st.edit.session.is_some() {
        let id = resp.id;
        let focused = ui.memory(|m| m.focused());
        if focused.is_none() || focused == Some(id) {
            ui.memory_mut(|m| {
                m.request_focus(id);
                m.set_focus_lock_filter(
                    id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                );
            });
            text::keys(ui, st, actions, path, d);
        }
    }

    let keys_free = st.edit.session.is_none() && ui.memory(|m| m.focused().is_none());
    // Tab / Shift+Tab: select the next / previous element.
    if keys_free && let Some(back) = st.edit.tab_pressed.take() {
        let order: Vec<usize> = boxes.iter().map(|(e, _)| *e).collect();
        if !order.is_empty() {
            let at = st
                .edit
                .element
                .and_then(|e| order.iter().position(|o| *o == e));
            let next = match (at, back) {
                (None, false) => 0,
                (None, true) => order.len() - 1,
                (Some(i), false) => (i + 1) % order.len(),
                (Some(i), true) => (i + order.len() - 1) % order.len(),
            };
            st.edit.element = Some(order[next]);
            st.edit.multi.clear();
        }
    }
    st.edit.tab_pressed = None;

    // Keyboard: arrows nudge (1 px, Shift 10 px), Enter edits the text, Delete
    // removes, Esc deselects. Never while typing.
    if keys_free
        && !multi
        && ui.input(|i| i.key_pressed(egui::Key::Enter))
        && let Some(e) = st.edit.element
        && item(e).is_some_and(|t| t.text)
    {
        text::begin(st, d, (g, s, e), None, scale * ppp, now);
    }
    let selection = st.edit.selection();
    if keys_free && !selection.is_empty() {
        let (dx, dy, esc, delete) = ui.input(|i| {
            let step = if i.modifiers.shift { 10.0 } else { 1.0 };
            let k = |key| if i.key_pressed(key) { step } else { 0.0 };
            (
                k(egui::Key::ArrowRight) - k(egui::Key::ArrowLeft),
                k(egui::Key::ArrowDown) - k(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
            )
        });
        if delete && st.edit.drag.is_none() {
            let mut sel = selection.clone();
            sel.sort_unstable_by(|a, b| b.cmp(a));
            let mut edits: Vec<Op> = sel
                .into_iter()
                .map(|e| Op::RemoveElement {
                    group: g,
                    slide: s,
                    element: e,
                })
                .collect();
            let op = if edits.len() == 1 {
                edits.remove(0)
            } else {
                Op::Batch {
                    label: "Delete".into(),
                    edits,
                }
            };
            text::send_checked(st, actions, path, d, op, None);
            st.edit.element = None;
            st.edit.multi.clear();
            return;
        }
        if dx != 0.0 || dy != 0.0 {
            let moved: Vec<(usize, Item<'_>)> = selection
                .iter()
                .filter_map(|&e| item(e).filter(|t| !t.locked).map(|t| (e, t)))
                .collect();
            let mut edits: Vec<Op> = moved
                .iter()
                .map(|(e, t)| Op::SetFrame {
                    group: g,
                    slide: s,
                    element: *e,
                    frame: doc::Rect {
                        x: t.frame.x + dx,
                        y: t.frame.y + dy,
                        ..t.frame
                    },
                })
                .collect();
            let key = format!(
                "nudge:{}:{}",
                slide.uuid,
                moved
                    .iter()
                    .map(|(_, t)| t.uuid)
                    .collect::<Vec<_>>()
                    .join(",")
            );
            match edits.len() {
                0 => {}
                1 => edit_merging(actions, path, edits.remove(0), key),
                _ => edit_merging(
                    actions,
                    path,
                    Op::Batch {
                        label: "Move".into(),
                        edits,
                    },
                    key,
                ),
            }
        }
        if esc {
            st.edit.element = None;
            st.edit.multi.clear();
        }
    }

    // Rulers, with the selection's extent marked.
    let extent = selection
        .iter()
        .filter_map(|&e| {
            item(e).map(|t| {
                let (f, r) = geom(e, &t);
                aabb(&f, r)
            })
        })
        .reduce(|a, b| a.union(&b))
        .map(|b| (b.x0 as f32, b.x1 as f32, b.y0 as f32, b.y1 as f32));
    rulers(ui, full, area, rect, scale, extent);
}

fn paint_guides(painter: &egui::Painter, guides: &[snap::Guide], to_screen: &dyn Fn(Pos2) -> Pos2) {
    let stroke = Stroke::new(1.0, GUIDE);
    let pt = |axis: snap::Axis, along: f64, cross: f64| match axis {
        snap::Axis::X => to_screen(Pos2::new(along as f32, cross as f32)),
        snap::Axis::Y => to_screen(Pos2::new(cross as f32, along as f32)),
    };
    for g in guides {
        match g {
            snap::Guide::Line {
                axis,
                pos,
                from,
                to,
                ..
            } => {
                painter.line_segment([pt(*axis, *pos, *from), pt(*axis, *pos, *to)], stroke);
            }
            snap::Guide::Spacing {
                axis,
                gaps,
                cross,
                gap,
            } => {
                for (a, b) in gaps {
                    // Measured along the axis, at `cross` on the other one.
                    let (p, q) = match axis {
                        snap::Axis::X => (
                            to_screen(Pos2::new(*a as f32, *cross as f32)),
                            to_screen(Pos2::new(*b as f32, *cross as f32)),
                        ),
                        snap::Axis::Y => (
                            to_screen(Pos2::new(*cross as f32, *a as f32)),
                            to_screen(Pos2::new(*cross as f32, *b as f32)),
                        ),
                    };
                    painter.line_segment([p, q], stroke);
                    let tick = match axis {
                        snap::Axis::X => Vec2::new(0.0, 4.0),
                        snap::Axis::Y => Vec2::new(4.0, 0.0),
                    };
                    painter.line_segment([p - tick, p + tick], stroke);
                    painter.line_segment([q - tick, q + tick], stroke);
                    let mid = p + (q - p) / 2.0;
                    let label = format!("{gap:.0}");
                    let r = Rect::from_center_size(
                        mid - tick * 3.0,
                        Vec2::new(8.0 + 7.0 * label.len() as f32, 16.0),
                    );
                    painter.rect_filled(r, 3.0, GUIDE);
                    painter.text(
                        r.center(),
                        Align2::CENTER_CENTER,
                        label,
                        FontId::proportional(11.0),
                        Color32::WHITE,
                    );
                }
            }
        }
    }
}

fn rulers(
    ui: &egui::Ui,
    full: Rect,
    area: Rect,
    canvas: Rect,
    scale: f32,
    extent: Option<(f32, f32, f32, f32)>,
) {
    let p = ui.painter_at(full);
    let bg = Color32::from_gray(36);
    let top = Rect::from_min_max(
        Pos2::new(area.left(), full.top()),
        Pos2::new(full.right(), area.top()),
    );
    let left = Rect::from_min_max(
        Pos2::new(full.left(), area.top()),
        Pos2::new(area.left(), full.bottom()),
    );
    p.rect_filled(top, 0.0, bg);
    p.rect_filled(left, 0.0, bg);
    p.rect_filled(Rect::from_min_max(full.min, area.min), 0.0, bg);
    if let Some((x0, x1, y0, y1)) = extent {
        let mark = Color32::from_rgba_unmultiplied(90, 160, 255, 70);
        p.rect_filled(
            Rect::from_x_y_ranges(
                canvas.left() + x0 * scale..=canvas.left() + x1 * scale,
                top.y_range(),
            ),
            0.0,
            mark,
        );
        p.rect_filled(
            Rect::from_x_y_ranges(
                left.x_range(),
                canvas.top() + y0 * scale..=canvas.top() + y1 * scale,
            ),
            0.0,
            mark,
        );
    }
    // Major ticks at least ~70 points apart, labelled in canvas pixels.
    let major = [10.0, 25.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1000.0, 2000.0]
        .into_iter()
        .find(|s| s * scale >= 70.0)
        .unwrap_or(5000.0);
    let minor = major / 5.0;
    let tick = Color32::from_gray(120);
    let font = FontId::proportional(9.0);
    let range = |from: f32, to: f32, origin: f32| {
        let a = ((from - origin) / scale / minor).floor() as i64;
        let b = ((to - origin) / scale / minor).ceil() as i64;
        a..=b
    };
    for i in range(top.left(), top.right(), canvas.left()) {
        let v = i as f32 * minor;
        let x = canvas.left() + v * scale;
        if x < top.left() || x > top.right() {
            continue;
        }
        let is_major = (v / major).fract().abs() < 1e-4;
        let len = if is_major { RULER - 4.0 } else { 4.0 };
        p.line_segment(
            [Pos2::new(x, top.bottom() - len), Pos2::new(x, top.bottom())],
            Stroke::new(1.0, tick),
        );
        if is_major {
            p.text(
                Pos2::new(x + 2.0, top.top() + 1.0),
                Align2::LEFT_TOP,
                format!("{v:.0}"),
                font.clone(),
                tick,
            );
        }
    }
    for i in range(left.top(), left.bottom(), canvas.top()) {
        let v = i as f32 * minor;
        let y = canvas.top() + v * scale;
        if y < left.top() || y > left.bottom() {
            continue;
        }
        let is_major = (v / major).fract().abs() < 1e-4;
        let len = if is_major { RULER - 4.0 } else { 4.0 };
        p.line_segment(
            [Pos2::new(left.right() - len, y), Pos2::new(left.right(), y)],
            Stroke::new(1.0, tick),
        );
        if is_major {
            p.text(
                Pos2::new(left.left() + 1.0, y + 2.0),
                Align2::LEFT_TOP,
                format!("{v:.0}"),
                font.clone(),
                tick,
            );
        }
    }
}

// ───────────────────────────── arrangements ─────────────────────────────

fn arrangements(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    doc: &doc::Document,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The editor run headless: a plain egui context, synthetic pointer input.
    struct Harness {
        ctx: egui::Context,
        st: UiState,
        show: ShowState,
        path: PathBuf,
        doc: Arc<doc::Document>,
        thumbs: Sender<ThumbRequest>,
        _thumbs_rx: crossbeam_channel::Receiver<ThumbRequest>,
        time: f64,
        mods: egui::Modifiers,
    }

    impl Harness {
        fn new() -> Harness {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/render_1024x768.pro6");
            let doc = Arc::new(doc::load(&path).unwrap());
            let mut show = ShowState {
                selected: Some(path.clone()),
                ..Default::default()
            };
            Arc::make_mut(&mut show.docs).insert(path.clone(), doc.clone());
            let mut st = UiState::default();
            st.edit.select(0, 0);
            let fonts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts");
            st.fonts = Some(render::Fonts::from_dirs(&[&fonts]));
            let (thumbs, _thumbs_rx) = crossbeam_channel::unbounded();
            Harness {
                ctx: egui::Context::default(),
                st,
                show,
                path,
                doc,
                thumbs,
                _thumbs_rx,
                time: 0.0,
                mods: Default::default(),
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> Vec<Op> {
            self.time += 0.05;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))),
                time: Some(self.time),
                events: std::iter::once(egui::Event::ModifiersChanged(self.mods))
                    .chain(events)
                    .collect(),
                ..Default::default()
            };
            let mut actions = Vec::new();
            let (st, show, thumbs, path, d) = (
                &mut self.st,
                &self.show,
                &self.thumbs,
                &self.path,
                &self.doc,
            );
            let mut out = self.ctx.run_ui(input, |ui| {
                editor(ui, st, show, thumbs, &mut actions, path, d);
            });
            out.textures_delta.clear();
            actions
                .into_iter()
                .filter_map(|a| match a {
                    UiAction::Core(
                        Command::Edit { edit, .. } | Command::EditMerging { edit, .. },
                    ) => Some(edit),
                    _ => None,
                })
                .collect()
        }

        /// Screen position of canvas pixel `(x, y)`.
        fn at(&self, x: f32, y: f32) -> Pos2 {
            let (rect, scale) = self.st.edit.view.unwrap();
            rect.min + Vec2::new(x, y) * scale
        }

        fn scale(&self) -> f32 {
            self.st.edit.view.unwrap().1
        }

        /// Press at `from`, move to `to` in steps, release; edits sent.
        fn drag(&mut self, from: Pos2, to: Pos2) -> Vec<Op> {
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            let mut out = self.frame(vec![egui::Event::PointerMoved(from)]);
            out.extend(self.frame(vec![button(from, true)]));
            for i in 1..=5 {
                let p = from + (to - from) * (i as f32 / 5.0);
                out.extend(self.frame(vec![egui::Event::PointerMoved(p)]));
            }
            out.extend(self.frame(vec![button(to, false)]));
            out.extend(self.frame(vec![]));
            out
        }

        fn click(&mut self, at: Pos2) -> Vec<Op> {
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            let mut out = self.frame(vec![egui::Event::PointerMoved(at)]);
            out.extend(self.frame(vec![button(true)]));
            out.extend(self.frame(vec![button(false)]));
            out
        }
    }

    #[test]
    fn images_select_resize_at_their_shape_and_delete() {
        let mut h = Harness::new();
        let mut d = (*h.doc).clone();
        d.groups[0].slides[0]
            .elements
            .push(Element::Image(Box::new(doc::ImageBox {
                uuid: new_uuid(),
                name: "pic".into(),
                frame: doc::Rect {
                    x: 920.0,
                    y: 690.0,
                    width: 80.0,
                    height: 40.0,
                },
                rotation: 0.0,
                opacity: 1.0,
                locked: false,
                hidden: false,
                flip_h: false,
                flip_v: false,
                media: doc::MediaRef {
                    path: "Media/none.png".into(),
                    sha256: None,
                },
                adjust: doc::Adjust::default(),
                src: None,
            })));
        h.doc = Arc::new(d);
        h.frame(vec![]);
        h.click(h.at(960.0, 710.0));
        assert_eq!(h.st.edit.element, Some(1), "the image got selected");
        // Bottom-right handle dragged right and a little down: 2:1 is kept.
        let edits = h.drag(
            h.at(1000.0, 730.0),
            h.at(1000.0, 730.0) + Vec2::new(10.0, 2.0),
        );
        match edits.as_slice() {
            [
                Op::SetFrame {
                    element: 1, frame, ..
                },
            ] => {
                assert_eq!((frame.x, frame.y), (920.0, 690.0));
                assert!(frame.width > 80.0);
                assert!((frame.width / frame.height - 2.0).abs() < 0.06, "{frame:?}");
            }
            other => panic!("expected a resize, got {other:?}"),
        }
        let edits = h.frame(vec![egui::Event::Key {
            key: egui::Key::Delete,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }]);
        assert_eq!(
            edits,
            vec![Op::RemoveElement {
                group: 0,
                slide: 0,
                element: 1
            }]
        );
        assert_eq!(h.st.edit.element, None);
    }

    /// The harness slide with a small image added: text box 0 at (112, 84) 800×600,
    /// image 1 at (920, 690) 80×40.
    fn with_image(h: &mut Harness) {
        let mut d = (*h.doc).clone();
        d.groups[0].slides[0]
            .elements
            .push(Element::Image(Box::new(doc::ImageBox {
                uuid: new_uuid(),
                name: "pic".into(),
                frame: doc::Rect {
                    x: 920.0,
                    y: 690.0,
                    width: 80.0,
                    height: 40.0,
                },
                rotation: 0.0,
                opacity: 1.0,
                locked: false,
                hidden: false,
                flip_h: false,
                flip_v: false,
                media: doc::MediaRef {
                    path: "Media/none.png".into(),
                    sha256: None,
                },
                adjust: doc::Adjust::default(),
                src: None,
            })));
        h.doc = Arc::new(d);
    }

    fn shift_click(h: &mut Harness, at: Pos2) {
        h.mods = egui::Modifiers::SHIFT;
        h.click(at);
        h.mods = egui::Modifiers::NONE;
        h.frame(vec![]);
    }

    #[test]
    fn shift_click_selects_several_and_they_move_as_one_step() {
        let mut h = Harness::new();
        with_image(&mut h);
        h.frame(vec![]);
        h.click(h.at(512.0, 384.0));
        let p = h.at(960.0, 710.0);
        shift_click(&mut h, p);
        assert_eq!(h.st.edit.selection(), vec![0, 1]);
        let edits = h.drag(
            h.at(960.0, 710.0),
            h.at(960.0, 710.0) + Vec2::new(-30.0, -30.0),
        );
        match edits.as_slice() {
            [Op::Batch { edits, .. }] => {
                assert_eq!(edits.len(), 2, "{edits:?}");
                let dx = |op: &Op, x0: f64| match op {
                    Op::SetFrame { frame, .. } => frame.x - x0,
                    other => panic!("{other:?}"),
                };
                assert_eq!(
                    dx(&edits[0], 112.0),
                    dx(&edits[1], 920.0),
                    "same move for both"
                );
                assert!(dx(&edits[0], 112.0) < 0.0);
            }
            other => panic!("expected one batch, got {other:?}"),
        }
        // Shift-click again: out of the selection.
        let p = h.at(960.0, 710.0);
        shift_click(&mut h, p);
        assert_eq!(h.st.edit.selection(), vec![0]);
    }

    #[test]
    fn marquee_selects_what_it_touches() {
        let mut h = Harness::new();
        with_image(&mut h);
        h.frame(vec![]);
        // From the empty bottom-right corner up-left over the image only.
        let edits = h.drag(h.at(1020.0, 765.0), h.at(950.0, 700.0));
        assert!(edits.is_empty());
        assert_eq!(h.st.edit.selection(), vec![1]);
        assert!(h.st.edit.marquee.is_none());
    }

    #[test]
    fn dragging_snaps_to_the_canvas_centre_unless_alt_is_held() {
        let mut h = Harness::new();
        with_image(&mut h);
        h.frame(vec![]);
        h.click(h.at(960.0, 710.0));
        // Image centre (960, 710) dragged to 2 canvas px right of the canvas centre.
        let s = h.scale();
        let to = h.at(514.0, 300.0);
        let edits = h.drag(h.at(960.0, 710.0), to);
        match edits.as_slice() {
            [Op::SetFrame { frame, .. }] => {
                assert_eq!(
                    frame.x + frame.width / 2.0,
                    512.0,
                    "snapped to the centre ({s})"
                );
            }
            other => panic!("{other:?}"),
        }
        // Again with Alt: no snapping.
        h.mods = egui::Modifiers::ALT;
        let edits = h.drag(h.at(960.0, 710.0), to);
        h.mods = egui::Modifiers::NONE;
        match edits.as_slice() {
            [Op::SetFrame { frame, .. }] => {
                assert!(
                    (frame.x + frame.width / 2.0 - 514.0).abs() <= 1.0,
                    "{frame:?}"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn tab_cycles_through_the_elements() {
        let mut h = Harness::new();
        with_image(&mut h);
        h.frame(vec![]);
        let tab = |shift: bool| egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: if shift {
                egui::Modifiers::SHIFT
            } else {
                egui::Modifiers::NONE
            },
        };
        h.frame(vec![tab(false)]);
        assert_eq!(h.st.edit.element, Some(0));
        h.frame(vec![tab(false)]);
        assert_eq!(h.st.edit.element, Some(1));
        h.frame(vec![tab(false)]);
        assert_eq!(h.st.edit.element, Some(0), "wraps round");
        h.frame(vec![tab(true)]);
        assert_eq!(h.st.edit.element, Some(1), "Shift+Tab goes back");
        assert!(
            h.ctx.memory(|m| m.focused().is_none()),
            "no button took focus"
        );
    }

    #[test]
    fn imported_files_are_copied_once_by_content() {
        let dir = std::env::temp_dir().join(format!("pp6-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("My Logo.png");
        ::image::RgbaImage::from_pixel(30, 20, ::image::Rgba([1, 2, 3, 255]))
            .save(&src)
            .unwrap();
        let doc_path = dir.join("songs").join("Song.pro6");
        std::fs::create_dir_all(doc_path.parent().unwrap()).unwrap();
        let a = image::import_file_for_test(&doc_path, &src).unwrap();
        let b = image::import_file_for_test(&doc_path, &src).unwrap();
        assert_eq!(a.media, b.media);
        assert!(
            a.media.path.starts_with("Media/My Logo-"),
            "{}",
            a.media.path
        );
        assert_eq!(a.size, (30, 20));
        let stored = a.media.resolve(&doc_path);
        assert_eq!(
            std::fs::read(&stored).unwrap(),
            std::fs::read(&src).unwrap()
        );
        assert_eq!(
            std::fs::read_dir(stored.parent().unwrap()).unwrap().count(),
            1
        );
        assert!(image::import_file_for_test(&doc_path, &dir.join("notes.txt")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dragging_a_slide_in_the_strip_moves_it() {
        let mut h = Harness::new();
        h.frame(vec![]);
        h.frame(vec![]);
        let tile = |h: &Harness, gs| h.st.edit.tiles.iter().find(|(k, _)| *k == gs).unwrap().1;
        // Slide 1 dropped just below slide 3: it becomes the third slide.
        let from = tile(&h, (0, 0)).center();
        let to = tile(&h, (0, 2)).center_bottom() + Vec2::new(0.0, 1.0);
        let edits = h.drag(from, to);
        assert_eq!(
            edits,
            vec![Op::MoveSlide {
                group: 0,
                slide: 0,
                to_group: 0,
                to: 2
            }]
        );
        assert_eq!(h.st.edit.sel, Some((0, 2)), "selection follows the slide");
        // Dropped where it already is: nothing.
        let t = tile(&h, (0, 1)).center();
        assert!(h.drag(t, t + Vec2::new(0.0, 8.0)).is_empty());
    }

    #[test]
    fn dragging_a_box_on_the_canvas_sends_one_move() {
        let mut h = Harness::new();
        h.frame(vec![]);
        h.frame(vec![]);
        // Slide 1's box: x 112, y 84, 800 × 600.
        let edits = h.drag(
            h.at(512.0, 384.0),
            h.at(512.0, 384.0) + Vec2::new(60.0, 30.0),
        );
        assert_eq!(h.st.edit.element, Some(0), "the box got selected");
        let s = h.scale();
        let want = doc::Rect {
            x: (112.0 + f64::from(60.0 / s)).round(),
            y: (84.0 + f64::from(30.0 / s)).round(),
            width: 800.0,
            height: 600.0,
        };
        match edits.as_slice() {
            [
                Op::SetFrame {
                    group: 0,
                    slide: 0,
                    element: 0,
                    frame,
                },
            ] => {
                assert!(
                    (frame.x - want.x).abs() <= 1.0 && (frame.y - want.y).abs() <= 1.0,
                    "{frame:?} vs {want:?}"
                );
                assert_eq!((frame.width, frame.height), (800.0, 600.0));
            }
            other => panic!("expected one SetFrame, got {other:?}"),
        }
    }

    fn double_click(h: &mut Harness, at: Pos2) -> Vec<Op> {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let mut out = h.frame(vec![egui::Event::PointerMoved(at)]);
        out.extend(h.frame(vec![
            button(true),
            button(false),
            button(true),
            button(false),
        ]));
        out.extend(h.frame(vec![]));
        out
    }

    fn key(h: &mut Harness, key: egui::Key) -> Vec<Op> {
        h.frame(vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: h.mods,
        }])
    }

    fn plain(paras: &[doc::Paragraph]) -> Vec<String> {
        paras.iter().map(doc::rich::text).collect()
    }

    #[test]
    fn typing_in_place_sends_one_text_edit() {
        let mut h = Harness::new();
        h.frame(vec![]);
        h.frame(vec![]);
        // Slide 1: "Centred middle" / "second line", centred in the box.
        let mid = h.at(512.0, 384.0);
        assert!(double_click(&mut h, mid).is_empty());
        assert!(h.st.edit.session.is_some(), "editing in place");
        // Caret to the very end, type, new paragraph, type, Backspace once.
        h.mods.command = true;
        key(&mut h, egui::Key::End);
        h.mods.command = false;
        h.frame(vec![egui::Event::Text("!".into())]);
        key(&mut h, egui::Key::Enter);
        h.frame(vec![egui::Event::Text("third liner".into())]);
        assert!(
            key(&mut h, egui::Key::Backspace).is_empty(),
            "nothing sent while typing"
        );
        // Select the first word and make it bold.
        h.mods.command = true;
        key(&mut h, egui::Key::Home);
        h.mods.command = false;
        h.mods.shift = true;
        h.mods.command = true;
        key(&mut h, egui::Key::ArrowRight);
        h.mods.shift = false;
        key(&mut h, egui::Key::B);
        h.mods.command = false;
        // Esc ends it: one edit with everything.
        let edits = key(&mut h, egui::Key::Escape);
        assert!(h.st.edit.session.is_none());
        match edits.as_slice() {
            [
                Op::SetRichText {
                    group: 0,
                    slide: 0,
                    element: 0,
                    paragraphs,
                },
            ] => {
                assert_eq!(
                    plain(paragraphs),
                    ["Centred middle", "second line!", "third line"]
                );
                assert!(paragraphs[0].runs[0].bold && paragraphs[0].runs[0].text == "Centred");
                assert!(!paragraphs[0].runs.last().unwrap().bold);
                assert_eq!(
                    paragraphs[2].runs[0].font, paragraphs[1].runs[0].font,
                    "new paragraph keeps the style"
                );
            }
            other => panic!("expected one SetRichText, got {other:?}"),
        }
        assert_eq!(h.st.edit.element, Some(0), "box stays selected");
    }

    #[test]
    fn clicking_outside_ends_editing_and_unchanged_text_sends_nothing() {
        let mut h = Harness::new();
        h.frame(vec![]);
        h.frame(vec![]);
        let mid = h.at(512.0, 384.0);
        double_click(&mut h, mid);
        assert!(h.st.edit.session.is_some());
        let edits = h.click(h.at(20.0, 20.0));
        assert!(h.st.edit.session.is_none());
        assert!(edits.is_empty(), "{edits:?}");
    }

    #[test]
    fn toolbar_styles_the_whole_box_and_effects_ask_first() {
        let mut h = Harness::new();
        h.frame(vec![]);
        h.click(h.at(512.0, 384.0));
        assert_eq!(h.st.edit.element, Some(0));
        // Ctrl+B is for text editing; the toolbar's apply does the whole box.
        let d = h.doc.clone();
        let mut actions = Vec::new();
        text::apply_for_test(&mut h.st, &mut actions, &h.path, &d, (0, 0, 0), true);
        match actions.as_slice() {
            [
                UiAction::Core(Command::Edit {
                    edit: Op::SetRichText { paragraphs, .. },
                    ..
                }),
            ] => {
                assert!(paragraphs.iter().flat_map(|p| &p.runs).all(|r| r.bold));
            }
            _ => panic!("expected a SetRichText edit"),
        }
        // An outline in a .pro6 document waits for "Continue anyway".
        let mut actions = Vec::new();
        text::send_checked(
            &mut h.st,
            &mut actions,
            &h.path,
            &d,
            Op::SetOutline {
                group: 0,
                slide: 0,
                element: 0,
                outline: Some(doc::Outline {
                    width: 2.0,
                    color: doc::Rgba {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    },
                }),
            },
            None,
        );
        assert!(actions.is_empty());
        assert!(h.st.edit.pending_native.is_some());
    }

    #[test]
    fn resize_and_rotate_handles_and_nudges() {
        let mut h = Harness::new();
        h.frame(vec![]);
        h.click(h.at(512.0, 384.0));
        assert_eq!(h.st.edit.element, Some(0));
        // Bottom-right handle dragged 40 points right.
        let edits = h.drag(
            h.at(912.0, 684.0),
            h.at(912.0, 684.0) + Vec2::new(40.0, 0.0),
        );
        match edits.as_slice() {
            [Op::SetFrame { frame, .. }] => {
                assert_eq!((frame.x, frame.y), (112.0, 84.0), "top-left stays");
                assert!((frame.width - (800.0 + f64::from(40.0 / h.scale()))).abs() <= 1.0);
                assert_eq!(frame.height, 600.0);
            }
            other => panic!("expected a resize, got {other:?}"),
        }
        // Rotation handle (above the top centre) dragged round to the right.
        let arm = h.at(512.0, 84.0) - Vec2::new(0.0, ROTATE_ARM);
        let edits = h.drag(arm, h.at(512.0, 384.0) + Vec2::new(200.0, 0.0));
        assert!(
            matches!(edits.as_slice(), [Op::SetRotation { degrees, .. }] if *degrees == 90.0),
            "{edits:?}"
        );
        // Arrow keys nudge 1 px (Shift: 10).
        let key = |key, shift| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                shift,
                ..Default::default()
            },
        };
        let edits = h.frame(vec![key(egui::Key::ArrowRight, false)]);
        assert!(
            matches!(edits.as_slice(), [Op::SetFrame { frame, .. }] if frame.x == 113.0),
            "{edits:?}"
        );
        h.mods.shift = true;
        let edits = h.frame(vec![key(egui::Key::ArrowDown, true)]);
        h.mods.shift = false;
        assert!(
            matches!(edits.as_slice(), [Op::SetFrame { frame, .. }] if frame.y == 94.0),
            "{edits:?}"
        );
        // Clicking empty canvas deselects.
        h.click(h.at(20.0, 20.0));
        assert_eq!(h.st.edit.element, None);
    }

    fn drag(grab: Grab, rotation: f64) -> Drag {
        let f = doc::Rect {
            x: 100.0,
            y: 100.0,
            width: 200.0,
            height: 100.0,
        };
        Drag {
            grab,
            element: 0,
            start: Pos2::new(0.0, 0.0),
            frame: f,
            rotation,
            now_frame: f,
            now_rotation: rotation,
            keep_aspect: false,
            members: vec![(0, f, rotation)],
            preview: None,
        }
    }

    #[test]
    fn image_resize_keeps_its_shape() {
        let mut dr = drag(Grab::Resize(1, 1), 0.0);
        dr.keep_aspect = true;
        // Pointer mostly right: the width leads, the height follows at 2:1.
        drag_update(&mut dr, Pos2::new(100.0, 10.0), None, false);
        assert_eq!(dr.now_frame.width, 300.0);
        assert_eq!(dr.now_frame.height, 150.0);
        assert_eq!(
            (dr.now_frame.x, dr.now_frame.y),
            (100.0, 100.0),
            "top-left stays"
        );
        // An edge handle: the other side grows evenly about the centre.
        let mut dr = drag(Grab::Resize(0, 1), 0.0);
        dr.keep_aspect = true;
        drag_update(&mut dr, Pos2::new(0.0, 50.0), None, false);
        assert_eq!((dr.now_frame.width, dr.now_frame.height), (300.0, 150.0));
        assert_eq!(dr.now_frame.x, 50.0);
        assert_eq!(dr.now_frame.y, 100.0);
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1.0
    }

    #[test]
    fn move_follows_the_pointer_in_whole_pixels() {
        let mut d = drag(Grab::Move, 0.0);
        drag_update(&mut d, Pos2::new(10.4, -5.6), None, false);
        assert_eq!((d.now_frame.x, d.now_frame.y), (110.0, 94.0));
        assert_eq!((d.now_frame.width, d.now_frame.height), (200.0, 100.0));
    }

    #[test]
    fn resize_keeps_the_opposite_edge() {
        // Right edge dragged 50 px right: left edge stays at 100.
        let mut d = drag(Grab::Resize(1, 0), 0.0);
        drag_update(&mut d, Pos2::new(50.0, 30.0), None, false);
        assert_eq!(d.now_frame.x, 100.0);
        assert_eq!(d.now_frame.width, 250.0);
        assert_eq!(d.now_frame.height, 100.0, "edge handle changes one axis");
        // Top-left corner dragged in: the bottom-right corner stays put.
        let mut d = drag(Grab::Resize(-1, -1), 0.0);
        drag_update(&mut d, Pos2::new(20.0, 10.0), None, false);
        assert_eq!((d.now_frame.x, d.now_frame.y), (120.0, 110.0));
        assert_eq!((d.now_frame.width, d.now_frame.height), (180.0, 90.0));
        // Never smaller than the minimum.
        let mut d = drag(Grab::Resize(1, 1), 0.0);
        drag_update(&mut d, Pos2::new(-1000.0, -1000.0), None, false);
        assert_eq!(
            (d.now_frame.width, d.now_frame.height),
            (MIN_SIZE, MIN_SIZE)
        );
    }

    #[test]
    fn resizing_a_rotated_box_keeps_its_opposite_edge_in_place() {
        // Rotated 90°: the box's own right edge points down on screen.
        let mut d = drag(Grab::Resize(1, 0), 90.0);
        let left_before = handle_pos(&d.frame, 90.0, -1, 0);
        drag_update(&mut d, Pos2::new(0.0, 40.0), None, false);
        assert!(close(d.now_frame.width, 240.0), "{:?}", d.now_frame);
        let left_after = handle_pos(&d.now_frame, 90.0, -1, 0);
        assert!(
            left_before.distance(left_after) <= 1.0,
            "{left_before:?} {left_after:?}"
        );
    }

    #[test]
    fn rotate_points_the_handle_at_the_pointer() {
        let mut d = drag(Grab::Rotate, 0.0);
        let c = centre(&d.frame);
        drag_update(&mut d, c + Vec2::new(100.0, 0.0), None, false); // pointer to the right
        assert_eq!(d.now_rotation, 90.0);
        drag_update(&mut d, c + Vec2::new(0.0, -100.0), None, false); // straight up
        assert_eq!(d.now_rotation, 0.0);
        drag_update(&mut d, c + Vec2::new(-100.0, 0.0), None, false);
        assert_eq!(d.now_rotation, 270.0);
    }

    #[test]
    fn hit_testing_follows_rotation() {
        let f = doc::Rect {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 20.0,
        };
        // A wide, flat box: a point 50 px above the centre is outside upright,
        // inside when turned 90°.
        let p = centre(&f) + Vec2::new(0.0, -50.0);
        assert!(!hit(&f, 0.0, p));
        assert!(hit(&f, 90.0, p));
    }
}
