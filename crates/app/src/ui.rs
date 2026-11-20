//! Operator UI (egui). Renders from the latest `ShowState` snapshot and returns
//! actions; it never mutates show state directly.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::Sender;
use egui::{Color32, RichText, Sense, Stroke, TextureHandle, Vec2};
use pp6core::library::LibraryIndex;
use pp6core::{Command, NoticeLevel, Settings, ShowState, SlideRef, next_enabled};

use crate::thumbs::{ThumbKey, ThumbRequest, ThumbResult};

const TILE_W: u32 = 200;
const PREVIEW_W: u32 = 320;

/// Things the UI asks the app to do after the frame.
pub enum UiAction {
    Core(Command),
    AddLibraryFolder,
    AddMediaFolder,
    OpenPlaylist,
    SavePlaylistAs,
    /// Output display chosen by description index (see `UiState::monitors`).
    ChooseMonitor(usize),
}

#[derive(Default)]
pub struct UiState {
    pub search: String,
    pub library: LibraryIndex,
    pub library_status: String,
    pub monitors: Vec<String>,
    pub media_files: Vec<PathBuf>,
    pub app_notices: Vec<String>,
    pub(crate) textures: HashMap<ThumbKey, Result<TextureHandle, String>>,
    pub(crate) requested: HashSet<ThumbKey>,
}

impl UiState {
    /// Take finished thumbnails into egui textures.
    pub fn receive_thumbs(
        &mut self,
        ctx: &egui::Context,
        rx: &crossbeam_channel::Receiver<ThumbResult>,
    ) {
        while let Ok(r) = rx.try_recv() {
            let tex = r.image.map(|img| {
                ctx.load_texture(
                    format!("thumb-{:?}", r.key),
                    img,
                    egui::TextureOptions::LINEAR,
                )
            });
            self.textures.insert(r.key, tex);
        }
        // Drop textures of documents that are no longer loaded (cheap bound on memory).
        if self.textures.len() > 2000 {
            self.textures.clear();
            self.requested.clear();
        }
    }

    fn thumb(
        &mut self,
        tx: &Sender<ThumbRequest>,
        doc: &Arc<pro6::Document>,
        flat: usize,
        width: u32,
    ) -> (ThumbKey, Option<&Result<TextureHandle, String>>) {
        let height = (width as f32 * doc.canvas.height as f32 / doc.canvas.width as f32)
            .round()
            .max(1.0) as u32;
        let key = ThumbKey {
            doc: Arc::as_ptr(doc) as usize,
            flat,
            width,
            height,
        };
        if !self.requested.contains(&key) {
            self.requested.insert(key);
            let _ = tx.send(ThumbRequest {
                key,
                doc: doc.clone(),
            });
        }
        (key, self.textures.get(&key))
    }
}

pub fn draw(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    settings: &Settings,
    thumbs: &Sender<ThumbRequest>,
    log_lines: &[String],
) -> Vec<UiAction> {
    let mut actions = Vec::new();
    toolbar(ui, st, show, settings, &mut actions);
    egui::Panel::bottom("messages")
        .resizable(true)
        .default_size(90.0)
        .show(ui, |ui| messages(ui, st, show, log_lines));
    egui::Panel::left("left")
        .resizable(true)
        .default_size(300.0)
        .show(ui, |ui| {
            library(ui, st, show, settings, &mut actions);
            ui.separator();
            playlist(ui, show, &mut actions);
        });
    egui::Panel::right("right")
        .resizable(true)
        .default_size(PREVIEW_W as f32 + 24.0)
        .show(ui, |ui| {
            preview(ui, st, show, thumbs);
            ui.separator();
            media_bin(ui, st, settings, &mut actions);
        });
    egui::CentralPanel::default().show(ui, |ui| grid(ui, st, show, thumbs, &mut actions));
    actions
}

fn keys(list: &[String]) -> String {
    list.first().map(|k| format!(" ({k})")).unwrap_or_default()
}

fn title(path: &Path) -> String {
    path.file_stem().map_or_else(
        || path.display().to_string(),
        |s| s.to_string_lossy().into_owned(),
    )
}

fn toolbar(
    ui: &mut egui::Ui,
    st: &UiState,
    show: &ShowState,
    settings: &Settings,
    actions: &mut Vec<UiAction>,
) {
    egui::Panel::top("toolbar").show(ui, |ui| {
        ui.horizontal(|ui| {
            let k = &settings.keys;
            let clear = |on: bool, label: String| {
                egui::Button::new(RichText::new(label).strong()).fill(if on {
                    Color32::from_rgb(70, 30, 30)
                } else {
                    Color32::TRANSPARENT
                })
            };
            if ui
                .add(clear(
                    !show.text_visible,
                    format!("Clear Text{}", keys(&k.clear_text)),
                ))
                .clicked()
            {
                actions.push(UiAction::Core(Command::ClearText));
            }
            if ui
                .add(clear(
                    !show.background_visible,
                    format!("Clear Background{}", keys(&k.clear_background)),
                ))
                .clicked()
            {
                actions.push(UiAction::Core(Command::ClearBackground));
            }
            if ui
                .add(clear(
                    !show.text_visible && !show.background_visible,
                    format!("Clear All{}", keys(&k.clear_all)),
                ))
                .clicked()
            {
                actions.push(UiAction::Core(Command::ClearAll));
            }
            ui.separator();
            let mut on = show.output_enabled;
            let label = if on { "Output ON" } else { "Output OFF" };
            if ui.toggle_value(&mut on, label).changed() {
                actions.push(UiAction::Core(Command::OutputEnable(on)));
            }
            let current = settings
                .output
                .monitor_name
                .clone()
                .unwrap_or_else(|| "automatic".into());
            egui::ComboBox::from_id_salt("monitor")
                .selected_text(format!("Display: {current}"))
                .show_ui(ui, |ui| {
                    for (i, m) in st.monitors.iter().enumerate() {
                        if ui.selectable_label(false, m).clicked() {
                            actions.push(UiAction::ChooseMonitor(i));
                        }
                    }
                });
            ui.separator();
            match &show.live {
                Some(l) => {
                    let group = l
                        .doc
                        .groups()
                        .get(l.slide.group)
                        .map(|g| g.name.clone())
                        .unwrap_or_default();
                    let pos = l.position.map_or("–".into(), |p| (p + 1).to_string());
                    ui.label(
                        RichText::new(format!(
                            "LIVE: {} · {} · {pos}/{}",
                            title(&l.path),
                            group,
                            l.order.len()
                        ))
                        .color(Color32::from_rgb(255, 170, 60)),
                    );
                }
                None => {
                    ui.label(RichText::new("Nothing live").weak());
                }
            }
        });
    });
}

fn library(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    settings: &Settings,
    actions: &mut Vec<UiAction>,
) {
    ui.horizontal(|ui| {
        ui.heading("Library");
        if ui.small_button("Add folder…").clicked() {
            actions.push(UiAction::AddLibraryFolder);
        }
    });
    if settings.library_dirs.is_empty() {
        ui.label(
            RichText::new("No library folders yet. Add the folder with your .pro6 files.")
                .italics(),
        );
    }
    ui.add(egui::TextEdit::singleline(&mut st.search).hint_text("Search titles and lyrics"));
    ui.label(RichText::new(&st.library_status).small().weak());
    let results = st.library.search(&st.search);
    egui::ScrollArea::vertical()
        .id_salt("library")
        .max_height(ui.available_height() * 0.5)
        .show(ui, |ui| {
            for e in results {
                let selected = show.selected.as_deref() == Some(e.path.as_path());
                let label = if e.error.is_some() {
                    RichText::new(format!("! {}", e.title)).color(Color32::LIGHT_RED)
                } else {
                    RichText::new(&e.title)
                };
                ui.horizontal(|ui| {
                    if ui
                        .small_button("+")
                        .on_hover_text("Add to playlist")
                        .clicked()
                    {
                        actions.push(UiAction::Core(Command::PlaylistAdd {
                            path: e.path.clone(),
                            arrangement: None,
                        }));
                    }
                    let r = ui.selectable_label(selected, label);
                    let r = match &e.error {
                        Some(err) => r.on_hover_text(err),
                        None => r,
                    };
                    if r.double_clicked() {
                        actions.push(UiAction::Core(Command::PlaylistAdd {
                            path: e.path.clone(),
                            arrangement: None,
                        }));
                    } else if r.clicked() {
                        actions.push(UiAction::Core(Command::Open(e.path.clone())));
                    }
                });
            }
        });
}

fn playlist(ui: &mut egui::Ui, show: &ShowState, actions: &mut Vec<UiAction>) {
    ui.horizontal(|ui| {
        ui.heading("Playlist");
        if ui.small_button("New").clicked() {
            actions.push(UiAction::Core(Command::PlaylistNew));
        }
        if ui.small_button("Open…").clicked() {
            actions.push(UiAction::OpenPlaylist);
        }
        if ui.small_button("Save").clicked() {
            if show.playlist_path.is_some() {
                actions.push(UiAction::Core(Command::PlaylistSave(None)));
            } else {
                actions.push(UiAction::SavePlaylistAs);
            }
        }
        if ui.small_button("Save As…").clicked() {
            actions.push(UiAction::SavePlaylistAs);
        }
    });
    if let Some(p) = &show.playlist_path {
        ui.label(RichText::new(title(p)).small().weak());
    }
    egui::ScrollArea::vertical()
        .id_salt("playlist")
        .show(ui, |ui| {
            let n = show.playlist.items.len();
            for (i, item) in show.playlist.items.iter().enumerate() {
                ui.horizontal(|ui| {
                    let live = show.is_live(&item.path);
                    let mut text = RichText::new(format!("{}. {}", i + 1, title(&item.path)));
                    if live {
                        text = text.color(Color32::from_rgb(255, 170, 60)).strong();
                    }
                    if show.load_errors.contains_key(&item.path) {
                        text = text.color(Color32::LIGHT_RED);
                    }
                    if ui
                        .selectable_label(show.playlist_item == Some(i), text)
                        .clicked()
                    {
                        actions.push(UiAction::Core(Command::PlaylistSelect(i)));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("x").clicked() {
                            actions.push(UiAction::Core(Command::PlaylistRemove(i)));
                        }
                        if i + 1 < n && ui.small_button("v").clicked() {
                            actions
                                .push(UiAction::Core(Command::PlaylistMove { from: i, to: i + 1 }));
                        }
                        if i > 0 && ui.small_button("^").clicked() {
                            actions
                                .push(UiAction::Core(Command::PlaylistMove { from: i, to: i - 1 }));
                        }
                    });
                });
            }
            if n == 0 {
                ui.label(
                    RichText::new("Empty. Use + in the library to add presentations.").italics(),
                );
            }
        });
}

fn color32(c: Option<pro6::Rgba>) -> Color32 {
    let q = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    match c {
        Some(c) if c.a > 0.0 => Color32::from_rgb(q(c.r), q(c.g), q(c.b)),
        _ => Color32::from_gray(90),
    }
}

fn tile_image(
    ui: &mut egui::Ui,
    tex: Option<&Result<TextureHandle, String>>,
    size: Vec2,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    match tex {
        Some(Ok(t)) => {
            egui::Image::new(t).paint_at(ui, rect);
        }
        Some(Err(e)) => {
            ui.painter()
                .rect_filled(rect, 2.0, Color32::from_rgb(60, 10, 10));
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "can't draw",
                egui::FontId::proportional(12.0),
                Color32::LIGHT_RED,
            );
            return response.on_hover_text(e);
        }
        None => {
            ui.painter().rect_filled(rect, 2.0, Color32::from_gray(25));
        }
    }
    response
}

fn grid(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    thumbs: &Sender<ThumbRequest>,
    actions: &mut Vec<UiAction>,
) {
    let Some(path) = show.selected.clone() else {
        ui.centered_and_justified(|ui| {
            ui.label("Select a presentation in the library or playlist.")
        });
        return;
    };
    let Some(doc) = show.docs.get(&path).cloned() else {
        match show.load_errors.get(&path) {
            Some(e) => ui.colored_label(Color32::LIGHT_RED, e),
            None => ui.label(format!("Loading {}…", title(&path))),
        };
        return;
    };
    let arrangement = show.arrangement_for(&path);
    let arrangements = doc.arrangements();
    ui.horizontal(|ui| {
        ui.heading(title(&path));
        let name = |id: &Option<String>| match id {
            None => "Natural order".to_owned(),
            Some(id) => arrangements
                .iter()
                .find(|a| &a.uuid == id)
                .map_or_else(|| "Unknown arrangement".into(), |a| a.name.clone()),
        };
        egui::ComboBox::from_id_salt("arrangement")
            .selected_text(format!("Arrangement: {}", name(&arrangement)))
            .show_ui(ui, |ui| {
                let mut pick = |id: Option<String>, label: String| {
                    if ui.selectable_label(arrangement == id, label).clicked() {
                        actions.push(UiAction::Core(Command::SelectArrangement {
                            presentation: path.clone(),
                            arrangement: id,
                        }));
                    }
                };
                pick(None, "Natural order".into());
                for a in &arrangements {
                    pick(Some(a.uuid.clone()), a.name.clone());
                }
            });
        if doc
            .timeline
            .as_ref()
            .is_some_and(|t| t.time_cue_count() + t.media_track_count() > 0)
            || doc.slides().any(|s| s.cue_count() > 0)
        {
            ui.colored_label(
                Color32::YELLOW,
                "! media cues / auto-advance not supported yet",
            );
        }
    });
    let order: Vec<SlideRef> = pp6core::slide_order(&doc, arrangement.as_deref());
    let live = show.live.as_ref().filter(|l| l.path == path);
    let height = TILE_W as f32 * doc.canvas.height as f32 / doc.canvas.width as f32;
    egui::ScrollArea::vertical().id_salt("grid").show(ui, |ui| {
        let mut i = 0;
        while i < order.len() {
            // A section is a run of consecutive positions from the same group.
            let g = order[i].group;
            let end = (i..order.len())
                .find(|&j| order[j].group != g)
                .unwrap_or(order.len());
            let group = &doc.groups()[g];
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::new(14.0, 14.0), Sense::hover());
                ui.painter().rect_filled(r, 2.0, color32(group.color));
                ui.strong(if group.name.is_empty() {
                    "(unnamed group)"
                } else {
                    &group.name
                });
            });
            let spacing = ui.spacing().item_spacing.x;
            let per_row = (((ui.available_width() + spacing) / (TILE_W as f32 + spacing)).floor()
                as usize)
                .max(1);
            let positions: Vec<usize> = (i..end).collect();
            for row in positions.chunks(per_row) {
                ui.horizontal(|ui| {
                    for &pos in row {
                        let r = &order[pos];
                        ui.vertical(|ui| {
                            let (_, tex) = st.thumb(thumbs, &doc, r.flat, TILE_W);
                            let resp = tile_image(ui, tex, Vec2::new(TILE_W as f32, height));
                            let is_live = live.is_some_and(|l| {
                                l.position == Some(pos)
                                    || (l.position.is_none() && l.slide.flat == r.flat)
                            });
                            let painter = ui.painter();
                            if !r.enabled {
                                painter.rect_filled(resp.rect, 0.0, Color32::from_black_alpha(170));
                            }
                            let stroke = if is_live {
                                Stroke::new(3.0, Color32::from_rgb(255, 150, 30))
                            } else {
                                Stroke::new(1.0, color32(group.color))
                            };
                            painter.rect_stroke(resp.rect, 2.0, stroke, egui::StrokeKind::Outside);
                            let label = format!(
                                "{}{}",
                                pos + 1,
                                if r.enabled { "" } else { " (disabled)" }
                            );
                            ui.label(RichText::new(label).small().color(if r.enabled {
                                Color32::GRAY
                            } else {
                                Color32::DARK_GRAY
                            }));
                            if resp.clicked() {
                                actions.push(UiAction::Core(Command::GoTo {
                                    presentation: path.clone(),
                                    position: pos,
                                }));
                            }
                        });
                    }
                });
            }
            ui.add_space(6.0);
            i = end;
        }
    });
}

fn preview(ui: &mut egui::Ui, st: &mut UiState, show: &ShowState, thumbs: &Sender<ThumbRequest>) {
    ui.heading("Preview");
    let Some(l) = &show.live else {
        let (r, _) = ui.allocate_exact_size(
            Vec2::new(PREVIEW_W as f32, PREVIEW_W as f32 * 0.75),
            Sense::hover(),
        );
        ui.painter().rect_filled(r, 2.0, Color32::BLACK);
        ui.label(RichText::new("Nothing live").weak());
        return;
    };
    let h = PREVIEW_W as f32 * l.doc.canvas.height as f32 / l.doc.canvas.width as f32;
    let size = Vec2::new(PREVIEW_W as f32, h);
    if show.text_visible {
        let (_, tex) = st.thumb(thumbs, &l.doc, l.slide.flat, PREVIEW_W);
        tile_image(ui, tex, size);
    } else {
        let (r, _) = ui.allocate_exact_size(size, Sense::hover());
        ui.painter().rect_filled(r, 2.0, Color32::BLACK);
    }
    let state = match (show.text_visible, show.background_visible) {
        (true, true) => "On screen".to_owned(),
        (false, true) => "Text cleared".into(),
        (true, false) => "Background cleared".into(),
        (false, false) => "All cleared".into(),
    };
    ui.label(RichText::new(state).small());
    if let Some(n) = next_enabled(&l.order, l.position) {
        ui.label(RichText::new("Next").small().weak());
        let small = Vec2::new(PREVIEW_W as f32 / 2.0, h / 2.0);
        let (_, tex) = st.thumb(thumbs, &l.doc, l.order[n].flat, PREVIEW_W / 2);
        tile_image(ui, tex, small);
    }
}

fn media_bin(ui: &mut egui::Ui, st: &UiState, settings: &Settings, actions: &mut Vec<UiAction>) {
    ui.horizontal(|ui| {
        ui.heading("Media");
        if ui.small_button("Add folder…").clicked() {
            actions.push(UiAction::AddMediaFolder);
        }
    });
    ui.label(
        RichText::new("Backgrounds and video arrive in M5.")
            .small()
            .weak(),
    );
    if settings.media_dirs.is_empty() {
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("media")
        .show(ui, |ui| {
            for f in &st.media_files {
                ui.label(
                    f.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
            }
        });
}

fn messages(ui: &mut egui::Ui, st: &UiState, show: &ShowState, log_lines: &[String]) {
    egui::ScrollArea::vertical()
        .id_salt("messages")
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for n in show.notices.iter() {
                let c = match n.level {
                    NoticeLevel::Error => Color32::LIGHT_RED,
                    NoticeLevel::Warning => Color32::YELLOW,
                    NoticeLevel::Info => Color32::GRAY,
                };
                ui.colored_label(c, &n.text);
            }
            for n in &st.app_notices {
                ui.colored_label(Color32::LIGHT_RED, n);
            }
            for l in log_lines.iter().rev().take(8).rev() {
                ui.label(RichText::new(l).small().weak().monospace());
            }
        });
}
