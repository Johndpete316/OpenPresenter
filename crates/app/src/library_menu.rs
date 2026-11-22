//! The library's right-click menu: open in the editor, rename, duplicate, show in
//! the folder, add to a playlist, delete (to the Recycle Bin / Trash). Renaming and
//! deleting tell the core ([`Command::PresentationMoved`]) so playlists follow.

use std::path::{Path, PathBuf};

use egui::{Color32, RichText};
use opcore::{Command, ShowState};

use crate::ui::{UiAction, UiState};

#[derive(Default)]
pub struct LibraryMenu {
    /// Being renamed: (file, name typed so far).
    rename: Option<(PathBuf, String)>,
    /// Asked to delete.
    delete: Option<PathBuf>,
}

fn title(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Why `path` can't be renamed or deleted now, if it can't.
fn busy(show: &ShowState, path: &Path) -> Option<&'static str> {
    if show.is_live(path) {
        Some("It's live: show something else (or clear) first.")
    } else if show.unsaved.contains(path) {
        Some("It has unsaved edits: save or revert them first.")
    } else {
        None
    }
}

/// A menu choice, carried out by [`pick`] once the library list is drawn.
pub enum Pick {
    Edit,
    Rename,
    Duplicate,
    ShowInFolder,
    Delete,
}

/// The menu items for one library row (inside its context menu).
pub fn items(ui: &mut egui::Ui, show: &ShowState, path: &Path, readable: bool) -> Option<Pick> {
    let busy = busy(show, path);
    let mut out = None;
    if ui
        .add_enabled(readable, egui::Button::new("Open in editor"))
        .clicked()
    {
        out = Some(Pick::Edit);
    }
    let r = ui.add_enabled(busy.is_none(), egui::Button::new("Rename…"));
    match busy {
        Some(why) => {
            r.on_disabled_hover_text(why);
        }
        None if r.clicked() => out = Some(Pick::Rename),
        None => {}
    }
    if ui
        .button("Duplicate")
        .on_hover_text("A copy next to it, then name the copy")
        .clicked()
    {
        out = Some(Pick::Duplicate);
    }
    if ui.button("Show in folder").clicked() {
        out = Some(Pick::ShowInFolder);
    }
    if out.is_some() {
        ui.close();
    }
    out
}

/// The Delete item, last in the menu (after the playlist items).
pub fn delete_item(ui: &mut egui::Ui, show: &ShowState, path: &Path) -> Option<Pick> {
    let busy = busy(show, path);
    let r = ui.add_enabled(
        busy.is_none(),
        egui::Button::new(RichText::new("Delete…").color(Color32::from_rgb(255, 140, 140))),
    );
    match busy {
        Some(why) => {
            r.on_disabled_hover_text(why);
            None
        }
        None if r.clicked() => {
            ui.close();
            Some(Pick::Delete)
        }
        None => None,
    }
}

/// Carry out a menu choice for `path`.
pub fn pick(st: &mut UiState, pick: Pick, path: &Path, actions: &mut Vec<UiAction>) {
    match pick {
        Pick::Edit => {
            st.edit.open_when_selected = Some(path.to_path_buf());
            actions.push(UiAction::Core(Command::Open(path.to_path_buf())));
        }
        Pick::Rename => st.library_menu.rename = Some((path.to_path_buf(), title(path))),
        Pick::Duplicate => duplicate(st, path, actions),
        Pick::ShowInFolder => show_in_folder(st, path),
        Pick::Delete => st.library_menu.delete = Some(path.to_path_buf()),
    }
}

fn duplicate(st: &mut UiState, path: &Path, actions: &mut Vec<UiAction>) {
    let (Some(dir), Some(ext)) = (path.parent(), path.extension()) else {
        return;
    };
    let base = title(path);
    let copy = (1..1000)
        .map(|n| {
            let name = if n == 1 {
                format!("{base} copy")
            } else {
                format!("{base} copy {n}")
            };
            dir.join(name).with_extension(ext)
        })
        .find(|p| !p.exists());
    let Some(copy) = copy else { return };
    match std::fs::copy(path, &copy) {
        Ok(_) => {
            actions.push(UiAction::RefreshLibrary);
            st.library_menu.rename = Some((copy.clone(), title(&copy)));
        }
        Err(e) => st.toast(format!("Couldn't copy '{base}' ({e}).")),
    }
}

fn show_in_folder(st: &mut UiState, path: &Path) {
    let r = if cfg!(windows) {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
    } else {
        std::process::Command::new("xdg-open")
            .arg(path.parent().unwrap_or(Path::new(".")))
            .spawn()
    };
    if let Err(e) = r {
        st.toast(format!("Couldn't open the folder ({e})."));
    }
}

/// The new path for renaming `from` to `name`, or why not.
fn rename_target(from: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Type a name.".into());
    }
    let dir = from.parent().ok_or("No folder.")?;
    let ext = from
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    let to = dir.join(format!("{}.{ext}", crate::editor::lyrics::file_stem(name)));
    let same_file = to.to_string_lossy().to_lowercase() == from.to_string_lossy().to_lowercase();
    if !same_file {
        let other = if ext.eq_ignore_ascii_case("pro6") {
            to.with_extension(doc::NATIVE_EXT)
        } else {
            to.with_extension("pro6")
        };
        if to.exists() || other.exists() {
            return Err(format!(
                "A presentation named '{name}' already exists here."
            ));
        }
    }
    Ok(to)
}

fn do_rename(st: &mut UiState, from: &Path, to: &Path, actions: &mut Vec<UiAction>) -> bool {
    if from == to {
        return true;
    }
    match std::fs::rename(from, to) {
        Ok(()) => {
            if st.edit.on {
                // Reopens on the renamed file.
                st.edit.on = false;
                st.edit.open_when_selected = Some(to.to_path_buf());
            }
            actions.push(UiAction::Core(Command::PresentationMoved {
                from: from.to_path_buf(),
                to: Some(to.to_path_buf()),
            }));
            actions.push(UiAction::RefreshLibrary);
            true
        }
        Err(e) => {
            st.toast(format!("Couldn't rename '{}' ({e}).", title(from)));
            false
        }
    }
}

/// The Rename and Delete prompts; each frame.
pub fn dialogs(
    ctx: &egui::Context,
    st: &mut UiState,
    show: &ShowState,
    actions: &mut Vec<UiAction>,
) {
    if let Some((from, mut name)) = st.library_menu.rename.take() {
        let target = rename_target(&from, &name);
        let mut done = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::unique("library-rename")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(
                RichText::new(format!("Rename '{}'", title(&from)))
                    .size(16.0)
                    .strong(),
            );
            let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY));
            r.request_focus();
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if let Err(e) = &target {
                ui.label(RichText::new(e).color(Color32::YELLOW));
            }
            ui.label(
                RichText::new("Playlists that use it follow the new name.")
                    .small()
                    .weak(),
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        target.is_ok(),
                        egui::Button::new(RichText::new("Rename").strong()),
                    )
                    .clicked()
                    || (enter && target.is_ok())
                {
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if done && let Ok(to) = &target {
            if !do_rename(st, &from, to, actions) {
                st.library_menu.rename = Some((from, name));
            }
        } else if !(cancel || modal.should_close()) {
            st.library_menu.rename = Some((from, name));
        }
    }
    if let Some(path) = st.library_menu.delete.take() {
        let lists: Vec<&str> = show
            .playlists
            .iter()
            .filter(|p| p.playlist.items.iter().any(|i| i.path == path))
            .map(|p| p.playlist.name.as_str())
            .collect();
        let mut go = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::unique("library-delete")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(
                RichText::new(format!("Delete '{}'?", title(&path)))
                    .size(16.0)
                    .strong(),
            );
            ui.label(if cfg!(windows) {
                "It goes to the Recycle Bin, so it can be restored from there."
            } else {
                "It goes to the Trash, so it can be restored from there."
            });
            if !lists.is_empty() {
                ui.label(
                    RichText::new(format!("It's taken out of: {}.", lists.join(", ")))
                        .color(Color32::YELLOW),
                );
            }
            ui.horizontal(|ui| {
                if ui
                    .button(RichText::new("Delete").color(Color32::from_rgb(255, 140, 140)))
                    .clicked()
                {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if go {
            match trash::delete(&path) {
                Ok(()) => {
                    if st.edit.on && show.selected.as_ref() == Some(&path) {
                        st.edit.on = false;
                    }
                    actions.push(UiAction::Core(Command::PresentationMoved {
                        from: path.clone(),
                        to: None,
                    }));
                    actions.push(UiAction::RefreshLibrary);
                }
                Err(e) => st.toast(format!("Couldn't delete '{}' ({e}).", title(&path))),
            }
        } else if !(cancel || modal.should_close()) {
            st.library_menu.delete = Some(path);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn rename_keeps_the_folder_and_kind_and_refuses_taken_names() {
        let dir = std::env::temp_dir().join(format!("openpresenter-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("Amazing Grace.pro6");
        std::fs::write(&a, "x").unwrap();
        std::fs::write(dir.join("Taken.opres"), "x").unwrap();
        assert_eq!(
            rename_target(&a, " Grace: Amazing ").unwrap(),
            dir.join("Grace_ Amazing.pro6")
        );
        assert!(
            rename_target(&a, "Taken").is_err(),
            "an .opres of that name exists"
        );
        assert!(rename_target(&a, "  ").is_err());
        assert_eq!(
            rename_target(&a, "amazing grace").unwrap(),
            dir.join("amazing grace.pro6")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
