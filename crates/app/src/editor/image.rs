//! Images in the editor (docs/EDITOR-PLAN.md E3): importing (file dialog, drag and
//! drop, paste), the image toolbar, the Image tab, the Adjust panel and the slide's
//! background image.
//!
//! Imported files are copied into a `Media` folder next to the presentation and
//! referenced by relative path plus SHA-256, on a worker thread (the UI never
//! waits on disk). Originals are never modified; adjustments are parameters
//! applied when drawing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use doc::{Adjust, BackgroundImage, Edit as Op, Element, Fit, ImageBox, MediaRef, new_uuid};
use egui::{Color32, RichText};
use sha2::{Digest, Sha256};

use super::{Tab, text::send_checked};
use crate::ui::{UiAction, UiState};

/// Folder (next to the presentation) imported images are copied into.
pub const MEDIA_DIR: &str = "Media";

/// What to do with an image once it's imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A new image element on slide `(g, s)`.
    Insert(usize, usize),
    /// The background image of slide `(g, s)`.
    Background(usize, usize),
}

pub enum Source {
    /// Ask with a file dialog.
    Pick,
    File(PathBuf),
    Pixels(Arc<egui::ColorImage>),
}

pub struct Imported {
    pub media: MediaRef,
    pub size: (u32, u32),
    pub name: String,
}

pub struct Done {
    /// Presentation the import was for.
    pub path: PathBuf,
    pub target: Target,
    /// `None`: the file dialog was cancelled.
    pub result: Option<Result<Imported, String>>,
}

/// Running imports and their results.
pub struct Imports {
    tx: Sender<Done>,
    rx: Receiver<Done>,
    pub running: usize,
}

impl Default for Imports {
    fn default() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Imports { tx, rx, running: 0 }
    }
}

const EXTENSIONS: [&str; 3] = ["png", "jpg", "jpeg"];

fn is_image_path(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Start importing on a worker thread.
pub fn start(
    st: &mut UiState,
    ctx: &egui::Context,
    doc_path: &Path,
    source: Source,
    target: Target,
) {
    let tx = st.edit.imports.tx.clone();
    st.edit.imports.running += 1;
    let ctx = ctx.clone();
    let path = doc_path.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("image-import".into())
        .spawn(move || {
            let result = match source {
                Source::Pick => rfd::FileDialog::new()
                    .set_title("Insert image")
                    .add_filter("Images (PNG, JPEG)", &EXTENSIONS)
                    .pick_file()
                    .map(|f| import_file(&path, &f)),
                Source::File(f) => Some(import_file(&path, &f)),
                Source::Pixels(img) => Some(import_pixels(&path, &img)),
            };
            let _ = tx.send(Done {
                path,
                target,
                result,
            });
            ctx.request_repaint();
        });
    if let Err(e) = spawned {
        st.edit.imports.running -= 1;
        st.toast(format!("Couldn't start importing the image: {e}"));
    }
}

fn import_file(doc_path: &Path, file: &Path) -> Result<Imported, String> {
    let _span = tracing::info_span!("import_image", file = %file.display()).entered();
    let name = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_owned();
    if !is_image_path(file) {
        return Err(format!(
            "\"{}\" isn't a PNG or JPEG image. Nothing was added.",
            file.display()
        ));
    }
    let bytes = std::fs::read(file).map_err(|e| {
        format!(
            "Couldn't read \"{}\" ({e}). Nothing was added.",
            file.display()
        )
    })?;
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    store(doc_path, &name, &ext, &bytes)
}

#[cfg(test)]
pub fn import_file_for_test(doc_path: &Path, file: &Path) -> Result<Imported, String> {
    import_file(doc_path, file)
}

fn import_pixels(doc_path: &Path, img: &egui::ColorImage) -> Result<Imported, String> {
    let [w, h] = img.size;
    let rgba: Vec<u8> = img
        .pixels
        .iter()
        .flat_map(|c| c.to_srgba_unmultiplied())
        .collect();
    let buf = image::RgbaImage::from_raw(w as u32, h as u32, rgba)
        .ok_or_else(|| "The pasted image is damaged. Nothing was added.".to_owned())?;
    let mut bytes = Vec::new();
    buf.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )
    .map_err(|e| format!("Couldn't save the pasted image ({e}). Nothing was added."))?;
    store(doc_path, "Pasted image", "png", &bytes)
}

/// Copy `bytes` into the presentation's media folder (named by content, so the
/// same picture is stored once) and describe it.
fn store(doc_path: &Path, name: &str, ext: &str, bytes: &[u8]) -> Result<Imported, String> {
    let size = image::load_from_memory(bytes)
        .map(|i| (i.width(), i.height()))
        .map_err(|e| format!("This file can't be read as an image ({e}). Nothing was added."))?;
    let sha = Sha256::digest(bytes);
    let hex: String = sha.iter().map(|b| format!("{b:02x}")).collect();
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let file_name = format!("{}-{}.{ext}", safe.trim(), &hex[..8]);
    let dir = doc_path.parent().unwrap_or(Path::new(".")).join(MEDIA_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| {
        format!(
            "Couldn't make the media folder {} ({e}). Nothing was added.",
            dir.display()
        )
    })?;
    let dest = dir.join(&file_name);
    let same = std::fs::metadata(&dest).is_ok_and(|m| m.len() == bytes.len() as u64);
    if !same {
        let tmp = dir.join(format!(".{file_name}.tmp"));
        std::fs::write(&tmp, bytes)
            .and_then(|()| std::fs::rename(&tmp, &dest))
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!(
                    "Couldn't copy the image into {} ({e}). Nothing was added.",
                    dir.display()
                )
            })?;
    }
    tracing::info!(dest = %dest.display(), "image imported");
    Ok(Imported {
        media: MediaRef {
            path: format!("{MEDIA_DIR}/{file_name}"),
            sha256: Some(hex),
        },
        size,
        name: name.to_owned(),
    })
}

/// A new image element for `d`, centred, fitting within 80% of the canvas at its
/// own shape (never enlarged past its natural size).
pub fn new_element(d: &doc::Document, im: &Imported) -> ImageBox {
    let (cw, ch) = (f64::from(d.canvas.width), f64::from(d.canvas.height));
    let (iw, ih) = (f64::from(im.size.0.max(1)), f64::from(im.size.1.max(1)));
    let s = (cw * 0.8 / iw).min(ch * 0.8 / ih).min(1.0);
    let (w, h) = ((iw * s).round().max(1.0), (ih * s).round().max(1.0));
    ImageBox {
        uuid: new_uuid(),
        name: im.name.clone(),
        frame: doc::Rect {
            x: ((cw - w) / 2.0).round(),
            y: ((ch - h) / 2.0).round(),
            width: w,
            height: h,
        },
        rotation: 0.0,
        opacity: 1.0,
        locked: false,
        flip_h: false,
        flip_v: false,
        media: im.media.clone(),
        adjust: Adjust::default(),
        src: None,
    }
}

/// Finished imports → edits; dropped and pasted images → imports. Called every
/// frame while the editor is open.
pub fn poll(
    ui: &egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
) {
    while let Ok(done) = st.edit.imports.rx.try_recv() {
        st.edit.imports.running = st.edit.imports.running.saturating_sub(1);
        if done.path != path {
            continue;
        }
        match done.result {
            None => {}
            Some(Err(e)) => st.toast(e),
            Some(Ok(im)) => match done.target {
                Target::Insert(g, s) if d.groups.get(g).is_some_and(|gr| s < gr.slides.len()) => {
                    let at = d.groups[g].slides[s].elements.len();
                    send_checked(
                        st,
                        actions,
                        path,
                        d,
                        Op::AddElement {
                            group: g,
                            slide: s,
                            at: None,
                            element: Element::Image(Box::new(new_element(d, &im))),
                        },
                        None,
                    );
                    st.edit.select_slide(g, s);
                    st.edit.element = Some(at);
                    st.edit.tab = Tab::Image;
                }
                Target::Background(g, s)
                    if d.groups.get(g).is_some_and(|gr| s < gr.slides.len()) =>
                {
                    let current = d.groups[g].slides[s].background_image.as_ref();
                    send_checked(
                        st,
                        actions,
                        path,
                        d,
                        Op::SetBackgroundImage {
                            group: g,
                            slide: s,
                            image: Some(BackgroundImage {
                                media: im.media,
                                fit: current.map_or(Fit::Fill, |b| b.fit),
                                adjust: current.map_or_else(Adjust::default, |b| b.adjust),
                            }),
                        },
                        None,
                    );
                }
                _ => st.toast("The slide the image was for no longer exists. Nothing was added."),
            },
        }
    }
    let Some((g, s)) = st.edit.sel else { return };
    let typing = st.edit.session.is_some() || ui.memory(|m| m.focused().is_some());
    let (dropped, pasted, pasted_text) = ui.input(|i| {
        let dropped: Vec<PathBuf> = i
            .raw
            .dropped_files
            .iter()
            .map(|f| f.path().to_path_buf())
            .filter(|p| !p.as_os_str().is_empty())
            .collect();
        let mut img = None;
        let mut text = None;
        for e in &i.events {
            match e {
                egui::Event::PasteImage(im) => img = Some(im.clone()),
                egui::Event::Paste(t) => text = Some(t.clone()),
                _ => {}
            }
        }
        (dropped, img, text)
    });
    for f in dropped {
        start(st, ui.ctx(), path, Source::File(f), Target::Insert(g, s));
    }
    if typing {
        return;
    }
    if let Some(img) = pasted {
        start(
            st,
            ui.ctx(),
            path,
            Source::Pixels(img),
            Target::Insert(g, s),
        );
    } else if let Some(t) = pasted_text {
        // A file copied in the file manager pastes as its path or file:// URL.
        for line in t.lines() {
            let line = line.trim();
            let p = PathBuf::from(
                line.strip_prefix("file://")
                    .map_or(line.to_owned(), percent_decode),
            );
            if is_image_path(&p) && p.is_file() {
                start(st, ui.ctx(), path, Source::File(p), Target::Insert(g, s));
            }
        }
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn image_at(d: &doc::Document, g: usize, s: usize, e: usize) -> Option<&ImageBox> {
    d.groups.get(g)?.slides.get(s)?.elements.get(e)?.as_image()
}

/// Remove the element and make its picture the slide background (one undo step).
fn set_as_background(
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    (g, s, e): (usize, usize, usize),
) {
    let Some(i) = image_at(d, g, s, e) else {
        return;
    };
    let op = Op::Batch {
        label: "Set as background".into(),
        edits: vec![
            Op::RemoveElement {
                group: g,
                slide: s,
                element: e,
            },
            Op::SetBackgroundImage {
                group: g,
                slide: s,
                image: Some(BackgroundImage {
                    media: i.media.clone(),
                    fit: Fit::Fill,
                    adjust: i.adjust,
                }),
            },
        ],
    };
    send_checked(st, actions, path, d, op, None);
    st.edit.element = None;
}

/// Image toolbar (contextual bar when an image is selected).
pub fn toolbar(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    (g, s, e): (usize, usize, usize),
) {
    let Some(i) = image_at(d, g, s, e) else {
        return;
    };
    ui.label(RichText::new("Image").strong());
    ui.separator();
    if ui
        .selectable_label(st.edit.tab == Tab::Adjust, "Adjust")
        .on_hover_text("Brightness, contrast, colour, vignette")
        .clicked()
    {
        st.edit.adjust_background = false;
        st.edit.tab = Tab::Adjust;
    }
    let flip = |h: bool, v: bool| Op::SetFlip {
        group: g,
        slide: s,
        element: e,
        horizontal: h,
        vertical: v,
    };
    if ui
        .button("Flip H")
        .on_hover_text("Mirror left to right")
        .clicked()
    {
        send_checked(st, actions, path, d, flip(!i.flip_h, i.flip_v), None);
    }
    if ui
        .button("Flip V")
        .on_hover_text("Mirror top to bottom")
        .clicked()
    {
        send_checked(st, actions, path, d, flip(i.flip_h, !i.flip_v), None);
    }
    opacity_slider(ui, st, actions, path, d, (g, s, e), i.opacity, &i.uuid);
    if ui
        .button("Set as Background")
        .on_hover_text("Fill the slide behind everything with this picture")
        .clicked()
    {
        set_as_background(st, actions, path, d, (g, s, e));
    }
}

#[allow(clippy::too_many_arguments)]
pub fn opacity_slider(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    (g, s, e): (usize, usize, usize),
    opacity: f64,
    uuid: &str,
) {
    ui.label("Opacity");
    let mut pct = (opacity * 100.0).round();
    if ui
        .add(
            egui::Slider::new(&mut pct, 0.0..=100.0)
                .suffix("%")
                .integer(),
        )
        .changed()
    {
        send_checked(
            st,
            actions,
            path,
            d,
            Op::SetOpacity {
                group: g,
                slide: s,
                element: e,
                opacity: pct / 100.0,
            },
            Some(format!("opacity:{uuid}")),
        );
    }
}

/// The Image tab: what the picture is, plus the toolbar's actions.
pub fn image_tab(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    (g, s, e): (usize, usize, usize),
) {
    let Some(i) = image_at(d, g, s, e) else {
        ui.label(
            RichText::new(
                "Select an image on the canvas, or insert one with Image in the top bar.",
            )
            .weak(),
        );
        return;
    };
    ui.label(RichText::new(if i.name.is_empty() { "Image" } else { &i.name }).strong());
    ui.label(RichText::new(i.media.file_name()).small().weak())
        .on_hover_text(i.media.resolve(path).display().to_string());
    if let Some(p) = st.canvas.as_ref().and_then(|c| c.problem(&i.uuid)) {
        ui.label(RichText::new(p).color(Color32::from_rgb(255, 130, 130)));
    }
    ui.label(format!(
        "{:.0} × {:.0} at {:.0}, {:.0}",
        i.frame.width, i.frame.height, i.frame.x, i.frame.y
    ));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        opacity_slider(ui, st, actions, path, d, (g, s, e), i.opacity, &i.uuid);
    });
    ui.horizontal(|ui| {
        let mut h = i.flip_h;
        let mut v = i.flip_v;
        let a = ui.checkbox(&mut h, "Flipped left–right").changed();
        let b = ui.checkbox(&mut v, "Flipped top–bottom").changed();
        if a || b {
            send_checked(
                st,
                actions,
                path,
                d,
                Op::SetFlip {
                    group: g,
                    slide: s,
                    element: e,
                    horizontal: h,
                    vertical: v,
                },
                None,
            );
        }
    });
    ui.add_space(6.0);
    if ui.button("Adjust…").clicked() {
        st.edit.adjust_background = false;
        st.edit.tab = Tab::Adjust;
    }
    if ui.button("Set as Background").clicked() {
        set_as_background(st, actions, path, d, (g, s, e));
    }
    if ui.button("Delete image").clicked() {
        send_checked(
            st,
            actions,
            path,
            d,
            Op::RemoveElement {
                group: g,
                slide: s,
                element: e,
            },
            None,
        );
        st.edit.element = None;
    }
}

/// The slide's background image, in the Slide tab.
pub fn background_section(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    (g, s): (usize, usize),
) {
    ui.label(RichText::new("Background image").strong());
    let slide = &d.groups[g].slides[s];
    let Some(bg) = &slide.background_image else {
        if ui
            .button("Choose image…")
            .on_hover_text("Fill the slide behind everything with a picture")
            .clicked()
        {
            start(
                st,
                ui.ctx(),
                path,
                Source::Pick,
                super::image::Target::Background(g, s),
            );
        }
        return;
    };
    ui.label(RichText::new(bg.media.file_name()).small().weak())
        .on_hover_text(bg.media.resolve(path).display().to_string());
    if let Some(p) = st.canvas.as_ref().and_then(|c| c.problem("")) {
        ui.label(RichText::new(p).color(Color32::from_rgb(255, 130, 130)));
    }
    let set = |fit: Fit, adjust: Adjust| Op::SetBackgroundImage {
        group: g,
        slide: s,
        image: Some(BackgroundImage {
            media: bg.media.clone(),
            fit,
            adjust,
        }),
    };
    ui.horizontal(|ui| {
        for (fit, label, tip) in [
            (
                Fit::Fit,
                "Fit",
                "The whole picture, bars where the shapes differ",
            ),
            (
                Fit::Fill,
                "Fill",
                "Cover the slide, cropping what doesn't fit",
            ),
            (Fit::Stretch, "Stretch", "Stretch to the slide's shape"),
        ] {
            if ui
                .selectable_label(bg.fit == fit, label)
                .on_hover_text(tip)
                .clicked()
                && bg.fit != fit
            {
                send_checked(st, actions, path, d, set(fit, bg.adjust), None);
            }
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Adjust…").clicked() {
            st.edit.adjust_background = true;
            st.edit.tab = Tab::Adjust;
        }
        if ui.button("Replace…").clicked() {
            start(st, ui.ctx(), path, Source::Pick, Target::Background(g, s));
        }
        if ui.button("Remove").clicked() {
            send_checked(
                st,
                actions,
                path,
                d,
                Op::SetBackgroundImage {
                    group: g,
                    slide: s,
                    image: None,
                },
                None,
            );
        }
    });
}

/// The Adjust panel (Canva-style sliders), for the selected image or the slide's
/// background image. Every change shows on the canvas straight away; a slider
/// drag is one undo step.
pub fn adjust_tab(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    (g, s): (usize, usize),
) {
    let background = st.edit.adjust_background;
    if ui.button("‹ Back").clicked() {
        st.edit.tab = if background { Tab::Slide } else { Tab::Image };
    }
    let slide = &d.groups[g].slides[s];
    let (current, key, make): (Adjust, String, Box<dyn Fn(Adjust) -> Op>) = if background {
        let Some(bg) = slide.background_image.clone() else {
            ui.label(RichText::new("This slide has no background image.").weak());
            return;
        };
        ui.heading("Adjust background");
        (
            bg.adjust,
            format!("adjust:bg:{}", slide.uuid),
            Box::new(move |adjust| Op::SetBackgroundImage {
                group: g,
                slide: s,
                image: Some(BackgroundImage {
                    adjust,
                    ..bg.clone()
                }),
            }),
        )
    } else {
        let Some((e, i)) = st
            .edit
            .element
            .and_then(|e| image_at(d, g, s, e).map(|i| (e, i)))
        else {
            ui.label(RichText::new("Select an image on the canvas.").weak());
            return;
        };
        ui.heading("Adjust image");
        (
            i.adjust,
            format!("adjust:{}", i.uuid),
            Box::new(move |adjust| Op::SetImageAdjust {
                group: g,
                slide: s,
                element: e,
                adjust,
            }),
        )
    };
    let mut a = current;
    let mut changed = false;
    let mut reset_one = false;
    egui::Grid::new("adjust-grid")
        .num_columns(3)
        .spacing([6.0, 8.0])
        .show(ui, |ui| {
            let defaults = Adjust::default();
            let rows: [(&str, &mut f64, f64, f64, f64); 7] = [
                (
                    "Brightness",
                    &mut a.brightness,
                    -1.0,
                    1.0,
                    defaults.brightness,
                ),
                ("Contrast", &mut a.contrast, -1.0, 1.0, defaults.contrast),
                (
                    "Saturation",
                    &mut a.saturation,
                    -1.0,
                    1.0,
                    defaults.saturation,
                ),
                (
                    "Temperature",
                    &mut a.temperature,
                    -1.0,
                    1.0,
                    defaults.temperature,
                ),
                ("Tint", &mut a.tint, -1.0, 1.0, defaults.tint),
                ("Vignette", &mut a.vignette, 0.0, 1.0, defaults.vignette),
                (
                    "Vignette size",
                    &mut a.vignette_size,
                    0.0,
                    1.0,
                    defaults.vignette_size,
                ),
            ];
            for (label, v, lo, hi, def) in rows {
                ui.label(label);
                let mut pct = (*v * 100.0).round();
                if ui
                    .add(
                        egui::Slider::new(&mut pct, lo * 100.0..=hi * 100.0)
                            .integer()
                            .show_value(true),
                    )
                    .changed()
                {
                    *v = pct / 100.0;
                    changed = true;
                }
                if ui
                    .add_enabled(*v != def, egui::Button::new("↺").small())
                    .on_hover_text(format!("Reset {}", label.to_lowercase()))
                    .clicked()
                {
                    *v = def;
                    reset_one = true;
                }
                ui.end_row();
            }
        });
    ui.add_space(6.0);
    let reset_all = ui
        .add_enabled(current != Adjust::default(), egui::Button::new("Reset all"))
        .clicked();
    if reset_all {
        a = Adjust::default();
    }
    if a != current {
        let merge = (changed && !reset_one && !reset_all).then_some(key);
        send_checked(st, actions, path, d, make(a), merge);
    }
    ui.add_space(8.0);
    ui.label(
        RichText::new(
            "Adjustments are kept with the slide; the picture file itself is never changed.",
        )
        .small()
        .weak(),
    );
}
