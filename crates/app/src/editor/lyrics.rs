//! Paste Lyrics (docs/EDITOR-PLAN.md E8): a full-height text view of the
//! presentation as lyrics (`[Group]` lines, blank lines between slides) with a live
//! preview beside it. Apply replaces or appends to the presentation (one undo
//! step), or makes a new presentation in the library.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::Sender;
use doc::lyrics::{LyricsMode, parse, to_text};
use doc::{Edit as Op, Theme};
use egui::{Color32, RichText, Vec2};

use super::text::send_checked;
use crate::thumbs::ThumbRequest;
use crate::ui::{UiAction, UiState, color32, paint_error};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Target {
    #[default]
    Replace,
    Append,
    New,
}

/// Rebuild the preview this long after the last keystroke.
const PREVIEW_AFTER: f64 = 0.35;

#[derive(Default)]
pub struct Lyrics {
    pub open: bool,
    text: String,
    target: Target,
    new_name: String,
    /// Theme for new slides (by UUID); `None` = the presentation's own style.
    theme: Option<String>,
    /// Text the preview was built from, and when the text last changed.
    preview_for: Option<String>,
    changed_at: f64,
    preview: Option<Result<Arc<doc::Document>, String>>,
}

/// Open with the presentation as text.
pub fn open(st: &mut UiState, ctx: &egui::Context, d: &doc::Document) {
    let l = &mut st.edit.lyrics;
    l.open = true;
    l.text = to_text(d);
    l.preview_for = None;
    l.preview = None;
    l.target = Target::Replace;
    l.new_name.clear();
    if st.edit.themes.is_empty() {
        super::themes::refresh(st, ctx);
    }
}

fn colors(st: &UiState) -> Vec<(String, doc::Rgba)> {
    st.edit
        .group_colors
        .iter()
        .map(|g| {
            let f = |v: u8| f64::from(v) / 255.0;
            (
                g.name.clone(),
                doc::Rgba {
                    r: f(g.color[0]),
                    g: f(g.color[1]),
                    b: f(g.color[2]),
                    a: 1.0,
                },
            )
        })
        .collect()
}

fn chosen_theme(st: &UiState, doc_path: &Path) -> Option<Box<Theme>> {
    let uuid = st.edit.lyrics.theme.as_ref()?;
    st.edit
        .themes
        .find(uuid)
        .map(|t| Box::new(t.for_document(doc_path)))
}

/// A new presentation: one slide styled like `d`'s first text slide (or the
/// theme), then the lyrics.
fn new_presentation(
    d: &doc::Document,
    path: &Path,
    text: &str,
    colors: Vec<(String, doc::Rgba)>,
    theme: Option<Box<Theme>>,
) -> Result<doc::Document, String> {
    let template = d
        .slides()
        .find(|s| {
            s.text_boxes()
                .any(|t| t.problem.is_none() && t.runs().next().is_some())
        })
        .cloned()
        .ok_or("This presentation has no text slide to take the style from.")?;
    let mut slide = template;
    slide.src = None;
    slide.pro6_incompatible = false;
    slide.theme = None;
    for e in &mut slide.elements {
        e.set_uuid(doc::new_uuid());
        match e {
            doc::Element::Text(t) => t.src = None,
            doc::Element::Image(i) => i.src = None,
            doc::Element::Pro6(_) => {}
        }
    }
    // Raw .pro6 elements can't live in a new native file.
    slide
        .elements
        .retain(|e| !matches!(e, doc::Element::Pro6(_)));
    let mut n = doc::Document {
        path: path.to_path_buf(),
        format_version: doc::FORMAT_VERSION,
        mode: doc::Mode::Native,
        canvas: theme.as_ref().map_or(d.canvas, |t| t.canvas),
        background: d.background,
        selected_arrangement: None,
        groups: vec![doc::Group {
            uuid: doc::new_uuid(),
            name: "Template".into(),
            color: None,
            slides: vec![slide],
            src: None,
        }],
        arrangements: Vec::new(),
        cue_report: doc::CueReport::default(),
        theme: None,
        pro6: None,
    };
    if d.canvas != n.canvas {
        // Scale the copied slide to the theme's canvas.
        let mut tmp = n.clone();
        tmp.canvas = d.canvas;
        doc::theme::rescale(&mut tmp, n.canvas);
        n = tmp;
    }
    Op::SetLyrics {
        text: text.to_owned(),
        mode: LyricsMode::Replace,
        colors,
        theme,
    }
    .apply(&mut n)
    .map_err(|e| e.user_message())?;
    Ok(n)
}

fn build_preview(
    st: &UiState,
    path: &Path,
    d: &doc::Document,
) -> Result<Arc<doc::Document>, String> {
    let l = &st.edit.lyrics;
    let theme = chosen_theme(st, path);
    let colors = colors(st);
    match l.target {
        Target::New => new_presentation(d, path, &l.text, colors, theme).map(Arc::new),
        Target::Replace | Target::Append => {
            let mut p = d.clone();
            Op::SetLyrics {
                text: l.text.clone(),
                mode: if l.target == Target::Append {
                    LyricsMode::Append
                } else {
                    LyricsMode::Replace
                },
                colors,
                theme,
            }
            .apply(&mut p)
            .map_err(|e| e.user_message())?;
            Ok(Arc::new(p))
        }
    }
}

/// The Paste Lyrics window.
pub fn window(
    ctx: &egui::Context,
    st: &mut UiState,
    thumbs: &Sender<ThumbRequest>,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
) {
    if !st.edit.lyrics.open {
        return;
    }
    let now = ctx.input(|i| i.time);
    let screen = ctx.content_rect();
    let mut open = true;
    egui::Window::new("Paste Lyrics")
        .open(&mut open)
        .collapsible(false)
        .fixed_pos(screen.min + Vec2::new(40.0, 30.0))
        .fixed_size(Vec2::new(screen.width() - 80.0, screen.height() - 110.0))
        .show(ctx, |ui| body(ui, st, thumbs, actions, path, d, now));
    if !open {
        st.edit.lyrics.open = false;
    }
    // Live preview, a moment after typing stops.
    let l = &st.edit.lyrics;
    if l.preview_for.as_deref() != Some(&l.text) {
        if now - l.changed_at >= PREVIEW_AFTER {
            let p = build_preview(st, path, d);
            let l = &mut st.edit.lyrics;
            l.preview = Some(p);
            l.preview_for = Some(l.text.clone());
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(PREVIEW_AFTER));
        }
    }
}

fn body(
    ui: &mut egui::Ui,
    st: &mut UiState,
    thumbs: &Sender<ThumbRequest>,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    now: f64,
) {
    ui.horizontal(|ui| {
        let l = &mut st.edit.lyrics;
        let before = (l.target, l.theme.clone());
        ui.label("Put the lyrics in:");
        ui.selectable_value(&mut l.target, Target::Replace, "This presentation (replace)")
            .on_hover_text("The text becomes the presentation. Groups that keep their name keep their slides and look.");
        ui.selectable_value(&mut l.target, Target::Append, "This presentation (add at the end)");
        ui.selectable_value(&mut l.target, Target::New, "A new presentation");
        ui.separator();
        ui.label("New slides styled by");
        let names: Vec<(String, String)> = st
            .edit
            .themes
            .names();
        let l = &mut st.edit.lyrics;
        let current = l
            .theme
            .as_ref()
            .and_then(|u| names.iter().find(|(id, _)| id == u))
            .map_or("this presentation".to_owned(), |(_, n)| format!("theme {n}"));
        egui::ComboBox::from_id_salt("lyrics-theme")
            .selected_text(current)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut l.theme, None, "this presentation");
                for (id, n) in &names {
                    ui.selectable_value(&mut l.theme, Some(id.clone()), format!("theme {n}"));
                }
            });
        if (l.target, l.theme.clone()) != before {
            l.preview_for = None;
        }
    });
    if st.edit.lyrics.target == Target::New {
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.add(
                egui::TextEdit::singleline(&mut st.edit.lyrics.new_name)
                    .hint_text("Song title")
                    .desired_width(260.0),
            );
            match &st.edit.library_dirs.first() {
                Some(dir) => ui.label(
                    RichText::new(format!("saved in {}", dir.display()))
                        .small()
                        .weak(),
                ),
                None => ui.label(
                    RichText::new("Add a library folder in Settings first.").color(Color32::YELLOW),
                ),
            };
        });
    }
    ui.label(
        RichText::new("[Name] on its own line starts a group (a name used again repeats it); a blank line starts a new slide.")
            .small()
            .weak(),
    );
    let parsed = parse(&st.edit.lyrics.text);
    let avail = ui.available_size();
    ui.horizontal_top(|ui| {
        // Text, full height.
        ui.vertical(|ui| {
            ui.set_width(avail.x * 0.45);
            egui::ScrollArea::vertical()
                .id_salt("lyrics-text")
                .max_height(avail.y - 40.0)
                .show(ui, |ui| {
                    let r = ui.add(
                        egui::TextEdit::multiline(&mut st.edit.lyrics.text)
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(((avail.y - 60.0) / 16.0).max(10.0) as usize),
                    );
                    if r.changed() {
                        st.edit.lyrics.changed_at = now;
                    }
                });
        });
        ui.separator();
        // Preview.
        ui.vertical(|ui| {
            ui.set_width(avail.x * 0.52);
            let order: Vec<&str> = parsed
                .order
                .iter()
                .map(|&g| parsed.groups[g].name.as_str())
                .collect();
            ui.label(format!(
                "{} group(s), {} slide(s). Order: {}",
                parsed.groups.len(),
                parsed.slide_count(),
                if order.is_empty() { "—".to_owned() } else { order.join(" → ") }
            ));
            if parsed.ignored_lines > 0 {
                ui.label(
                    RichText::new(format!(
                        "{} line(s) under a repeated name are ignored (the group already has its slides).",
                        parsed.ignored_lines
                    ))
                    .small()
                    .color(Color32::from_rgb(255, 190, 120)),
                );
            }
            let blank = d
                .slides()
                .filter(|s| s.text_boxes().all(|t| t.plain_text().trim().is_empty()))
                .count();
            if st.edit.lyrics.target == Target::Replace && blank > 0 {
                ui.label(
                    RichText::new(format!(
                        "{blank} slide(s) without text aren't shown as lyrics and will be removed by Replace."
                    ))
                    .small()
                    .color(Color32::from_rgb(255, 190, 120)),
                );
            }
            let preview = st.edit.lyrics.preview.clone();
            match preview {
                None => {
                    ui.label(RichText::new("Preparing preview…").weak());
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(Color32::from_rgb(255, 130, 130)));
                }
                Some(Ok(p)) => {
                    egui::ScrollArea::vertical()
                        .id_salt("lyrics-preview")
                        .max_height(avail.y - 90.0)
                        .show(ui, |ui| preview_grid(ui, st, thumbs, &p));
                }
            }
        });
    });
    ui.separator();
    ui.horizontal(|ui| {
        let ready = !parsed.groups.is_empty()
            && (st.edit.lyrics.target != Target::New
                || (!st.edit.lyrics.new_name.trim().is_empty()
                    && !st.edit.library_dirs.is_empty()));
        if ui
            .add_enabled(ready, egui::Button::new(RichText::new("Apply").strong()))
            .clicked()
        {
            apply(st, ui.ctx(), actions, path, d);
        }
        if ui.button("Cancel").clicked() {
            st.edit.lyrics.open = false;
        }
    });
}

fn preview_grid(
    ui: &mut egui::Ui,
    st: &mut UiState,
    thumbs: &Sender<ThumbRequest>,
    p: &Arc<doc::Document>,
) {
    let tile_w = 150.0;
    let tile_h = tile_w * p.canvas.height as f32 / p.canvas.width.max(1) as f32;
    let mut flat = 0;
    for g in &p.groups {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::new(12.0, 12.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 2.0, color32(g.color));
            ui.label(RichText::new(&g.name).strong());
        });
        ui.horizontal_wrapped(|ui| {
            for _ in &g.slides {
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(tile_w, tile_h), egui::Sense::hover());
                let (_, tex) = st.thumb_public(thumbs, p, flat);
                match tex {
                    Some(Ok(t)) => {
                        egui::Image::new(t).paint_at(ui, rect);
                    }
                    Some(Err(e)) => paint_error(ui, rect, e),
                    None => {
                        ui.painter().rect_filled(rect, 0.0, Color32::from_gray(18));
                    }
                }
                ui.painter().rect_stroke(
                    rect,
                    2.0,
                    egui::Stroke::new(1.0, color32(g.color)),
                    egui::StrokeKind::Outside,
                );
                flat += 1;
            }
        });
        ui.add_space(4.0);
    }
}

fn apply(
    st: &mut UiState,
    ctx: &egui::Context,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
) {
    let theme = chosen_theme(st, path);
    let colors = colors(st);
    let text = st.edit.lyrics.text.clone();
    match st.edit.lyrics.target {
        Target::Replace | Target::Append => {
            let op = Op::SetLyrics {
                text,
                mode: if st.edit.lyrics.target == Target::Append {
                    LyricsMode::Append
                } else {
                    LyricsMode::Replace
                },
                colors,
                theme,
            };
            send_checked(st, actions, path, d, op, None);
            st.edit.sel = None;
            st.edit.element = None;
            st.edit.lyrics.open = false;
        }
        Target::New => {
            let Some(dir) = st.edit.library_dirs.first().cloned() else {
                return;
            };
            let name = st.edit.lyrics.new_name.trim().to_owned();
            let file: PathBuf = dir.join(format!(
                "{}.{}",
                name.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_"),
                doc::NATIVE_EXT
            ));
            if file.exists() || file.with_extension("pro6").exists() {
                st.toast(format!(
                    "A presentation named '{name}' already exists in {}. Choose another name.",
                    dir.display()
                ));
                return;
            }
            let theme = theme.map(|t| Box::new(t.for_document(&file)));
            match new_presentation(d, &file, &text, colors, theme) {
                Ok(n) => {
                    // Written on a worker thread, then opened in the editor.
                    let tx_ctx = ctx.clone();
                    let (done_tx, done_rx) = crossbeam_channel::bounded(1);
                    let spawned = std::thread::Builder::new()
                        .name("new-presentation".into())
                        .spawn(move || {
                            let _ = done_tx
                                .send(doc::save(&n).map(|s| s.path).map_err(|e| e.user_message()));
                            tx_ctx.request_repaint();
                        });
                    match spawned {
                        Ok(_) => st.edit.lyrics_new = Some(done_rx),
                        Err(e) => st.toast(format!("Couldn't save the new presentation: {e}")),
                    }
                    st.edit.lyrics.open = false;
                }
                Err(e) => st.toast(e),
            }
        }
    }
}

/// A new presentation finished saving: open it in the editor.
pub fn poll(st: &mut UiState, actions: &mut Vec<UiAction>) {
    let Some(rx) = &st.edit.lyrics_new else {
        return;
    };
    let Ok(r) = rx.try_recv() else { return };
    st.edit.lyrics_new = None;
    match r {
        Ok(p) => {
            st.edit.sel = None;
            st.edit.element = None;
            actions.push(UiAction::RefreshLibrary);
            actions.push(UiAction::Core(opcore::Command::Open(p)));
        }
        Err(e) => st.toast(e),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_new_presentation_from_lyrics_saves_as_opres_in_the_presentations_style() {
        let src =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/song_1024x768.pro6");
        let d = doc::load(&src).unwrap();
        let dir = std::env::temp_dir().join(format!("openpresenter-lyrics-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("New Song.opres");
        let text = "[Verse]\nLine one\nLine two\n\n[Chorus]\nSing it\n\n[Verse]\n";
        let n = new_presentation(&d, &file, text, Vec::new(), None).unwrap();
        assert_eq!(n.mode, doc::Mode::Native);
        let names: Vec<&str> = n.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Verse", "Chorus"], "the template group is replaced");
        assert_eq!(to_text(&n), text);
        let style = d
            .text_boxes()
            .next()
            .unwrap()
            .runs()
            .next()
            .unwrap()
            .font
            .clone();
        assert!(n.text_boxes().all(|t| t.runs().all(|r| r.font == style)));
        let saved = doc::save(&n).unwrap();
        assert_eq!(saved.path, file);
        let back = doc::load(&file).unwrap();
        assert_eq!(to_text(&back), text);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
