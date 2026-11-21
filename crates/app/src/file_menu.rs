//! Import / Export, in the toolbar (docs/PLAN.md, Backlog).
//!
//! Import makes a new song from typed or pasted words, copies presentations into the
//! library (a `.pro6` into the first library folder, a `.opres` into the
//! new-presentations folder; nothing is converted), opens a lyrics `.txt` in the song
//! window, or imports a playlist. Export
//! writes the selected presentation's words as text, its slides as PNG pictures, or
//! (for a `.opres` that came from a `.pro6`) a ProPresenter 6 file.

use std::path::{Path, PathBuf};

use egui::RichText;
use opcore::{Command, Settings, ShowState};

use crate::theme::{self, icon};
use crate::ui::{UiAction, UiState};

/// Background exports report here: `Ok` = done (shown as information), `Err` = a
/// problem.
pub type Report = crossbeam_channel::Receiver<Result<String, String>>;

/// The toolbar button and its menu.
pub fn button(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    settings: &Settings,
    actions: &mut Vec<UiAction>,
) {
    let resp = theme::tool_button(
        ui,
        icon::FOLDER_OPEN,
        "Import/Export",
        egui::Color32::from_rgb(110, 110, 116),
        false,
        !st.edit.on,
    )
    .on_hover_text("Bring presentations, lyrics and playlists in; save copies out");
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(300.0);
        theme::menu_heading(ui, "Import");
        if ui
            .button("New song from typed or pasted words…")
            .on_hover_text("Paste a song's words (from a lyrics site or a document); groups and slides are made from them")
            .clicked()
        {
            let selected = show
                .selected
                .as_ref()
                .is_some_and(|p| show.docs.get(p).is_some_and(|d| d.theme.is_none()));
            crate::editor::lyrics::open_new(st, ui.ctx(), selected);
            ui.close();
        }
        let lib = settings.library_dirs.first();
        if ui
            .add_enabled(lib.is_some(), egui::Button::new("Presentations (.pro6, .opres)…"))
            .on_hover_text(match lib {
                Some(d) => format!(
                    "Copied into the library: .pro6 files into {}, .opres into the new-presentations folder. The originals aren't touched.",
                    d.display()
                ),
                None => "Add a library folder in Settings first.".to_owned(),
            })
            .clicked()
        {
            import_presentations(st, settings, actions);
        }
        if ui
            .button("Song words from a text file (.txt)…")
            .on_hover_text("Opens the song window with the file's text, ready to check and save")
            .clicked()
        {
            import_lyrics(st, ui.ctx(), show);
        }
        if ui.button("Playlist…").clicked() {
            actions.push(UiAction::ImportPlaylist);
        }
        ui.add_space(4.0);
        theme::menu_heading(ui, "Export");
        let selected = show
            .selected
            .as_ref()
            .and_then(|p| show.docs.get(p).map(|d| (p.clone(), d.clone())));
        if let Some((p, _)) = &selected {
            ui.label(RichText::new(crate::ui::title(p)).small().weak());
        }
        let Some((path, d)) = selected else {
            ui.label(
                RichText::new("Select a presentation in the library or a playlist first.")
                    .small()
                    .weak(),
            );
            return;
        };
        if show.unsaved.contains(&path) {
            ui.label(
                RichText::new("The editor has unsaved changes: the saved version is exported.")
                    .small()
                    .weak(),
            );
        }
        if ui
            .button("Words as text (.txt)…")
            .on_hover_text("In the format Paste Lyrics and New presentation read back")
            .clicked()
        {
            export_text(st, &path, &d);
        }
        if ui
            .add_enabled(st.fonts.is_some(), egui::Button::new("Slides as pictures (PNG)…"))
            .on_hover_text("One picture per slide, at the presentation's size, in a new folder")
            .on_disabled_hover_text("Fonts are still loading")
            .clicked()
        {
            export_png(st, &path, &d);
        }
        let pro6 = d.mode == doc::Mode::Native && d.pro6.is_some();
        if doc::is_native(&path)
            && ui
                .add_enabled(pro6, egui::Button::new("As a ProPresenter 6 file (.pro6)…"))
                .on_hover_text("A copy for ProPresenter 6")
                .on_disabled_hover_text(
                    "Only presentations that started as a .pro6 file can be saved as one for now.",
                )
                .clicked()
        {
            export_pro6(st, &path, &d);
        }
    });
}

fn import_presentations(st: &mut UiState, settings: &Settings, actions: &mut Vec<UiAction>) {
    let Some(files) = rfd::FileDialog::new()
        .set_title("Import presentations")
        .add_filter("Presentations", &["pro6", doc::NATIVE_EXT])
        .pick_files()
    else {
        return;
    };
    let (mut copied, mut problems) = (Vec::new(), Vec::new());
    for f in files {
        let dir = if doc::is_native(&f) {
            settings.new_presentations_folder()
        } else {
            settings.library_dirs.first().cloned()
        };
        let Some(dir) = dir else { continue };
        let Some(name) = f.file_name() else { continue };
        let to = dir.join(name);
        if to == f {
            problems.push(format!(
                "{} is already in the library.",
                name.to_string_lossy()
            ));
            continue;
        }
        if to.exists() {
            problems.push(format!(
                "{} is already in {}; it wasn't replaced.",
                name.to_string_lossy(),
                dir.display()
            ));
            continue;
        }
        match std::fs::create_dir_all(&dir).and_then(|()| std::fs::copy(&f, &to)) {
            Ok(_) => copied.push(to),
            Err(e) => problems.push(format!(
                "{} couldn't be copied ({e}).",
                name.to_string_lossy()
            )),
        }
    }
    for p in problems {
        st.toast(p);
    }
    if let Some(first) = copied.first() {
        actions.push(UiAction::RefreshLibrary);
        actions.push(UiAction::Core(Command::Open(first.clone())));
        st.info(format!("Imported {} presentation(s).", copied.len()));
    }
}

/// Text from a file people may have saved in any editor: UTF-8 (with or without a
/// BOM), UTF-16 with a BOM, else Windows-1252-ish bytes taken as Latin-1.
fn decode_text(bytes: &[u8]) -> String {
    if let Some(b) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(b).into_owned();
    }
    let utf16 = |b: &[u8], le: bool| {
        let units: Vec<u16> = b
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| {
                if le {
                    u16::from_le_bytes(c)
                } else {
                    u16::from_be_bytes(c)
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(b) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(b, true);
    }
    if let Some(b) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(b, false);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}

fn import_lyrics(st: &mut UiState, ctx: &egui::Context, show: &ShowState) {
    let Some(f) = rfd::FileDialog::new()
        .set_title("Song words from a text file")
        .add_filter("Text", &["txt"])
        .pick_file()
    else {
        return;
    };
    match std::fs::read(&f) {
        Ok(bytes) => {
            let selected = show
                .selected
                .as_ref()
                .is_some_and(|p| show.docs.get(p).is_some_and(|d| d.theme.is_none()));
            let name = f
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            crate::editor::lyrics::open_new_with(st, ctx, selected, &name, &decode_text(&bytes));
        }
        Err(e) => st.toast(format!("{} couldn't be read ({e}).", f.display())),
    }
}

/// A save dialog starting next to the presentation, named after it.
fn save_dialog(title: &str, path: &Path, ext: &str, filter: &str) -> Option<PathBuf> {
    let mut d = rfd::FileDialog::new()
        .set_title(title)
        .add_filter(filter, &[ext])
        .set_file_name(format!("{}.{ext}", crate::ui::title(path)));
    if let Some(dir) = path.parent() {
        d = d.set_directory(dir);
    }
    d.save_file()
}

fn export_text(st: &mut UiState, path: &Path, d: &doc::Document) {
    let Some(to) = save_dialog("Export words as text", path, "txt", "Text") else {
        return;
    };
    let text = doc::lyrics::to_text(d).replace('\n', "\r\n");
    match doc::file::write_atomic(&to, text.as_bytes()) {
        Ok(()) => st.info(format!("Saved {}.", to.display())),
        Err(e) => st.toast(format!("{} couldn't be saved ({e}).", to.display())),
    }
}

/// File name for slide `n` (1-based) of group `group`.
fn png_name(n: usize, group: &str) -> String {
    let g: String = group
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_()&'".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let g = g.trim();
    if g.is_empty() {
        format!("{n:03}.png")
    } else {
        format!("{n:03} {g}.png")
    }
}

fn export_png(st: &mut UiState, path: &Path, d: &std::sync::Arc<doc::Document>) {
    let Some(fonts) = st.fonts.as_ref().map(render::Fonts::fork) else {
        return;
    };
    let mut pick = rfd::FileDialog::new().set_title("Export slides as pictures: choose where");
    if let Some(dir) = path.parent() {
        pick = pick.set_directory(dir);
    }
    let Some(parent) = pick.pick_folder() else {
        return;
    };
    let mut out = parent.join(crate::ui::title(path));
    // A new folder each time, never mixing with an earlier export.
    let mut n = 2;
    while out.exists() {
        out = parent.join(format!("{} ({n})", crate::ui::title(path)));
        n += 1;
    }
    let (tx, rx) = crossbeam_channel::bounded(1);
    let d = d.clone();
    let spawned = std::thread::Builder::new()
        .name("export-png".into())
        .spawn(move || {
            let mut fonts = fonts;
            let _ = tx.send(write_pngs(&mut fonts, &d, &out));
        });
    match spawned {
        Ok(_) => {
            st.exports.push(rx);
            st.info("Exporting slides as pictures…");
        }
        Err(e) => st.toast(format!("The export couldn't start ({e}).")),
    }
}

fn write_pngs(fonts: &mut render::Fonts, d: &doc::Document, out: &Path) -> Result<String, String> {
    std::fs::create_dir_all(out).map_err(|e| {
        format!(
            "{} couldn't be made ({e}). Nothing was exported.",
            out.display()
        )
    })?;
    let (w, h) = (d.canvas.width, d.canvas.height);
    let (mut n, mut failed) = (0, Vec::new());
    for g in d.groups() {
        for s in g.slides() {
            n += 1;
            let file = out.join(png_name(n, &g.name));
            let r = render::render_slide(fonts, d, s, w, h)
                .map_err(|e| e.user_message())
                .and_then(|f| f.composite().save(&file).map_err(|e| e.to_string()));
            if let Err(e) = r {
                failed.push(format!("slide {n}: {e}"));
            }
        }
    }
    if failed.is_empty() {
        Ok(format!("Saved {n} picture(s) in {}.", out.display()))
    } else {
        Err(format!(
            "{} of {n} slide(s) couldn't be exported to {} ({}).",
            failed.len(),
            out.display(),
            failed.join("; ")
        ))
    }
}

fn export_pro6(st: &mut UiState, path: &Path, d: &doc::Document) {
    let incompatible = d
        .groups
        .iter()
        .flat_map(|g| &g.slides)
        .filter(|s| s.pro6_incompatible)
        .count();
    let choice = if incompatible == 0 {
        doc::ExportChoice::DropUnsupported
    } else {
        let answer = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Save as ProPresenter 6")
            .set_description(format!(
                "{incompatible} slide(s) use things a ProPresenter 6 file can't hold. Leave those slides out, or keep them without those parts?"
            ))
            .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
                "Keep, simplified".into(),
                "Leave out".into(),
                "Cancel".into(),
            ))
            .show();
        match answer {
            rfd::MessageDialogResult::Custom(s) if s == "Keep, simplified" => {
                doc::ExportChoice::DropUnsupported
            }
            rfd::MessageDialogResult::Custom(s) if s == "Leave out" => {
                doc::ExportChoice::SkipIncompatible
            }
            _ => return,
        }
    };
    let Some(to) = save_dialog("Save as ProPresenter 6", path, "pro6", "ProPresenter 6") else {
        return;
    };
    match doc::export_pro6(d, choice) {
        Ok((p, report)) => match pro6::save(&p, &to) {
            Ok(()) => {
                let note = if report.left_out.is_empty() {
                    String::new()
                } else {
                    format!(" Left out or simplified: {}.", report.left_out.join(", "))
                };
                st.info(format!("Saved {}.{note}", to.display()));
            }
            Err(e) => st.toast(format!("{} couldn't be saved ({e}).", to.display())),
        },
        Err(e) => st.toast(e.user_message()),
    }
}

/// Finished background exports.
pub fn poll(st: &mut UiState) {
    let mut done = Vec::new();
    st.exports.retain(|rx| match rx.try_recv() {
        Ok(r) => {
            done.push(r);
            false
        }
        Err(crossbeam_channel::TryRecvError::Empty) => true,
        Err(crossbeam_channel::TryRecvError::Disconnected) => false,
    });
    for r in done {
        match r {
            Ok(m) => st.info(m),
            Err(e) => st.toast(e),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn text_files_in_common_encodings() {
        assert_eq!(decode_text(b"\xEF\xBB\xBF[Verse]\nA"), "[Verse]\nA");
        assert_eq!(decode_text(b"\xFF\xFEA\x00\xE9\x00"), "Aé");
        assert_eq!(decode_text(b"caf\xE9"), "café", "Windows-1252 é");
        assert_eq!(decode_text("grâce".as_bytes()), "grâce");
    }

    #[test]
    fn png_names_sort_and_are_safe() {
        assert_eq!(png_name(3, "Verse 1"), "003 Verse 1.png");
        assert_eq!(png_name(12, "A/B: C?"), "012 A_B_ C_.png");
        assert_eq!(png_name(1, ""), "001.png");
    }

    #[test]
    fn slides_export_as_pictures() {
        let src =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/song_1024x768.pro6");
        let d = doc::load(&src).unwrap();
        let dir = std::env::temp_dir().join(format!("openpresenter-png-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let font_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts");
        let mut fonts = render::Fonts::from_dirs(&[font_dir.as_path()]);
        // Missing fonts make some slides fail; the rest are still written.
        let r = write_pngs(&mut fonts, &d, &dir);
        let n = d.slides().count();
        let written = std::fs::read_dir(&dir).unwrap().count();
        match r {
            Ok(m) => assert!(m.contains(&format!("{n} picture"))),
            Err(e) => assert!(e.contains(&format!("of {n} slide"))),
        }
        assert!(written <= n);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
