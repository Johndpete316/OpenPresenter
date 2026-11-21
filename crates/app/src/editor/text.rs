//! Rich text in the editor (docs/EDITOR-PLAN.md E2): editing text in place on the
//! canvas, the text toolbar, the Effects panel, and the "won't work in ProPresenter
//! 6" question.
//!
//! A text editing session works on its own copy of the box's paragraphs; the canvas
//! shows it live (rendered by `render`, like everything else) and the caret and
//! selection come from the same layout (`render::layout_box`). Leaving the box
//! (Esc, clicking elsewhere, another slide, Save) sends it as one edit: one undo step.

use std::sync::Arc;

use doc::rich::{self, Pos};
use doc::{Align, Feature, Paragraph, Run, VAlign};
use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use super::*;

/// Text being edited in place.
pub(super) struct TextSession {
    pub g: usize,
    pub s: usize,
    pub e: usize,
    slide_uuid: String,
    el_uuid: String,
    pub paras: Vec<Paragraph>,
    base: Vec<Paragraph>,
    pub caret: Pos,
    pub anchor: Pos,
    /// Style for the next typed text (set by the toolbar with nothing selected).
    typing: Option<Run>,
    /// Bumped on every change (keys the preview and the layout).
    rev: u64,
    preview: Option<(u64, Arc<doc::Document>)>,
    layout: Option<(u64, u32, render::BoxLayout)>,
    /// Column kept while moving up and down.
    want_x: Option<f32>,
    /// When the caret last moved (blink phase).
    moved_at: f64,
    /// Output pixels per canvas pixel of the canvas image (set each frame), so the
    /// caret uses exactly the layout that's on screen.
    pub scale: f32,
}

impl TextSession {
    /// The text differs from the box's.
    pub(super) fn dirty(&self) -> bool {
        self.paras != self.base
    }

    fn changed(&mut self) {
        self.rev += 1;
        self.typing = None;
    }

    fn has_selection(&self) -> bool {
        self.caret != self.anchor
    }

    fn sel(&self) -> (Pos, Pos) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    fn delete_selection(&mut self) -> bool {
        if !self.has_selection() {
            return false;
        }
        let (a, b) = self.sel();
        self.caret = rich::delete(&mut self.paras, a, b);
        self.anchor = self.caret;
        self.changed();
        true
    }

    fn insert(&mut self, s: &str) {
        self.delete_selection_keep_style();
        let style = self
            .typing
            .clone()
            .or_else(|| rich::style_at(&self.paras, self.caret));
        let Some(style) = style else { return };
        self.caret = rich::insert(&mut self.paras, self.caret, s, &style);
        self.anchor = self.caret;
        self.changed();
    }

    /// Delete the selection but keep a pending typing style.
    fn delete_selection_keep_style(&mut self) {
        let typing = self.typing.clone().or_else(|| {
            self.has_selection()
                .then(|| rich::style_at(&self.paras, rich::next(&self.paras, self.sel().0)))
                .flatten()
        });
        self.delete_selection();
        self.typing = typing;
    }

    fn move_to(&mut self, p: Pos, extend: bool, now: f64) {
        self.caret = rich::clamp(&self.paras, p);
        if !extend {
            self.anchor = self.caret;
        }
        self.typing = None;
        self.moved_at = now;
    }
}

/// An edit waiting for "Continue anyway".
pub(super) struct PendingNative {
    op: Op,
    features: Vec<Feature>,
    slides: Vec<String>,
}

/// Send an edit after checking it against `.pro6` (asks first when it would make the
/// presentation OpenPresenter-only).
pub(super) fn send_checked(
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    op: Op,
    merge: Option<String>,
) {
    match doc::check(d, &op) {
        doc::Check::Ok => match merge {
            Some(key) => edit_merging(actions, path, op, key),
            None => edit(actions, path, op),
        },
        doc::Check::NeedsNative { features, slides } => {
            let names = slides
                .iter()
                .filter_map(|u| {
                    d.groups.iter().find_map(|g| {
                        g.slides
                            .iter()
                            .position(|s| &s.uuid == u)
                            .map(|i| format!("{} {}", g.name, i + 1))
                    })
                })
                .collect();
            st.edit.pending_native = Some(PendingNative {
                op,
                features,
                slides: names,
            });
        }
    }
}

/// "This won't work in ProPresenter 6." (docs/EDITOR-PLAN.md, Compatibility Modes).
pub(super) fn native_prompt(
    ctx: &egui::Context,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
) {
    let Some(p) = &st.edit.pending_native else {
        return;
    };
    let mut answer = None;
    egui::Modal::new(egui::Id::unique("editor-native")).show(ctx, |ui| {
        ui.set_max_width(440.0);
        ui.heading("This won't work in ProPresenter 6.");
        let names: Vec<&str> = p.features.iter().map(|f| f.name()).collect();
        let what = names.join(", ");
        let slides = match p.slides.as_slice() {
            [] => "this presentation".to_owned(),
            [one] => format!("slide “{one}”"),
            many => format!("slides “{}”", many.join("”, “")),
        };
        ui.label(format!(
            "{what} can't be saved in a .pro6 file. If you continue, {slides} will only open correctly in OpenPresenter."
        ));
        ui.label(
            RichText::new("The original .pro6 file is kept; this presentation will be saved as a .opres file next to it.")
                .small()
                .weak(),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Don't do it").clicked() {
                answer = Some(false);
            }
            if ui.button(RichText::new("Continue anyway").strong()).clicked() {
                answer = Some(true);
            }
        });
    });
    match answer {
        Some(true) => {
            if let Some(p) = st.edit.pending_native.take() {
                actions.push(UiAction::Core(Command::Edit {
                    presentation: path.to_path_buf(),
                    edit: p.op,
                    accept_native: true,
                }));
            }
        }
        Some(false) => st.edit.pending_native = None,
        None => {}
    }
}

// ───────────────────────────── session ─────────────────────────────

/// Start editing box `e` of slide `(g, s)`, caret at box-local canvas point `at`
/// (or at the end of the text).
#[allow(clippy::too_many_arguments)]
pub(super) fn begin(
    st: &mut UiState,
    d: &doc::Document,
    (g, s, e): (usize, usize, usize),
    at: Option<(f32, f32)>,
    scale: f32,
    now: f64,
) {
    let Some(t) = text_box(d, g, s, e) else {
        return;
    };
    if t.problem.is_some() || t.runs().next().is_none() {
        // Unreadable / unstyled: the Text tab explains why.
        st.edit.tab = Tab::Text;
        return;
    }
    let mut sess = TextSession {
        g,
        s,
        e,
        slide_uuid: d.groups[g].slides[s].uuid.clone(),
        el_uuid: t.uuid.clone(),
        paras: t.paragraphs.clone(),
        base: t.paragraphs.clone(),
        caret: rich::end(&t.paragraphs),
        anchor: rich::end(&t.paragraphs),
        typing: None,
        rev: 0,
        preview: None,
        layout: None,
        want_x: None,
        moved_at: now,
        scale,
    };
    rich::normalize(&mut sess.paras);
    if let Some((x, y)) = at
        && let Some(l) = session_layout(st.fonts.as_mut(), d, &mut sess)
    {
        let p = l.hit(x, y);
        sess.caret = p;
        sess.anchor = p;
    }
    st.edit.element = Some(e);
    st.edit.session = Some(sess);
}

/// End the session, sending its text as one edit if it changed.
pub(super) fn commit(
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
) {
    let Some(sess) = st.edit.session.take() else {
        return;
    };
    if sess.paras == sess.base || text_box(d, sess.g, sess.s, sess.e).is_none() {
        return;
    }
    send_checked(
        st,
        actions,
        path,
        d,
        Op::SetRichText {
            group: sess.g,
            slide: sess.s,
            element: sess.e,
            paragraphs: sess.paras,
        },
        None,
    );
    st.edit.text_for = None;
}

/// Keep the session valid: it ends (committed) when its slide or box isn't selected
/// any more, and is dropped if the box is gone.
pub(super) fn check_session(
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
) {
    let Some(sess) = &st.edit.session else { return };
    let gone = text_box(d, sess.g, sess.s, sess.e).is_none_or(|t| t.uuid != sess.el_uuid)
        || d.groups[sess.g].slides[sess.s].uuid != sess.slide_uuid;
    if gone {
        st.edit.session = None;
        return;
    }
    if st.edit.sel != Some((sess.g, sess.s)) || st.edit.element != Some(sess.e) {
        commit(st, actions, path, d);
    }
}

/// The document as the session shows it (for the canvas).
pub(super) fn preview(sess: &mut TextSession, d: &Arc<doc::Document>) -> Arc<doc::Document> {
    if let Some((rev, p)) = &sess.preview
        && *rev == sess.rev
    {
        return p.clone();
    }
    let mut p = (**d).clone();
    if let Some(Element::Text(t)) = p.groups[sess.g].slides[sess.s].elements.get_mut(sess.e) {
        t.paragraphs.clone_from(&sess.paras);
    }
    let p = Arc::new(p);
    sess.preview = Some((sess.rev, p.clone()));
    p
}

/// Layout of the session's text at `scale` output pixels per canvas pixel.
fn session_layout<'a>(
    fonts: Option<&mut render::Fonts>,
    d: &doc::Document,
    sess: &'a mut TextSession,
) -> Option<&'a render::BoxLayout> {
    let fonts = fonts?;
    let scale = sess.scale;
    let key = (sess.rev, scale.to_bits());
    if sess.layout.as_ref().is_none_or(|(r, s, _)| (*r, *s) != key) {
        let mut t = text_box(d, sess.g, sess.s, sess.e)?.clone();
        t.paragraphs.clone_from(&sess.paras);
        let l = render::layout_box(fonts, &d.path, &sess.slide_uuid, &t, scale).ok()?;
        sess.layout = Some((key.0, key.1, l));
    }
    sess.layout.as_ref().map(|(_, _, l)| l)
}

/// Keyboard input for the session. Returns true if the session ended.
pub(super) fn keys(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
) {
    let now = ui.input(|i| i.time);
    let events = ui.input(|i| i.events.clone());
    let mut end = false;
    {
        let UiState { edit, fonts, .. } = &mut *st;
        let Some(sess) = edit.session.as_mut() else {
            return;
        };
        for ev in events {
            match ev {
                egui::Event::Text(t) => sess.insert(&t),
                egui::Event::Paste(t) => {
                    let t = t.replace("\r\n", "\n").replace('\r', "\n");
                    sess.insert(&t);
                }
                egui::Event::Copy | egui::Event::Cut => {
                    if sess.has_selection() {
                        let (a, b) = sess.sel();
                        ui.ctx().copy_text(rich::text_between(&sess.paras, a, b));
                        if matches!(ev, egui::Event::Cut) {
                            sess.delete_selection();
                        }
                    }
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers: m,
                    ..
                } => {
                    let extend = m.shift;
                    let word = m.command;
                    match key {
                        egui::Key::Escape => end = true,
                        egui::Key::Backspace => {
                            if !sess.delete_selection() {
                                let from = if word {
                                    rich::word_prev(&sess.paras, sess.caret)
                                } else {
                                    rich::prev(&sess.paras, sess.caret)
                                };
                                sess.caret = rich::delete(&mut sess.paras, from, sess.caret);
                                sess.anchor = sess.caret;
                                sess.changed();
                            }
                        }
                        egui::Key::Delete => {
                            if !sess.delete_selection() {
                                let to = if word {
                                    rich::word_next(&sess.paras, sess.caret)
                                } else {
                                    rich::next(&sess.paras, sess.caret)
                                };
                                sess.caret = rich::delete(&mut sess.paras, sess.caret, to);
                                sess.anchor = sess.caret;
                                sess.changed();
                            }
                        }
                        egui::Key::Enter => {
                            if m.shift {
                                sess.insert(&rich::LINE_BREAK.to_string());
                            } else {
                                sess.delete_selection_keep_style();
                                sess.caret = rich::split(&mut sess.paras, sess.caret);
                                sess.anchor = sess.caret;
                                sess.changed();
                            }
                        }
                        egui::Key::ArrowLeft | egui::Key::ArrowRight => {
                            let left = key == egui::Key::ArrowLeft;
                            let p = if sess.has_selection() && !extend {
                                if left { sess.sel().0 } else { sess.sel().1 }
                            } else if left {
                                if word {
                                    rich::word_prev(&sess.paras, sess.caret)
                                } else {
                                    rich::prev(&sess.paras, sess.caret)
                                }
                            } else if word {
                                rich::word_next(&sess.paras, sess.caret)
                            } else {
                                rich::next(&sess.paras, sess.caret)
                            };
                            sess.want_x = None;
                            sess.move_to(p, extend, now);
                        }
                        egui::Key::ArrowUp
                        | egui::Key::ArrowDown
                        | egui::Key::Home
                        | egui::Key::End => {
                            let target = if word && key == egui::Key::Home {
                                Some(Pos::default())
                            } else if word && key == egui::Key::End {
                                Some(rich::end(&sess.paras))
                            } else {
                                let caret = sess.caret;
                                let want = sess.want_x;
                                session_layout(fonts.as_mut(), d, sess)
                                    .map(|l| {
                                        let (x, top, h) = l.caret(caret);
                                        let x = want.unwrap_or(x);
                                        match key {
                                            egui::Key::ArrowUp => {
                                                (l.hit(x, top - h * 0.5), Some(x))
                                            }
                                            egui::Key::ArrowDown => {
                                                (l.hit(x, top + h * 1.5), Some(x))
                                            }
                                            egui::Key::Home => (l.hit(-1e4, top + h * 0.5), None),
                                            _ => (l.hit(1e6, top + h * 0.5), None),
                                        }
                                    })
                                    .map(|(p, x)| {
                                        sess.want_x = x;
                                        p
                                    })
                            };
                            if let Some(p) = target {
                                sess.move_to(p, extend, now);
                            }
                        }
                        egui::Key::A if m.command => {
                            sess.anchor = Pos::default();
                            sess.caret = rich::end(&sess.paras);
                        }
                        egui::Key::B if m.command => toggle(sess, |r| r.bold, |r, v| r.bold = v),
                        egui::Key::I if m.command => {
                            toggle(sess, |r| r.italic, |r, v| r.italic = v)
                        }
                        egui::Key::U if m.command => {
                            toggle(sess, |r| r.underline, |r, v| r.underline = v)
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    if end {
        commit(st, actions, path, d);
    }
}

/// Toggle a flag on the selection (or the typing style).
fn toggle(sess: &mut TextSession, get: impl Fn(&Run) -> bool, set: impl Fn(&mut Run, bool) + Copy) {
    let runs = current_runs(sess);
    let on = !runs.iter().all(&get);
    restyle_session(sess, |r| set(r, on));
}

/// Runs the toolbar describes: the selection, or the style typing would use.
fn current_runs(sess: &TextSession) -> Vec<Run> {
    if sess.has_selection() {
        let (a, b) = sess.sel();
        rich::runs_between(&sess.paras, a, b)
    } else {
        sess.typing
            .clone()
            .or_else(|| rich::style_at(&sess.paras, sess.caret))
            .into_iter()
            .collect()
    }
}

fn restyle_session(sess: &mut TextSession, f: impl FnMut(&mut Run)) {
    if sess.has_selection() {
        let (a, b) = sess.sel();
        rich::restyle(&mut sess.paras, a, b, f);
        sess.rev += 1;
    } else {
        let mut f = f;
        let mut style = sess
            .typing
            .clone()
            .or_else(|| rich::style_at(&sess.paras, sess.caret));
        if let Some(s) = style.as_mut() {
            f(s);
        }
        // An empty paragraph's style-holding run takes it too.
        let p = sess.caret.para;
        if sess.paras.get(p).is_some_and(|p| rich::len(p) == 0) {
            rich::restyle(&mut sess.paras, sess.caret, sess.caret, f);
            sess.rev += 1;
        }
        sess.typing = style;
    }
}

/// Mouse on the session's box: `local` is the box-local canvas point.
pub(super) enum Mouse {
    Press { extend: bool },
    Drag,
    Double,
}

pub(super) fn pointer(st: &mut UiState, d: &doc::Document, local: (f32, f32), m: Mouse, now: f64) {
    let UiState { edit, fonts, .. } = &mut *st;
    let Some(sess) = edit.session.as_mut() else {
        return;
    };
    let Some(l) = session_layout(fonts.as_mut(), d, sess) else {
        return;
    };
    let p = l.hit(local.0, local.1);
    match m {
        Mouse::Double => {
            sess.anchor = rich::word_prev(&sess.paras, rich::next(&sess.paras, p));
            sess.caret = rich::word_next(&sess.paras, sess.anchor);
            sess.moved_at = now;
        }
        Mouse::Press { extend } => {
            sess.want_x = None;
            sess.move_to(p, extend, now);
        }
        Mouse::Drag => {
            sess.caret = p;
            sess.moved_at = now;
        }
    }
}

/// Selection and blinking caret, drawn over the canvas.
pub(super) fn paint(
    ui: &egui::Ui,
    painter: &egui::Painter,
    st: &mut UiState,
    d: &doc::Document,
    to_screen: &dyn Fn(Pos2) -> Pos2,
) {
    let now = ui.input(|i| i.time);
    let UiState { edit, fonts, .. } = &mut *st;
    let Some(sess) = edit.session.as_mut() else {
        return;
    };
    let Some(t) = text_box(d, sess.g, sess.s, sess.e) else {
        return;
    };
    let (f, r) = (t.frame, t.rotation);
    let (caret, sel, moved) = (sess.caret, sess.sel(), sess.moved_at);
    let Some(l) = session_layout(fonts.as_mut(), d, sess) else {
        return;
    };
    let half = Vec2::new(f.width as f32, f.height as f32) / 2.0;
    let at = |x: f32, y: f32| to_screen(centre(&f) + rotate(Vec2::new(x, y) - half, r));
    if sel.0 != sel.1 {
        for [x, y, w, h] in l.selection(sel.0, sel.1) {
            let pts = vec![at(x, y), at(x + w, y), at(x + w, y + h), at(x, y + h)];
            painter.add(egui::Shape::convex_polygon(
                pts,
                Color32::from_rgba_unmultiplied(90, 160, 255, 90),
                Stroke::NONE,
            ));
        }
    }
    let on = ((now - moved) * 2.0) as i64 % 2 == 0;
    if on {
        let (x, top, h) = l.caret(caret);
        painter.line_segment(
            [at(x, top), at(x, top + h)],
            Stroke::new(2.0, Color32::WHITE),
        );
        painter.line_segment([at(x, top), at(x, top + h)], Stroke::new(1.0, SELECT));
    }
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(500));
}

// ───────────────────────────── toolbar ─────────────────────────────

#[derive(Clone)]
enum Change {
    Font(String),
    Size(f64),
    SizeStep(f64),
    Color([u8; 3]),
    Bold(bool),
    Italic(bool),
    Underline(bool),
    Align(Align),
    LineSpacing(f64),
    LetterSpacing(f64),
}

impl Change {
    fn run(&self, r: &mut Run) {
        match self {
            Change::Font(f) => r.font.clone_from(f),
            Change::Size(v) => r.size = v.clamp(1.0, 1000.0),
            Change::SizeStep(d) => r.size = (r.size + d).clamp(1.0, 1000.0),
            Change::Color(c) => r.color = Some(*c),
            Change::Bold(v) => r.bold = *v,
            Change::Italic(v) => r.italic = *v,
            Change::Underline(v) => r.underline = *v,
            Change::LetterSpacing(v) => r.letter_spacing = *v,
            Change::Align(_) | Change::LineSpacing(_) => {}
        }
    }
}

/// Apply a toolbar change: to the session's selection / typing style, or to the
/// whole box (one edit; `merge` joins a run of slider changes into one undo step).
fn apply(
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    (g, s, e): (usize, usize, usize),
    change: Change,
    merge: Option<String>,
) {
    if let Change::Color(c) = change {
        st.edit.recent_colors.retain(|x| *x != c);
        st.edit.recent_colors.insert(0, c);
        st.edit.recent_colors.truncate(8);
    }
    if let Some(sess) = st.edit.session.as_mut() {
        let (a, b) = sess.sel();
        match &change {
            Change::Align(al) => {
                rich::set_align(&mut sess.paras, a, b, *al);
                sess.rev += 1;
            }
            Change::LineSpacing(v) => {
                rich::set_line_spacing(&mut sess.paras, a, b, *v);
                sess.rev += 1;
            }
            c => {
                let c = c.clone();
                restyle_session(sess, |r| c.run(r));
            }
        }
        return;
    }
    let Some(t) = text_box(d, g, s, e) else {
        return;
    };
    let mut paras = t.paragraphs.clone();
    let end = rich::end(&paras);
    match &change {
        Change::Align(al) => rich::set_align(&mut paras, Pos::default(), end, *al),
        Change::LineSpacing(v) => rich::set_line_spacing(&mut paras, Pos::default(), end, *v),
        c => rich::restyle_all(&mut paras, |r| c.run(r)),
    }
    if paras != t.paragraphs {
        send_checked(
            st,
            actions,
            path,
            d,
            Op::SetRichText {
                group: g,
                slide: s,
                element: e,
                paragraphs: paras,
            },
            merge,
        );
    }
}

/// A small painted icon button for alignment.
fn align_button(ui: &mut egui::Ui, selected: bool, lines: &[(f32, f32)], tip: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(24.0, 20.0), Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 3.0, Color32::from_rgb(50, 80, 130));
    } else if resp.hovered() {
        p.rect_filled(rect, 3.0, Color32::from_gray(60));
    }
    let n = lines.len() as f32;
    for (i, (x0, x1)) in lines.iter().enumerate() {
        let y = rect.top() + 4.0 + (i as f32) * (12.0 / (n - 1.0).max(1.0));
        p.line_segment(
            [
                Pos2::new(rect.left() + 4.0 + x0 * 16.0, y),
                Pos2::new(rect.left() + 4.0 + x1 * 16.0, y),
            ],
            Stroke::new(1.5, Color32::from_gray(220)),
        );
    }
    resp.on_hover_text(tip).clicked()
}

fn valign_button(ui: &mut egui::Ui, selected: bool, at: f32, tip: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(24.0, 20.0), Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 3.0, Color32::from_rgb(50, 80, 130));
    } else if resp.hovered() {
        p.rect_filled(rect, 3.0, Color32::from_gray(60));
    }
    let b = rect.shrink2(Vec2::new(5.0, 3.0));
    p.rect_stroke(
        b,
        1.0,
        Stroke::new(1.0, Color32::from_gray(150)),
        egui::StrokeKind::Inside,
    );
    let y = b.top() + 3.0 + at * (b.height() - 6.0);
    p.line_segment(
        [Pos2::new(b.left() + 3.0, y), Pos2::new(b.right() - 3.0, y)],
        Stroke::new(2.0, Color32::from_gray(230)),
    );
    resp.on_hover_text(tip).clicked()
}

/// The text toolbar (contextual bar when a text box is selected).
pub(super) fn toolbar(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    (g, s, e): (usize, usize, usize),
) {
    let Some(t) = text_box(d, g, s, e) else {
        return;
    };
    if t.problem.is_some() {
        ui.label(
            RichText::new("This text box's text can't be edited (see the Text tab).")
                .color(Color32::YELLOW),
        );
        return;
    }
    let runs: Vec<Run> = match &st.edit.session {
        Some(sess) => current_runs(sess),
        None => t.runs().cloned().collect(),
    };
    let Some(first) = runs.first().cloned() else {
        return;
    };
    let all = |f: &dyn Fn(&Run) -> bool| runs.iter().all(f);
    let at = (g, s, e);
    let key = |what: &str| Some(format!("{what}:{}", t.uuid));

    // Font.
    let mixed_font = !all(&|r| r.font == first.font);
    let installed = st
        .fonts
        .as_ref()
        .is_none_or(|f| f.resolve(&first.font, false, false).is_some());
    let label = if mixed_font {
        RichText::new("Mixed fonts")
    } else if installed {
        RichText::new(&first.font)
    } else {
        RichText::new(format!("{} (not installed)", first.font)).color(Color32::LIGHT_RED)
    };
    let mut picked = None;
    ui.menu_button(label, |ui| {
        picked = font_picker(ui, st, &first.font);
        if picked.is_some() {
            ui.close();
        }
    })
    .response
    .on_hover_text("Font");
    if let Some(f) = picked {
        apply(st, actions, path, d, at, Change::Font(f), None);
    }

    // Size.
    let mixed_size = !all(&|r| r.size == first.size);
    if ui.small_button("−").on_hover_text("Smaller").clicked() {
        apply(
            st,
            actions,
            path,
            d,
            at,
            Change::SizeStep(-1.0),
            key("size"),
        );
    }
    let mut size = first.size;
    let r = ui.add(
        egui::DragValue::new(&mut size)
            .range(1.0..=1000.0)
            .speed(0.5)
            .max_decimals(1)
            .custom_formatter(move |v, _| {
                if mixed_size {
                    format!("{v:.0}+")
                } else {
                    format!("{v:.0}")
                }
            }),
    );
    if r.changed() {
        apply(st, actions, path, d, at, Change::Size(size), key("size"));
    }
    if ui.small_button("+").on_hover_text("Larger").clicked() {
        apply(st, actions, path, d, at, Change::SizeStep(1.0), key("size"));
    }

    // Colour, with recent colours.
    let [cr, cg, cb] = first.color.unwrap_or([0, 0, 0]);
    let mut col = Color32::from_rgb(cr, cg, cb);
    if egui::color_picker::color_edit_button_srgba(ui, &mut col, egui::color_picker::Alpha::Opaque)
        .on_hover_text("Text colour")
        .changed()
    {
        apply(
            st,
            actions,
            path,
            d,
            at,
            Change::Color([col.r(), col.g(), col.b()]),
            key("color"),
        );
    }
    for c in st.edit.recent_colors.clone() {
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::click());
        ui.painter()
            .rect_filled(rect, 2.0, Color32::from_rgb(c[0], c[1], c[2]));
        ui.painter().rect_stroke(
            rect,
            2.0,
            Stroke::new(1.0, Color32::from_gray(90)),
            egui::StrokeKind::Inside,
        );
        if resp.on_hover_text("Recent colour").clicked() {
            apply(st, actions, path, d, at, Change::Color(c), None);
        }
    }
    ui.separator();

    // Bold / italic / underline.
    let b = all(&|r| r.bold);
    if ui
        .selectable_label(b, RichText::new("B").strong())
        .on_hover_text("Bold (Ctrl+B)")
        .clicked()
    {
        apply(st, actions, path, d, at, Change::Bold(!b), None);
    }
    let i = all(&|r| r.italic);
    if ui
        .selectable_label(i, RichText::new("I").italics())
        .on_hover_text("Italic (Ctrl+I)")
        .clicked()
    {
        apply(st, actions, path, d, at, Change::Italic(!i), None);
    }
    let u = all(&|r| r.underline);
    if ui
        .selectable_label(u, RichText::new("U").underline())
        .on_hover_text("Underline (Ctrl+U)")
        .clicked()
    {
        apply(st, actions, path, d, at, Change::Underline(!u), None);
    }
    ui.separator();

    // Alignment (of the session's paragraphs, or the box's first).
    let (paras, caret_para) = match &st.edit.session {
        Some(sess) => (&sess.paras[..], sess.caret.para),
        None => (&t.paragraphs[..], 0),
    };
    let align = paras.get(caret_para).map(|p| p.align).unwrap_or_default();
    let line_spacing = paras.get(caret_para).map_or(1.0, |p| p.line_spacing);
    for (a, lines, tip) in [
        (
            Align::Left,
            [(0.0, 1.0), (0.0, 0.6), (0.0, 0.85)],
            "Align left",
        ),
        (
            Align::Center,
            [(0.0, 1.0), (0.2, 0.8), (0.1, 0.9)],
            "Centre",
        ),
        (
            Align::Right,
            [(0.0, 1.0), (0.4, 1.0), (0.15, 1.0)],
            "Align right",
        ),
        (
            Align::Justified,
            [(0.0, 1.0), (0.0, 1.0), (0.0, 1.0)],
            "Justify",
        ),
    ] {
        if align_button(ui, align == a, &lines, tip) {
            apply(st, actions, path, d, at, Change::Align(a), None);
        }
    }
    ui.add_space(4.0);
    for (v, pos, tip) in [
        (VAlign::Top, 0.0, "Top"),
        (VAlign::Middle, 0.5, "Middle"),
        (VAlign::Bottom, 1.0, "Bottom"),
    ] {
        if valign_button(ui, t.valign == v, pos, tip) && t.valign != v {
            if let Some(sess) = st.edit.session.take() {
                // Keep editing after the box moves its text.
                st.edit.session = Some(sess);
            }
            send_checked(
                st,
                actions,
                path,
                d,
                Op::SetVAlign {
                    group: g,
                    slide: s,
                    element: e,
                    valign: v,
                },
                None,
            );
        }
    }
    ui.separator();

    // Spacing.
    ui.menu_button("Spacing", |ui| {
        let mut ls = line_spacing;
        ui.label("Line spacing");
        if ui
            .add(egui::Slider::new(&mut ls, 0.5..=3.0).step_by(0.05).suffix("×"))
            .changed()
        {
            apply(st, actions, path, d, at, Change::LineSpacing(ls), key("line-spacing"));
        }
        let mut lt = first.letter_spacing;
        ui.label("Letter spacing");
        if ui
            .add(egui::Slider::new(&mut lt, -10.0..=60.0).step_by(0.5).suffix(" px"))
            .changed()
        {
            apply(st, actions, path, d, at, Change::LetterSpacing(lt), key("letter-spacing"));
        }
        ui.label(
            RichText::new("Spacing isn't checked against ProPresenter 6 yet; using it makes the presentation OpenPresenter-only.")
                .small()
                .weak(),
        );
    });
    if ui
        .selectable_label(st.edit.tab == Tab::Effects, "Effects")
        .on_hover_text("Outline and drop shadow")
        .clicked()
    {
        st.edit.tab = Tab::Effects;
    }
}

/// Font list with search and a preview of each font. Returns the family picked.
fn font_picker(ui: &mut egui::Ui, st: &mut UiState, current: &str) -> Option<String> {
    let mut picked = None;
    ui.set_min_width(280.0);
    let r = ui.add(
        egui::TextEdit::singleline(&mut st.edit.font_search)
            .hint_text("Search fonts")
            .desired_width(260.0),
    );
    r.request_focus();
    if st.families.is_empty() {
        ui.label(RichText::new("Loading fonts…").weak());
        return None;
    }
    let q = st.edit.font_search.to_lowercase();
    let list: Vec<String> = st
        .families
        .iter()
        .filter(|f| q.is_empty() || f.to_lowercase().contains(&q))
        .cloned()
        .collect();
    let row_h = 30.0;
    egui::ScrollArea::vertical()
        .max_height(380.0)
        .show_rows(ui, row_h, list.len(), |ui, range| {
            for fam in &list[range] {
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(270.0, row_h), Sense::click());
                if fam == current {
                    ui.painter()
                        .rect_filled(rect, 3.0, Color32::from_rgb(50, 80, 130));
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 3.0, Color32::from_gray(55));
                }
                // Preview: the name drawn in the font itself (made once, kept).
                if !st.edit.font_previews.contains_key(fam) {
                    let tex = st
                        .fonts
                        .as_mut()
                        .and_then(|f| f.sample(fam, fam, 18.0))
                        .map(|(w, h, px)| {
                            ui.ctx().load_texture(
                                format!("font-{fam}"),
                                egui::ColorImage::from_rgba_unmultiplied(
                                    [w as usize, h as usize],
                                    &px,
                                ),
                                egui::TextureOptions::LINEAR,
                            )
                        });
                    st.edit.font_previews.insert(fam.clone(), tex);
                }
                match st.edit.font_previews.get(fam).and_then(Option::as_ref) {
                    Some(t) => {
                        let size = t.size_vec2();
                        let scale = ((rect.width() - 12.0) / size.x).min(1.0);
                        let r = Rect::from_min_size(
                            Pos2::new(rect.left() + 6.0, rect.center().y - size.y * scale / 2.0),
                            size * scale,
                        );
                        ui.painter().image(
                            t.id(),
                            r,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    None => {
                        ui.painter().text(
                            rect.left_center() + Vec2::new(6.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            fam,
                            egui::FontId::proportional(14.0),
                            Color32::WHITE,
                        );
                    }
                }
                if resp.on_hover_text(fam).clicked() {
                    picked = Some(fam.clone());
                }
            }
        });
    picked
}

// ───────────────────────────── effects ─────────────────────────────

/// The Effects panel (in the inspector column): outline and drop shadow.
pub(super) fn effects_tab(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    (g, s, e): (usize, usize, usize),
) {
    if ui.button("‹ Back").clicked() {
        st.edit.tab = Tab::Text;
    }
    let Some(t) = text_box(d, g, s, e) else {
        ui.label(RichText::new("Select a text box on the canvas.").weak());
        return;
    };
    let to_rgba = |c: Color32, a: f64| doc::Rgba {
        r: f64::from(c.r()) / 255.0,
        g: f64::from(c.g()) / 255.0,
        b: f64::from(c.b()) / 255.0,
        a,
    };
    let col = |c: doc::Rgba| {
        let [r, g, b, _] = c.to_rgba8();
        Color32::from_rgb(r, g, b)
    };
    let key = |what: &str| Some(format!("{what}:{}", t.uuid));

    ui.add_space(6.0);
    ui.heading("Outline");
    let mut on = t.outline.is_some();
    let o = t.outline.unwrap_or(doc::Outline {
        width: 3.0,
        color: doc::Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        },
    });
    let mut new = None;
    if ui.checkbox(&mut on, "Outline the text").changed() {
        new = Some(on.then_some(o));
    }
    if let Some(cur) = t.outline {
        let mut w = cur.width;
        ui.horizontal(|ui| {
            ui.label("Width");
            if ui
                .add(
                    egui::Slider::new(&mut w, 0.5..=30.0)
                        .step_by(0.5)
                        .suffix(" px"),
                )
                .changed()
            {
                new = Some(Some(doc::Outline { width: w, ..cur }));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Colour");
            let mut c = col(cur.color);
            if egui::color_picker::color_edit_button_srgba(
                ui,
                &mut c,
                egui::color_picker::Alpha::Opaque,
            )
            .changed()
            {
                new = Some(Some(doc::Outline {
                    color: to_rgba(c, 1.0),
                    ..cur
                }));
            }
        });
    }
    if let Some(outline) = new {
        send_checked(
            st,
            actions,
            path,
            d,
            Op::SetOutline {
                group: g,
                slide: s,
                element: e,
                outline,
            },
            key("outline"),
        );
    }

    ui.add_space(12.0);
    ui.heading("Drop shadow");
    let mut on = t.shadow.is_some();
    let base = t.shadow.unwrap_or(doc::Shadow {
        color: doc::Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.6,
        },
        blur: 6.0,
        offset: doc::Shadow::from_polar(45.0, 6.0),
    });
    let mut new = None;
    if ui.checkbox(&mut on, "Shadow behind the text").changed() {
        new = Some(on.then_some(base));
    }
    if let Some(cur) = t.shadow {
        let (mut angle, mut dist) = cur.polar();
        let mut blur = cur.blur;
        let mut opacity = cur.color.a * 100.0;
        egui::Grid::new("shadow-grid")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Colour");
                let mut c = col(cur.color);
                if egui::color_picker::color_edit_button_srgba(
                    ui,
                    &mut c,
                    egui::color_picker::Alpha::Opaque,
                )
                .changed()
                {
                    new = Some(Some(doc::Shadow {
                        color: to_rgba(c, cur.color.a),
                        ..cur
                    }));
                }
                ui.end_row();
                ui.label("Opacity");
                if ui
                    .add(egui::Slider::new(&mut opacity, 0.0..=100.0).suffix(" %"))
                    .changed()
                {
                    new = Some(Some(doc::Shadow {
                        color: doc::Rgba {
                            a: opacity / 100.0,
                            ..cur.color
                        },
                        ..cur
                    }));
                }
                ui.end_row();
                ui.label("Angle");
                let a = ui.add(egui::Slider::new(&mut angle, 0.0..=359.0).suffix("°"));
                ui.end_row();
                ui.label("Distance");
                let dd = ui.add(egui::Slider::new(&mut dist, 0.0..=100.0).suffix(" px"));
                if a.changed() || dd.changed() {
                    new = Some(Some(doc::Shadow {
                        offset: doc::Shadow::from_polar(angle, dist),
                        ..cur
                    }));
                }
                ui.end_row();
                ui.label("Blur");
                if ui
                    .add(egui::Slider::new(&mut blur, 0.0..=50.0).suffix(" px"))
                    .changed()
                {
                    new = Some(Some(doc::Shadow { blur, ..cur }));
                }
                ui.end_row();
            });
    }
    if let Some(shadow) = new {
        send_checked(
            st,
            actions,
            path,
            d,
            Op::SetShadow {
                group: g,
                slide: s,
                element: e,
                shadow,
            },
            key("shadow"),
        );
    }
    ui.add_space(10.0);
    ui.label(
        RichText::new("Outline and shadow aren't checked against ProPresenter 6 yet; using them makes the presentation OpenPresenter-only.")
            .small()
            .weak(),
    );
}

/// Test hook: make the whole box bold (or not) as the toolbar's B button does.
#[cfg(test)]
pub(super) fn apply_for_test(
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    at: (usize, usize, usize),
    bold: bool,
) {
    apply(st, actions, path, d, at, Change::Bold(bold), None);
}
