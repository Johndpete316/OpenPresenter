//! Themes in the editor (docs/EDITOR-PLAN.md E7): the Themes window (make a theme
//! from the slide, apply it to the slide / group / presentation or to many
//! presentations from the library, edit, delete) and the "restyle linked slides"
//! confirmation after a theme is saved.
//!
//! Theme files are read and written on worker threads; applying to presentations
//! that aren't open is done by the core (it loads them, then edits each one's
//! editor copy: one undo step per presentation).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use doc::{Edit as Op, Theme};
use egui::{Color32, RichText};
use pp6core::{Command, ShowState, ThemeTargets};

use super::text::send_checked;
use crate::ui::{UiAction, UiState};

/// Native presentations with slides linked to a theme: (path, linked slides).
type Linked = Vec<(PathBuf, usize)>;

/// Folder for themes inside a library folder.
pub const THEMES_DIR: &str = "Themes";

enum Job {
    Listed(Vec<Theme>, Vec<String>),
    Saved(Box<Result<Theme, String>>),
    /// Native presentations linked to a saved theme: (path, linked slides).
    Linked(Arc<Theme>, Linked),
}

pub struct Themes {
    pub open: bool,
    list: Vec<Theme>,
    selected: Option<String>,
    new_name: String,
    many: BTreeSet<PathBuf>,
    search: String,
    tx: Sender<Job>,
    rx: Receiver<Job>,
    busy: usize,
    /// Presentation to go back to when theme editing ends.
    pub return_to: Option<PathBuf>,
    /// After saving an edited theme: linked slides to restyle, waiting for "yes".
    relink: Option<(Arc<Theme>, Linked)>,
}

impl Default for Themes {
    fn default() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Themes {
            open: false,
            list: Vec::new(),
            selected: None,
            new_name: String::new(),
            many: BTreeSet::new(),
            search: String::new(),
            tx,
            rx,
            busy: 0,
            return_to: None,
            relink: None,
        }
    }
}

fn spawn(st: &mut UiState, ctx: &egui::Context, job: impl FnOnce() -> Job + Send + 'static) {
    let tx = st.edit.themes.tx.clone();
    let ctx = ctx.clone();
    st.edit.themes.busy += 1;
    let r = std::thread::Builder::new()
        .name("themes".into())
        .spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
    if let Err(e) = r {
        st.edit.themes.busy -= 1;
        st.toast(format!("Couldn't start working on themes: {e}"));
    }
}

/// Read every theme in `dir` (worker thread).
fn list(dir: Option<PathBuf>) -> Job {
    let mut themes = Vec::new();
    let mut errors = Vec::new();
    if let Some(dir) = dir
        && let Ok(rd) = std::fs::read_dir(&dir)
    {
        for e in rd.flatten() {
            let p = e.path();
            if doc::theme::is_theme(&p) {
                match Theme::load(&p) {
                    Ok(t) => themes.push(t),
                    Err(e) => errors.push(e.user_message()),
                }
            }
        }
    }
    themes.sort_by_key(|t| t.name.to_lowercase());
    Job::Listed(themes, errors)
}

/// Native presentations under `dirs` with slides linked to `theme` (worker thread).
fn linked(theme: Arc<Theme>, dirs: Vec<PathBuf>) -> Job {
    let mut out = Vec::new();
    let mut stack = dirs;
    let mut seen = 0usize;
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
                {
                    stack.push(p);
                }
                continue;
            }
            seen += 1;
            if seen > 50_000 || !doc::is_native(&p) {
                continue;
            }
            // Cheap check before parsing.
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            if !text.contains(&theme.uuid) {
                continue;
            }
            if let Ok(d) = doc::file::from_json(text.as_bytes(), &p) {
                let n = d
                    .slides()
                    .filter(|s| s.theme.as_ref().is_some_and(|l| l.uuid == theme.uuid))
                    .count();
                if n > 0 {
                    out.push((p, n));
                }
            }
        }
    }
    Job::Linked(theme, out)
}

/// Worker results; called every frame while the editor is open.
pub fn poll(st: &mut UiState) {
    while let Ok(job) = st.edit.themes.rx.try_recv() {
        st.edit.themes.busy = st.edit.themes.busy.saturating_sub(1);
        match job {
            Job::Listed(list, errors) => {
                st.edit.themes.list = list;
                for e in errors {
                    st.toast(e);
                }
            }
            Job::Saved(r) if r.is_ok() => {
                let Ok(t) = *r else { continue };
                st.edit.themes.selected = Some(t.uuid.clone());
                st.edit.themes.list.retain(|x| x.uuid != t.uuid);
                st.edit.themes.list.push(t);
                st.edit.themes.list.sort_by_key(|t| t.name.to_lowercase());
            }
            Job::Saved(r) => {
                if let Err(e) = *r {
                    st.toast(e);
                }
            }
            Job::Linked(theme, found) => {
                if !found.is_empty() {
                    st.edit.themes.relink = Some((theme, found));
                }
            }
        }
    }
}

/// Open the Themes window (and read the theme list).
pub fn open(st: &mut UiState, ctx: &egui::Context) {
    st.edit.themes.open = true;
    let dir = st.edit.themes_dir.clone();
    spawn(st, ctx, move || list(dir));
}

/// After saving a theme being edited: look for linked slides to restyle.
pub fn after_save(st: &mut UiState, ctx: &egui::Context, d: &doc::Document) {
    if d.theme.is_none() {
        return;
    }
    match Theme::from_document(d) {
        Ok(t) => {
            let dirs = st.edit.library_dirs.clone();
            spawn(st, ctx, move || linked(Arc::new(t), dirs));
        }
        Err(e) => st.toast(e.user_message()),
    }
}

fn apply_here(
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    theme: &Theme,
    slides: Vec<(usize, usize)>,
    all: bool,
) {
    let becomes_native = theme.features().into_iter().any(doc::needs_native);
    let op = Op::ApplyTheme {
        slides,
        theme: Box::new(theme.for_document(&d.path)),
        all,
        link: d.mode == doc::Mode::Native || becomes_native,
    };
    send_checked(st, actions, path, d, op, None);
}

/// The Themes window.
pub fn window(
    ctx: &egui::Context,
    st: &mut UiState,
    show: &ShowState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
) {
    relink_prompt(ctx, st, actions);
    if !st.edit.themes.open {
        return;
    }
    let mut open = true;
    egui::Window::new("Themes")
        .open(&mut open)
        .default_width(560.0)
        .default_height(480.0)
        .collapsible(false)
        .show(ctx, |ui| window_body(ui, st, show, actions, path, d));
    if !open {
        st.edit.themes.open = false;
    }
}

fn window_body(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
) {
    let Some(dir) = st.edit.themes_dir.clone() else {
        ui.label("Add a library folder in Settings → Folders first: themes are kept in its Themes folder.");
        return;
    };
    ui.label(
        RichText::new(format!("Kept in {}", dir.display()))
            .small()
            .weak(),
    );
    let (g, s) = st.edit.sel.unwrap_or((0, 0));
    ui.horizontal(|ui| {
        ui.label("New theme from this slide:");
        ui.add(
            egui::TextEdit::singleline(&mut st.edit.themes.new_name)
                .hint_text("Theme name")
                .desired_width(180.0),
        );
        let name = st.edit.themes.new_name.trim().to_owned();
        if ui
            .add_enabled(
                !name.is_empty() && d.theme.is_none(),
                egui::Button::new("Create"),
            )
            .on_hover_text("Its canvas size, background, first text box's place and text style")
            .clicked()
        {
            match Theme::from_slide(&name, d, (g, s)) {
                Ok(t) => {
                    let file = dir.join(doc::theme::file_name(&name));
                    st.edit.themes.new_name.clear();
                    spawn(st, ui.ctx(), move || {
                        Job::Saved(Box::new(if file.exists() {
                            Err(format!(
                                "A theme file named '{}' already exists. Choose another name.",
                                file.display()
                            ))
                        } else {
                            t.save(&file).map_err(|e| e.user_message())
                        }))
                    });
                }
                Err(e) => st.toast(e.user_message()),
            }
        }
    });
    ui.separator();
    if st.edit.themes.list.is_empty() {
        ui.label(
            RichText::new(if st.edit.themes.busy > 0 {
                "Reading themes…"
            } else {
                "No themes yet. Make one from a slide above."
            })
            .weak(),
        );
        return;
    }
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        egui::ScrollArea::vertical()
            .id_salt("theme-list")
            .max_height(360.0)
            .show(ui, |ui| {
                for t in &st.edit.themes.list {
                    let sel = st.edit.themes.selected.as_deref() == Some(&t.uuid);
                    let label = format!(
                        "{}  ({}×{}, {} {:.0})",
                        t.name, t.canvas.width, t.canvas.height, t.text.run.font, t.text.run.size
                    );
                    if ui.selectable_label(sel, label).clicked() {
                        st.edit.themes.selected = Some(t.uuid.clone());
                    }
                }
            });
        let ui = &mut cols[1];
        let Some(theme) = st
            .edit
            .themes
            .selected
            .as_ref()
            .and_then(|u| st.edit.themes.list.iter().find(|t| &t.uuid == u))
            .cloned()
        else {
            ui.label(RichText::new("Select a theme.").weak());
            return;
        };
        theme_actions(ui, st, show, actions, path, d, &theme, (g, s));
    });
}

#[allow(clippy::too_many_arguments)]
fn theme_actions(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    theme: &Theme,
    (g, s): (usize, usize),
) {
    ui.label(RichText::new(&theme.name).strong());
    let native: Vec<&str> = theme
        .features()
        .into_iter()
        .filter(|f| doc::needs_native(*f))
        .map(doc::Feature::name)
        .collect();
    if !native.is_empty() {
        ui.label(
            RichText::new(format!(
                "Uses {} — ProPresenter 6 presentations it's applied to will be saved as OpenPresent files (.opres).",
                native.join(", ")
            ))
            .small()
            .color(Color32::from_rgb(255, 190, 120)),
        );
    }
    let editing_theme = d.theme.is_some();
    ui.add_enabled_ui(!editing_theme, |ui| {
        ui.label("Apply to");
        ui.horizontal_wrapped(|ui| {
            if ui.button("This slide").clicked() {
                apply_here(st, actions, path, d, theme, vec![(g, s)], false);
            }
            if ui.button("This group").clicked() {
                let slides = (0..d.groups[g].slides.len()).map(|i| (g, i)).collect();
                apply_here(st, actions, path, d, theme, slides, false);
            }
            if ui
                .button("This presentation")
                .on_hover_text("Every slide; a different canvas size rescales the presentation")
                .clicked()
            {
                apply_here(st, actions, path, d, theme, Vec::new(), true);
            }
        });
    });
    if d.mode == doc::Mode::Pro6 {
        ui.label(
            RichText::new("In a ProPresenter 6 presentation the theme's styles are copied (no link to the theme).")
                .small()
                .weak(),
        );
    }
    ui.add_space(6.0);
    egui::CollapsingHeader::new("Apply to presentations in the library…")
        .id_salt("theme-many")
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut st.edit.themes.search)
                    .hint_text("Search")
                    .desired_width(f32::INFINITY),
            );
            let q = st.edit.themes.search.to_lowercase();
            egui::ScrollArea::vertical()
                .id_salt("theme-many-list")
                .max_height(180.0)
                .show(ui, |ui| {
                    for e in st
                        .library
                        .entries
                        .iter()
                        .filter(|e| e.error.is_none() && e.title.to_lowercase().contains(&q))
                    {
                        let mut on = st.edit.themes.many.contains(&e.path);
                        if ui.checkbox(&mut on, &e.title).changed() {
                            if on {
                                st.edit.themes.many.insert(e.path.clone());
                            } else {
                                st.edit.themes.many.remove(&e.path);
                            }
                        }
                    }
                });
            let n = st.edit.themes.many.len();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        n > 0,
                        egui::Button::new(format!("Apply to {n} presentation(s)")),
                    )
                    .on_hover_text(
                        "Each one changes as one undo step and stays unsaved until saved",
                    )
                    .clicked()
                {
                    actions.push(UiAction::Core(Command::ApplyTheme {
                        presentations: st.edit.themes.many.iter().cloned().collect(),
                        theme: Arc::new(theme.clone()),
                        targets: ThemeTargets::All,
                        save: false,
                    }));
                    st.edit.themes.many.clear();
                }
                let unsaved: Vec<PathBuf> = show.unsaved.iter().cloned().collect();
                if ui
                    .add_enabled(
                        !unsaved.is_empty(),
                        egui::Button::new(format!("Save all changed ({})", unsaved.len())),
                    )
                    .clicked()
                {
                    for p in unsaved {
                        actions.push(UiAction::Core(Command::Save(p)));
                    }
                }
            });
        });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!editing_theme, egui::Button::new("Edit theme"))
            .on_hover_text(
                "Edit it on a sample slide; saving can restyle the slides that follow it",
            )
            .clicked()
        {
            st.edit.themes.return_to = Some(path.to_path_buf());
            st.edit.themes.open = false;
            st.edit.sel = None;
            st.edit.element = None;
            st.edit.multi.clear();
            actions.push(UiAction::Core(Command::Open(theme.path.clone())));
        }
        if ui.button("Delete theme").clicked() {
            let p = theme.path.clone();
            let deleted = p.parent().map(|d| d.join("deleted"));
            let r = deleted
                .ok_or_else(|| "no folder".to_owned())
                .and_then(|dir| {
                    std::fs::create_dir_all(&dir)
                        .and_then(|()| {
                            std::fs::rename(&p, dir.join(p.file_name().unwrap_or_default()))
                        })
                        .map_err(|e| e.to_string())
                });
            match r {
                Ok(()) => {
                    st.edit.themes.list.retain(|t| t.uuid != theme.uuid);
                    st.edit.themes.selected = None;
                }
                Err(e) => st.toast(format!(
                    "The theme couldn't be moved to the 'deleted' folder ({e})."
                )),
            }
        }
    });
}

/// "Editing this theme restyles N slides in M presentations."
fn relink_prompt(ctx: &egui::Context, st: &mut UiState, actions: &mut Vec<UiAction>) {
    let Some((theme, found)) = st.edit.themes.relink.clone() else {
        return;
    };
    let slides: usize = found.iter().map(|(_, n)| n).sum();
    let mut answer = None;
    egui::Modal::new(egui::Id::unique("theme-relink")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.heading(format!("Restyle slides using “{}”?", theme.name));
        ui.label(format!(
            "{slides} slide(s) in {} presentation(s) follow this theme. Restyle them to match it now? Each presentation is saved afterwards.",
            found.len()
        ));
        for (p, n) in found.iter().take(8) {
            ui.label(
                RichText::new(format!("• {} ({n})", crate::ui::title(p)))
                    .small()
                    .weak(),
            );
        }
        if found.len() > 8 {
            ui.label(RichText::new(format!("… and {} more", found.len() - 8)).small().weak());
        }
        ui.horizontal(|ui| {
            if ui.button("Restyle them").clicked() {
                answer = Some(true);
            }
            if ui.button("Not now").clicked() {
                answer = Some(false);
            }
        });
    });
    match answer {
        Some(true) => {
            actions.push(UiAction::Core(Command::ApplyTheme {
                presentations: found.into_iter().map(|(p, _)| p).collect(),
                theme,
                targets: ThemeTargets::Linked,
                save: true,
            }));
            st.edit.themes.relink = None;
        }
        Some(false) => st.edit.themes.relink = None,
        None => {}
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name)
    }

    #[test]
    fn linked_presentations_are_found_by_theme() {
        let dir = std::env::temp_dir().join(format!("pp6-themes-linked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let mut d = doc::load(&fixture("song_1024x768.pro6")).unwrap();
        let theme = Theme::from_slide("T", &d, (0, 0)).unwrap();
        Op::ApplyTheme {
            slides: vec![(0, 0), (0, 1)],
            theme: Box::new(theme.clone()),
            all: false,
            link: true,
        }
        .apply(&mut d)
        .unwrap();
        d.mode = doc::Mode::Native;
        d.path = dir.join("sub").join("song.opres");
        doc::save(&d).unwrap();
        let other = Theme::from_slide("Other", &d, (0, 0)).unwrap();
        let Job::Linked(_, found) = linked(Arc::new(theme), vec![dir.clone()]) else {
            panic!()
        };
        assert_eq!(found, vec![(dir.join("sub").join("song.opres"), 2)]);
        let Job::Linked(_, none) = linked(Arc::new(other), vec![dir.clone()]) else {
            panic!()
        };
        assert!(none.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn themes_in_a_folder_are_listed_by_name() {
        let dir = std::env::temp_dir().join(format!("pp6-themes-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = doc::load(&fixture("song_1024x768.pro6")).unwrap();
        for n in ["b", "A"] {
            Theme::from_slide(n, &d, (0, 0))
                .unwrap()
                .save(&dir.join(doc::theme::file_name(n)))
                .unwrap();
        }
        std::fs::write(dir.join("broken.optheme"), "{").unwrap();
        let Job::Listed(list, errors) = list(Some(dir.clone())) else {
            panic!()
        };
        assert_eq!(
            list.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["A", "b"]
        );
        assert_eq!(errors.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
