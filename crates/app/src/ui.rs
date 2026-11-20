//! Operator UI (egui). Renders from the latest `ShowState` snapshot and returns
//! actions; it never mutates show state directly.
//!
//! Layout follows ProPresenter 6, which the team knows:
//!
//! ```text
//! ┌──────────────────────── toolbar (icons) ─────────────────────────┐
//! │ Preview              │ Presentation (slide grid)                 │
//! │ Library  [search]    │                                           │
//! │ Playlist             │ Video/Image (backgrounds)                 │
//! └──────────────────────┴───────────────────────────────────────────┘
//! ```
//! Messages and the log live in Settings → Log; new problems show a toast and a
//! badge in the toolbar.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use egui::{
    Align, Align2, Color32, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    TextureHandle, Vec2,
};
use pp6core::library::LibraryIndex;
use pp6core::{Command, NoticeLevel, Settings, ShowState, SlideRef};

use crate::theme::{self, color, icon, section_header, small_icon, tool_button};
use crate::thumbs::{ThumbKey, ThumbRequest, ThumbResult};

/// Every slide thumbnail (grid, preview, editor) is rendered at this width and scaled
/// for display, so the preview reuses the grid's texture: a slide change never waits
/// for a new thumbnail.
pub const THUMB_W: u32 = 400;
const TILE_DEFAULT: f32 = 200.0;
const TILE_RANGE: std::ops::RangeInclusive<f32> = 110.0..=400.0;
const TOAST_FOR: Duration = Duration::from_secs(8);

/// Things the UI asks the app to do after the frame.
pub enum UiAction {
    Core(Command),
    AddLibraryFolder,
    AddMediaFolder,
    OpenPlaylist,
    SavePlaylistAs,
    /// Output display chosen by description index (see `UiState::monitors`).
    ChooseMonitor(usize),
    /// mpv audio device name; `None` = system default.
    SetAudioDevice(Option<String>),
    RemoveLibraryFolder(usize),
    RemoveMediaFolder(usize),
    /// New phone-remote PIN (logs all phones out).
    NewPin,
    /// Turn the phone remote on/off (saved).
    RemoteEnabled(bool),
}

/// Phone remote status shown in the UI (refreshed by the app each frame).
#[derive(Default)]
pub struct RemoteStatus {
    pub url: Option<String>,
    pub pin: String,
    pub clients: usize,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    General,
    Folders,
    Keys,
    Log,
}

#[derive(Default)]
pub struct UiState {
    pub search: String,
    pub library: LibraryIndex,
    pub library_status: String,
    pub monitors: Vec<String>,
    pub media_files: Vec<PathBuf>,
    pub app_notices: Vec<String>,
    pub audio_devices: Vec<media::AudioDevice>,
    pub settings_open: bool,
    pub settings_tab: SettingsTab,
    pub settings_path: String,
    pub remote: RemoteStatus,
    pub edit: crate::editor::EditState,
    /// Video/Image panel hidden (toolbar toggle).
    pub media_hidden: bool,
    /// Media folder shown in the Video/Image panel (`None` = all).
    pub media_bin: Option<usize>,
    /// Slide tile width in the grid (0 = default).
    pub tile_w: f32,
    /// Problems up to this time have been seen in Settings → Log.
    pub seen_notices_at: Option<Instant>,
    pub seen_app_notices: usize,
    /// When each app notice arrived (for the toast).
    pub(crate) app_notice_seen_count: usize,
    pub(crate) app_notice_at: Option<Instant>,
    /// Last image shown in the preview: kept on screen until the next one is ready.
    pub(crate) last_preview: Option<TextureHandle>,
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
        // Cheap bound on memory.
        if self.textures.len() > 2000 {
            self.textures.clear();
            self.requested.clear();
        }
    }

    pub(crate) fn thumb_public(
        &mut self,
        tx: &Sender<ThumbRequest>,
        doc: &Arc<pro6::Document>,
        flat: usize,
    ) -> (ThumbKey, Option<&Result<TextureHandle, String>>) {
        self.thumb(tx, doc, flat)
    }

    fn thumb(
        &mut self,
        tx: &Sender<ThumbRequest>,
        doc: &Arc<pro6::Document>,
        flat: usize,
    ) -> (ThumbKey, Option<&Result<TextureHandle, String>>) {
        let width = THUMB_W;
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

    fn tile_width(&self) -> f32 {
        if self.tile_w <= 0.0 {
            TILE_DEFAULT
        } else {
            self.tile_w.clamp(*TILE_RANGE.start(), *TILE_RANGE.end())
        }
    }

    /// Problems (warnings/errors) not yet seen in Settings → Log.
    fn unseen_problems(&self, show: &ShowState) -> usize {
        let core = show
            .notices
            .iter()
            .filter(|n| n.level != NoticeLevel::Info)
            .filter(|n| self.seen_notices_at.is_none_or(|t| n.at > t))
            .count();
        core + self.app_notices.len().saturating_sub(self.seen_app_notices)
    }

    fn mark_seen(&mut self, show: &ShowState) {
        self.seen_notices_at = show
            .notices
            .iter()
            .map(|n| n.at)
            .max()
            .or(self.seen_notices_at);
        self.seen_app_notices = self.app_notices.len();
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
    if st.app_notices.len() != st.app_notice_seen_count {
        st.app_notice_seen_count = st.app_notices.len();
        st.app_notice_at = Some(Instant::now());
    }
    toolbar(ui, st, show, settings, &mut actions);
    egui::Panel::left("left")
        .resizable(true)
        .default_size(330.0)
        .size_range(240.0..=640.0)
        .frame(egui::Frame::new().fill(color::PANEL))
        .show(ui, |ui| {
            egui::Panel::top("preview-panel")
                .resizable(false)
                .frame(egui::Frame::new().fill(color::PANEL))
                .show(ui, |ui| preview(ui, st, show, thumbs, &mut actions));
            let half = (ui.available_height() * 0.5).max(140.0);
            egui::Panel::bottom("playlist-panel")
                .resizable(true)
                .default_size(half)
                .size_range(110.0..=(ui.available_height() - 110.0).max(120.0))
                .frame(egui::Frame::new().fill(color::LIST))
                .show(ui, |ui| playlist(ui, show, &mut actions));
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(color::LIST))
                .show(ui, |ui| library(ui, st, show, settings, &mut actions));
        });
    let mut media_open = !st.media_hidden;
    egui::Panel::bottom("media-panel")
        .resizable(true)
        .default_size(230.0)
        .size_range(120.0..=600.0)
        .frame(egui::Frame::new().fill(color::LIST))
        .show_collapsible(ui, &mut media_open, |ui| {
            media_panel(ui, st, show, settings, &mut actions)
        });
    st.media_hidden = !media_open;
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(color::LIST))
        .show(ui, |ui| presentation(ui, st, show, thumbs, &mut actions));
    settings_window(ui.ctx(), st, show, settings, log_lines, &mut actions);
    toast(ui.ctx(), st, show);
    actions
}

fn keys(list: &[String]) -> String {
    list.first().map(|k| format!(" ({k})")).unwrap_or_default()
}

pub(crate) fn title(path: &Path) -> String {
    path.file_stem().map_or_else(
        || path.display().to_string(),
        |s| s.to_string_lossy().into_owned(),
    )
}

// ───────────────────────────── toolbar ─────────────────────────────

fn toolbar(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    settings: &Settings,
    actions: &mut Vec<UiAction>,
) {
    egui::Panel::top("toolbar")
        .frame(
            egui::Frame::new()
                .fill(Color32::from_rgb(52, 52, 56))
                .inner_margin(egui::Margin::symmetric(8, 4)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let k = &settings.keys;
                let text_off = !show.text_visible;
                let bg_off = !show.background_visible;
                if tool_button(
                    ui,
                    icon::X,
                    "Clear All",
                    Color32::from_rgb(110, 110, 116),
                    text_off && bg_off,
                    true,
                )
                .on_hover_text(format!("Clear All{}", keys(&k.clear_all)))
                .clicked()
                {
                    actions.push(UiAction::Core(Command::ClearAll));
                }
                if tool_button(
                    ui,
                    icon::TEXT_T_SLASH,
                    "Slides",
                    Color32::from_rgb(214, 68, 58),
                    text_off,
                    true,
                )
                .on_hover_text(format!("Clear the slide text{}", keys(&k.clear_text)))
                .clicked()
                {
                    actions.push(UiAction::Core(Command::ClearText));
                }
                if tool_button(
                    ui,
                    icon::IMAGE,
                    "BkGrds",
                    Color32::from_rgb(47, 143, 216),
                    bg_off,
                    true,
                )
                .on_hover_text(format!("Clear the background{}", keys(&k.clear_background)))
                .clicked()
                {
                    actions.push(UiAction::Core(Command::ClearBackground));
                }
                toolbar_gap(ui);
                let can_edit = show
                    .selected
                    .as_ref()
                    .is_some_and(|p| show.docs.contains_key(p));
                if tool_button(
                    ui,
                    icon::PENCIL,
                    "Editor",
                    Color32::from_rgb(232, 116, 42),
                    st.edit.on,
                    can_edit || st.edit.on,
                )
                .on_hover_text("Edit text, slides, groups and arrangements")
                .clicked()
                {
                    st.edit.on = !st.edit.on;
                }
                toolbar_gap(ui);
                if tool_button(
                    ui,
                    icon::FILM_STRIP,
                    "Video/Image",
                    Color32::from_rgb(110, 92, 214),
                    !st.media_hidden,
                    true,
                )
                .on_hover_text("Show or hide the backgrounds panel")
                .clicked()
                {
                    st.media_hidden = !st.media_hidden;
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if tool_button(
                        ui,
                        icon::GEAR,
                        "Settings",
                        Color32::from_rgb(110, 110, 116),
                        st.settings_open,
                        true,
                    )
                    .clicked()
                    {
                        st.settings_open = !st.settings_open;
                    }
                    remote_button(ui, st, settings, actions);
                    display_button(ui, st, settings, actions);
                    let on = show.output_enabled;
                    let out = tool_button(
                        ui,
                        icon::MONITOR,
                        "Output",
                        if on {
                            Color32::from_rgb(52, 168, 83)
                        } else {
                            Color32::from_rgb(110, 110, 116)
                        },
                        on,
                        true,
                    )
                    .on_hover_text(if on {
                        "Output is ON. Click to turn it off (releases the display)."
                    } else {
                        "Output is OFF. Click to turn it on."
                    });
                    if out.clicked() {
                        actions.push(UiAction::Core(Command::OutputEnable(!on)));
                    }
                    let problems = st.unseen_problems(show);
                    if problems > 0 {
                        toolbar_gap(ui);
                        if tool_button(
                            ui,
                            icon::WARNING,
                            &format!("{problems} new"),
                            Color32::from_rgb(214, 160, 30),
                            false,
                            true,
                        )
                        .on_hover_text("Problems were reported. Click to read them.")
                        .clicked()
                        {
                            st.settings_open = true;
                            st.settings_tab = SettingsTab::Log;
                        }
                    }
                });
            });
        });
}

fn toolbar_gap(ui: &mut egui::Ui) {
    ui.add_space(6.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(1.0, 44.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, Color32::from_gray(75));
    ui.add_space(6.0);
}

fn display_button(
    ui: &mut egui::Ui,
    st: &UiState,
    settings: &Settings,
    actions: &mut Vec<UiAction>,
) {
    let current = settings.output.monitor_name.clone();
    let resp = tool_button(
        ui,
        icon::PROJECTOR_SCREEN,
        "Display",
        Color32::from_rgb(47, 143, 216),
        false,
        true,
    )
    .on_hover_text(format!(
        "Output display: {}",
        current.as_deref().unwrap_or("automatic")
    ));
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(300.0);
        ui.label(RichText::new("Show the output on").strong());
        if st.monitors.is_empty() {
            ui.label(RichText::new("No displays found").weak());
        }
        for (i, m) in st.monitors.iter().enumerate() {
            let selected = current.as_deref().is_some_and(|c| m.contains(c));
            if ui.selectable_label(selected, m).clicked() {
                actions.push(UiAction::ChooseMonitor(i));
            }
        }
    });
}

fn remote_button(
    ui: &mut egui::Ui,
    st: &UiState,
    settings: &Settings,
    actions: &mut Vec<UiAction>,
) {
    let running = st.remote.url.is_some();
    let caption = if st.remote.clients > 0 {
        format!("Remote ({})", st.remote.clients)
    } else {
        "Remote".to_owned()
    };
    let resp = tool_button(
        ui,
        icon::DEVICE_MOBILE,
        &caption,
        if running {
            Color32::from_rgb(28, 160, 150)
        } else {
            Color32::from_rgb(110, 110, 116)
        },
        false,
        true,
    )
    .on_hover_text("Phone remote");
    egui::Popup::menu(&resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(280.0);
            ui.label(RichText::new("Phone remote").strong());
            let mut on = settings.web.enabled;
            if ui.checkbox(&mut on, "Enabled").changed() {
                actions.push(UiAction::RemoteEnabled(on));
            }
            match &st.remote.url {
                Some(url) => {
                    ui.label("On a phone on the same network, open:");
                    ui.label(RichText::new(url).monospace().strong());
                    ui.horizontal(|ui| {
                        ui.label("PIN");
                        ui.label(
                            RichText::new(&st.remote.pin)
                                .monospace()
                                .strong()
                                .size(20.0),
                        );
                    });
                    ui.label(
                        RichText::new(format!("{} phone(s) connected", st.remote.clients)).weak(),
                    );
                    if ui
                        .button("New PIN")
                        .on_hover_text("Logs out every phone")
                        .clicked()
                    {
                        actions.push(UiAction::NewPin);
                    }
                }
                None if settings.web.enabled => {
                    ui.label(RichText::new("Not running (see Settings → Log).").weak());
                }
                None => {}
            }
        });
}

// ───────────────────────────── preview ─────────────────────────────

fn preview(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    thumbs: &Sender<ThumbRequest>,
    actions: &mut Vec<UiAction>,
) {
    section_header(ui, "Preview", |ui| {
        if show.live.is_some() {
            ui.label(
                RichText::new("LIVE")
                    .small()
                    .strong()
                    .color(color::LIVE_TEXT),
            );
            let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, color::LIVE);
        }
    });
    let w = ui.available_width();
    let aspect = show.live.as_ref().map_or(0.75, |l| {
        l.doc.canvas.height as f32 / l.doc.canvas.width as f32
    });
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, (w * aspect).round()), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
    match &show.live {
        Some(l) if show.text_visible => {
            let (_, tex) = st.thumb(thumbs, &l.doc, l.slide.flat);
            match tex {
                Some(Ok(t)) => {
                    let t = t.clone();
                    egui::Image::new(&t).paint_at(ui, rect);
                    st.last_preview = Some(t);
                }
                Some(Err(e)) => {
                    let e = e.clone();
                    paint_error(ui, rect, &e);
                }
                // Not ready yet: keep the previous image rather than flash black.
                None => {
                    if let Some(t) = &st.last_preview {
                        egui::Image::new(t).paint_at(ui, rect);
                    }
                }
            }
            // Have the likely next slide's thumbnail ready too.
            if let Some((_, doc, _, flat)) = show.next_slide() {
                let _ = st.thumb(thumbs, &doc, flat);
            }
        }
        Some(_) => {}
        None => st.last_preview = None,
    }
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        match &show.live {
            Some(l) => {
                let group = l
                    .doc
                    .groups()
                    .get(l.slide.group)
                    .map(|g| g.name.clone())
                    .unwrap_or_default();
                let pos = l.position.map_or("–".into(), |p| (p + 1).to_string());
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "{} · {group} · {pos}/{}",
                            title(&l.path),
                            l.order.len()
                        ))
                        .size(12.5)
                        .color(color::LIVE_TEXT),
                    )
                    .truncate(),
                );
            }
            None => {
                ui.label(RichText::new("Nothing live").size(12.5).weak());
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let state = match (show.text_visible, show.background_visible) {
                (true, true) => None,
                (false, true) => Some("Text cleared"),
                (true, false) => Some("Background cleared"),
                (false, false) => Some("All cleared"),
            };
            if let Some(s) = state {
                ui.label(
                    RichText::new(s)
                        .size(12.5)
                        .color(Color32::from_rgb(230, 110, 100)),
                );
            }
        });
    });
    loop_panel(ui, show, actions);
    ui.add_space(4.0);
}

fn paint_error(ui: &egui::Ui, rect: Rect, message: &str) {
    ui.painter()
        .rect_filled(rect, 0.0, Color32::from_rgb(60, 10, 10));
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        "can't show",
        FontId::proportional(12.0),
        Color32::LIGHT_RED,
    );
    ui.interact(
        rect,
        ui.scope_id()
            .with(("err", rect.min.x as i32, rect.min.y as i32)),
        Sense::hover(),
    )
    .on_hover_text(message);
}

/// Loop indicator + Start/Stop, shown under the preview.
fn loop_panel(ui: &mut egui::Ui, show: &ShowState, actions: &mut Vec<UiAction>) {
    if let Some(l) = &show.looping {
        egui::Frame::new()
            .stroke(Stroke::new(2.0, color::LIVE))
            .fill(Color32::from_rgb(50, 32, 10))
            .inner_margin(6.0)
            .outer_margin(egui::Margin::symmetric(6, 2))
            .corner_radius(4.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(theme::icon_text(icon::REPEAT, 14.0).color(color::LIVE_TEXT));
                    ui.label(
                        RichText::new("LOOP RUNNING")
                            .strong()
                            .color(color::LIVE_TEXT),
                    );
                    let left = match l.next_at {
                        Some(t) => {
                            let secs = t.saturating_duration_since(Instant::now()).as_secs_f32();
                            format!("next in {:.0} s", secs.ceil())
                        }
                        None => "no stored time: waiting".to_owned(),
                    };
                    ui.label(left);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Stop loop").clicked() {
                            actions.push(UiAction::Core(Command::StopLoop));
                        }
                    });
                });
                ui.label(RichText::new(title(&l.path)).small().weak());
            });
        // Keep the countdown moving.
        ui.ctx().request_repaint_after(Duration::from_millis(250));
        return;
    }
    let candidate = show
        .live
        .as_ref()
        .map(|l| l.path.clone())
        .or_else(|| show.selected.clone());
    if let Some(path) = candidate
        && show
            .docs
            .get(&path)
            .is_some_and(|d| d.slide_timings().is_some())
    {
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            if ui
                .button(RichText::new(format!("{} Start loop", icon::REPEAT)))
                .on_hover_text("Advance on its own using the slide timing stored in the file")
                .clicked()
            {
                actions.push(UiAction::Core(Command::StartLoop));
            }
            ui.add(
                egui::Label::new(
                    RichText::new(format!("{} has slide timers", title(&path)))
                        .small()
                        .weak(),
                )
                .truncate(),
            );
        });
    }
}

// ───────────────────────────── library ─────────────────────────────

fn library(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    settings: &Settings,
    actions: &mut Vec<UiAction>,
) {
    let search = &mut st.search;
    section_header(ui, "Library", |ui| {
        if small_icon(ui, icon::FOLDER_PLUS, "Add a library folder").clicked() {
            actions.push(UiAction::AddLibraryFolder);
        }
        ui.add(
            egui::TextEdit::singleline(search)
                .hint_text(format!("{} Search", icon::MAGNIFYING_GLASS))
                .desired_width(ui.available_width()),
        );
    });
    if settings.library_dirs.is_empty() {
        egui::Frame::new().inner_margin(8).show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new("No library folders yet. Add the folder with your .pro6 files.")
                        .italics(),
                )
                .wrap(),
            );
            if ui.button("Add library folder…").clicked() {
                actions.push(UiAction::AddLibraryFolder);
            }
        });
        return;
    }
    let results = st.library.search(&st.search);
    let status = if st.search.trim().is_empty() {
        st.library_status.clone()
    } else {
        format!("{} match(es)", results.len())
    };
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(RichText::new(status).small().weak());
    });
    let row_h = 20.0;
    egui::ScrollArea::vertical()
        .id_salt("library")
        .auto_shrink(false)
        .show_rows(ui, row_h, results.len(), |ui, range| {
            for i in range {
                let e = results[i];
                let selected = show.selected.as_deref() == Some(e.path.as_path());
                let live = show.is_live(&e.path);
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
                let p = ui.painter();
                if selected {
                    p.rect_filled(rect, 0.0, color::ACCENT);
                } else if resp.hovered() {
                    p.rect_filled(rect, 0.0, Color32::from_gray(55));
                } else if i % 2 == 1 {
                    p.rect_filled(rect, 0.0, Color32::from_rgb(39, 39, 42));
                }
                let text_color = if e.error.is_some() {
                    Color32::LIGHT_RED
                } else if live {
                    color::LIVE_TEXT
                } else {
                    Color32::from_gray(225)
                };
                let label = if e.error.is_some() {
                    format!("! {}", e.title)
                } else {
                    e.title.clone()
                };
                p.with_clip_rect(
                    rect.shrink2(Vec2::new(28.0, 0.0))
                        .translate(Vec2::new(-20.0, 0.0)),
                )
                .text(
                    rect.left_center() + Vec2::new(8.0, 0.0),
                    Align2::LEFT_CENTER,
                    label,
                    FontId::proportional(13.0),
                    text_color,
                );
                // Add-to-playlist button on the right of the row.
                let add_rect = Rect::from_center_size(
                    rect.right_center() - Vec2::new(14.0, 0.0),
                    Vec2::splat(18.0),
                );
                let add = ui
                    .interact(add_rect, ui.scope_id().with(("lib-add", i)), Sense::click())
                    .on_hover_text("Add to playlist");
                if resp.hovered() || add.hovered() || selected {
                    ui.painter().text(
                        add_rect.center(),
                        Align2::CENTER_CENTER,
                        icon::PLUS,
                        theme::icon_font(13.0),
                        if add.hovered() {
                            Color32::WHITE
                        } else {
                            Color32::from_gray(170)
                        },
                    );
                }
                let resp = match &e.error {
                    Some(err) => resp.on_hover_text(err),
                    None => resp,
                };
                let add_to_playlist = || {
                    UiAction::Core(Command::PlaylistAdd {
                        path: e.path.clone(),
                        arrangement: None,
                    })
                };
                if add.clicked() || resp.double_clicked() {
                    actions.push(add_to_playlist());
                } else if resp.clicked() {
                    actions.push(UiAction::Core(Command::Open(e.path.clone())));
                }
                resp.context_menu(|ui| {
                    if ui.button("Add to playlist").clicked() {
                        actions.push(add_to_playlist());
                        ui.close();
                    }
                });
            }
        });
}

// ───────────────────────────── playlist ─────────────────────────────

fn playlist(ui: &mut egui::Ui, show: &ShowState, actions: &mut Vec<UiAction>) {
    let name = show
        .playlist_path
        .as_deref()
        .map_or_else(|| "unsaved".to_owned(), title);
    section_header(ui, "Playlist", |ui| {
        ui.label(RichText::new(name).small().weak());
    });
    // Footer first so the list fills what's left.
    egui::Panel::bottom("playlist-footer")
        .exact_size(26.0)
        .frame(
            egui::Frame::new()
                .fill(color::HEADER)
                .inner_margin(egui::Margin::symmetric(6, 2)),
        )
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                if small_icon(ui, icon::FILE_PLUS, "New playlist").clicked() {
                    actions.push(UiAction::Core(Command::PlaylistNew));
                }
                if small_icon(ui, icon::FOLDER_OPEN, "Open playlist…").clicked() {
                    actions.push(UiAction::OpenPlaylist);
                }
                if small_icon(ui, icon::FLOPPY_DISK, "Save playlist").clicked() {
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
        });
    egui::ScrollArea::vertical()
        .id_salt("playlist")
        .auto_shrink(false)
        .show(ui, |ui| {
            let n = show.playlist.items.len();
            for (i, item) in show.playlist.items.iter().enumerate() {
                let live = show.is_live(&item.path);
                let selected = show.playlist_item == Some(i);
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
                let p = ui.painter();
                if selected {
                    p.rect_filled(rect, 0.0, color::ACCENT);
                } else if resp.hovered() {
                    p.rect_filled(rect, 0.0, Color32::from_gray(55));
                } else if i % 2 == 1 {
                    p.rect_filled(rect, 0.0, Color32::from_rgb(39, 39, 42));
                }
                if live {
                    p.rect_filled(
                        Rect::from_min_size(rect.min, Vec2::new(4.0, rect.height())),
                        0.0,
                        color::LIVE,
                    );
                }
                let text_color = if show.load_errors.contains_key(&item.path) {
                    Color32::LIGHT_RED
                } else if live {
                    color::LIVE_TEXT
                } else {
                    Color32::from_gray(225)
                };
                p.with_clip_rect(Rect::from_min_max(rect.min, rect.max - Vec2::new(70.0, 0.0)))
                    .text(
                        rect.left_center() + Vec2::new(10.0, 0.0),
                        Align2::LEFT_CENTER,
                        format!("{}. {}", i + 1, title(&item.path)),
                        FontId::proportional(13.0),
                        text_color,
                    );
                if resp.clicked() {
                    actions.push(UiAction::Core(Command::PlaylistSelect(i)));
                }
                // Row controls (shown on hover / selection).
                if resp.hovered() || selected || ui.rect_contains_pointer(rect) {
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect.shrink2(Vec2::new(4.0, 1.0)))
                            .layout(Layout::right_to_left(Align::Center)),
                    );
                    if small_icon(&mut child, icon::TRASH, "Remove from playlist").clicked() {
                        actions.push(UiAction::Core(Command::PlaylistRemove(i)));
                    }
                    if i + 1 < n && small_icon(&mut child, icon::ARROW_DOWN, "Move down").clicked()
                    {
                        actions.push(UiAction::Core(Command::PlaylistMove { from: i, to: i + 1 }));
                    }
                    if i > 0 && small_icon(&mut child, icon::ARROW_UP, "Move up").clicked() {
                        actions.push(UiAction::Core(Command::PlaylistMove { from: i, to: i - 1 }));
                    }
                }
            }
            if n == 0 {
                egui::Frame::new().inner_margin(8).show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(
                                "Empty. Double-click a presentation in the library (or use +) to add it.",
                            )
                            .italics()
                            .weak(),
                        )
                        .wrap(),
                    );
                });
            }
        });
}

// ─────────────────────────── presentation ───────────────────────────

pub(crate) fn color32(c: Option<pro6::Rgba>) -> Color32 {
    let q = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    match c {
        Some(c) if c.a > 0.0 => Color32::from_rgb(q(c.r), q(c.g), q(c.b)),
        _ => Color32::from_gray(90),
    }
}

/// Black or white, whichever reads better on `bg`.
fn text_on(bg: Color32) -> Color32 {
    let l = 0.299 * f32::from(bg.r()) + 0.587 * f32::from(bg.g()) + 0.114 * f32::from(bg.b());
    if l > 150.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// A thumbnail at `size` (used by the editor grid).
pub(crate) fn tile_image(
    ui: &mut egui::Ui,
    tex: Option<&Result<TextureHandle, String>>,
    size: Vec2,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    paint_thumb(ui, rect, tex);
    response
}

fn paint_thumb(ui: &egui::Ui, rect: Rect, tex: Option<&Result<TextureHandle, String>>) {
    match tex {
        Some(Ok(t)) => {
            egui::Image::new(t).paint_at(ui, rect);
        }
        Some(Err(e)) => paint_error(ui, rect, e),
        None => {
            ui.painter().rect_filled(rect, 0.0, Color32::from_gray(18));
        }
    }
}

fn presentation(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    thumbs: &Sender<ThumbRequest>,
    actions: &mut Vec<UiAction>,
) {
    let Some(path) = show.selected.clone() else {
        section_header(ui, "Presentation", |_| {});
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new("Select a presentation in the library or playlist.").weak())
        });
        return;
    };
    let Some(doc) = show.docs.get(&path).cloned() else {
        section_header(ui, &title(&path), |_| {});
        ui.add_space(8.0);
        match show.load_errors.get(&path) {
            Some(e) => ui.colored_label(Color32::LIGHT_RED, e),
            None => ui.label(format!("Loading {}…", title(&path))),
        };
        return;
    };
    if st.edit.on {
        crate::editor::grid_edit(ui, st, show, thumbs, actions, &path, &doc);
        return;
    }
    let arrangement = show.arrangement_for(&path);
    let arrangements = doc.arrangements();
    let unsaved = if show.unsaved.contains(&path) {
        " •"
    } else {
        ""
    };
    section_header(ui, &format!("{}{unsaved}", title(&path)), |ui| {
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
        // Slide timers are supported; other cues and timeline/media tracks aren't yet.
        let timeline_cues = doc
            .timeline
            .as_ref()
            .is_some_and(|t| t.time_cue_count() + t.media_track_count() > 0);
        let other_cues = match doc.slide_timings() {
            Some(t) => t.other_cues,
            None => doc.slides().map(|s| s.cue_count()).sum(),
        };
        if timeline_cues || other_cues > 0 {
            ui.label(theme::icon_text(icon::WARNING, 14.0).color(Color32::YELLOW))
                .on_hover_text("Some cues (media / timeline) in this file aren't supported yet.");
        }
    });

    // Tile size slider, bottom right (as in PP6).
    egui::Panel::bottom("grid-footer")
        .exact_size(26.0)
        .frame(
            egui::Frame::new()
                .fill(color::HEADER)
                .inner_margin(egui::Margin::symmetric(8, 2)),
        )
        .show(ui, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut w = st.tile_width();
                ui.spacing_mut().slider_width = 110.0;
                if ui
                    .add(egui::Slider::new(&mut w, TILE_RANGE).show_value(false))
                    .on_hover_text("Slide size")
                    .changed()
                {
                    st.tile_w = w;
                }
                ui.label(theme::icon_text(icon::SQUARES_FOUR, 13.0).color(Color32::GRAY));
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let n = pp6core::slide_order(&doc, arrangement.as_deref()).len();
                    ui.label(RichText::new(format!("{n} slides")).small().weak());
                });
            });
        });

    let order: Vec<SlideRef> = pp6core::slide_order(&doc, arrangement.as_deref());
    let live = show.live.as_ref().filter(|l| l.path == path);
    let tile_w = st.tile_width();
    let img_h = (tile_w * doc.canvas.height as f32 / doc.canvas.width as f32).round();
    egui::ScrollArea::vertical()
        .id_salt("grid")
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.add_space(8.0);
            let gap = 10.0;
            let avail = ui.available_width().min(ui.clip_rect().width()) - 16.0;
            let per_row = ((avail + gap) / (tile_w + gap)).floor().max(1.0) as usize;
            for (row_i, row) in order.chunks(per_row).enumerate() {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.spacing_mut().item_spacing.x = gap;
                    for (j, r) in row.iter().enumerate() {
                        let pos = row_i * per_row + j;
                        let group = &doc.groups()[r.group];
                        // Group name on the first slide of each run, as in PP6.
                        let first_of_group = pos == 0 || order[pos - 1].group != r.group;
                        let mut label = format!("{}.", pos + 1);
                        if first_of_group && !group.name.is_empty() {
                            label.push_str(&format!(" [{}]", group.name));
                        }
                        if !r.enabled {
                            label.push_str(" (disabled)");
                        }
                        let is_live = live.is_some_and(|l| {
                            l.position == Some(pos)
                                || (l.position.is_none() && l.slide.flat == r.flat)
                        });
                        let (_, tex) = st.thumb(thumbs, &doc, r.flat);
                        let resp = slide_tile(
                            ui,
                            tex,
                            Vec2::new(tile_w, img_h),
                            &label,
                            color32(group.color),
                            is_live,
                            r.enabled,
                        );
                        if resp.clicked() {
                            actions.push(UiAction::Core(Command::GoTo {
                                presentation: path.clone(),
                                position: pos,
                            }));
                        }
                    }
                });
                ui.add_space(gap);
            }
        });
}

/// A PP6-style slide tile: the thumbnail with a label bar in the group colour under it.
fn slide_tile(
    ui: &mut egui::Ui,
    tex: Option<&Result<TextureHandle, String>>,
    img: Vec2,
    label: &str,
    group: Color32,
    live: bool,
    enabled: bool,
) -> egui::Response {
    let bar_h = 18.0;
    let (rect, resp) = ui.allocate_exact_size(img + Vec2::new(0.0, bar_h), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let img_rect = Rect::from_min_size(rect.min, img);
    let bar = Rect::from_min_max(Pos2::new(rect.min.x, img_rect.max.y), rect.max);
    paint_thumb(ui, img_rect, tex);
    let p = ui.painter();
    p.rect_filled(bar, 0.0, group);
    p.with_clip_rect(bar.shrink2(Vec2::new(4.0, 0.0))).text(
        bar.left_center() + Vec2::new(5.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.0),
        text_on(group),
    );
    if !enabled {
        p.rect_filled(rect, 0.0, Color32::from_black_alpha(160));
    }
    let stroke = if live {
        Stroke::new(3.0, color::LIVE)
    } else if resp.hovered() {
        Stroke::new(2.0, Color32::from_gray(200))
    } else {
        Stroke::new(1.0, Color32::from_gray(15))
    };
    p.rect_stroke(rect, 2.0, stroke, StrokeKind::Outside);
    resp
}

// ──────────────────────────── video/image ────────────────────────────

fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|e| {
            matches!(
                e.as_str(),
                "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp" | "tif" | "tiff"
            )
        })
}

fn media_panel(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    settings: &Settings,
    actions: &mut Vec<UiAction>,
) {
    section_header(ui, "Video/Image", |ui| {
        if small_icon(ui, icon::FOLDER_PLUS, "Add a media folder").clicked() {
            actions.push(UiAction::AddMediaFolder);
        }
        if show.background_media.is_some()
            && ui
                .button(RichText::new(format!("{} Stop background", icon::STOP)))
                .clicked()
        {
            actions.push(UiAction::Core(Command::SetBackground {
                media: None,
                next: None,
            }));
        }
    });
    if settings.media_dirs.is_empty() {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Add a folder of background images and videos.").italics());
            if ui.button("Add media folder…").clicked() {
                actions.push(UiAction::AddMediaFolder);
            }
        });
        return;
    }
    if st.media_bin.is_some_and(|b| b >= settings.media_dirs.len()) {
        st.media_bin = None;
    }
    // Folder list on the left (like PP6's media playlists).
    egui::Panel::left("media-bins")
        .resizable(true)
        .default_size(160.0)
        .size_range(100.0..=320.0)
        .frame(egui::Frame::new().fill(color::PANEL).inner_margin(4))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("media-bins")
                .show(ui, |ui| {
                    if ui.selectable_label(st.media_bin.is_none(), "All").clicked() {
                        st.media_bin = None;
                    }
                    for (i, d) in settings.media_dirs.iter().enumerate() {
                        let name = d.file_name().map_or_else(
                            || d.display().to_string(),
                            |n| n.to_string_lossy().into(),
                        );
                        if ui
                            .selectable_label(st.media_bin == Some(i), name)
                            .on_hover_text(d.display().to_string())
                            .clicked()
                        {
                            st.media_bin = Some(i);
                        }
                    }
                });
        });
    let files: Vec<&PathBuf> = match st.media_bin.and_then(|b| settings.media_dirs.get(b)) {
        Some(dir) => st
            .media_files
            .iter()
            .filter(|f| f.starts_with(dir))
            .collect(),
        None => st.media_files.iter().collect(),
    };
    egui::ScrollArea::vertical()
        .id_salt("media")
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.add_space(6.0);
            if files.is_empty() {
                ui.label(RichText::new("No images or videos in this folder.").weak());
            }
            let (w, h) = (128.0, 72.0);
            let gap = 8.0;
            let per_row = ((ui.available_width() - 12.0 + gap) / (w + gap))
                .floor()
                .max(1.0) as usize;
            for (ri, row) in files.chunks(per_row).enumerate() {
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    ui.spacing_mut().item_spacing.x = gap;
                    for (j, f) in row.iter().enumerate() {
                        let idx = ri * per_row + j;
                        let live = show.background_media.as_deref() == Some(f.as_path());
                        let resp = media_tile(ui, f, Vec2::new(w, h), live);
                        if resp.clicked() {
                            // Pre-load the next file in the list so moving on is instant.
                            let next = files.get(idx + 1).map(|p| (*p).clone());
                            actions.push(UiAction::Core(Command::SetBackground {
                                media: Some((*f).clone()),
                                next,
                            }));
                        }
                    }
                });
                ui.add_space(gap);
            }
        });
}

fn media_tile(ui: &mut egui::Ui, path: &Path, img: Vec2, live: bool) -> egui::Response {
    let bar_h = 16.0;
    let (rect, resp) = ui.allocate_exact_size(img + Vec2::new(0.0, bar_h), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let img_rect = Rect::from_min_size(rect.min, img);
    let p = ui.painter();
    p.rect_filled(img_rect, 0.0, Color32::from_rgb(20, 20, 24));
    let (glyph, tint) = if is_image(path) {
        (icon::IMAGE, Color32::from_rgb(90, 150, 220))
    } else {
        (icon::FILM_STRIP, Color32::from_rgb(140, 120, 230))
    };
    p.text(
        img_rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(28.0),
        tint,
    );
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let bar = Rect::from_min_max(Pos2::new(rect.min.x, img_rect.max.y), rect.max);
    p.rect_filled(bar, 0.0, Color32::from_rgb(28, 28, 30));
    p.with_clip_rect(bar.shrink2(Vec2::new(3.0, 0.0))).text(
        bar.left_center() + Vec2::new(4.0, 0.0),
        Align2::LEFT_CENTER,
        &name,
        FontId::proportional(11.0),
        if live {
            color::LIVE_TEXT
        } else {
            Color32::from_gray(210)
        },
    );
    let stroke = if live {
        Stroke::new(3.0, color::LIVE)
    } else if resp.hovered() {
        Stroke::new(2.0, Color32::from_gray(200))
    } else {
        Stroke::new(1.0, Color32::from_gray(15))
    };
    p.rect_stroke(rect, 2.0, stroke, StrokeKind::Outside);
    resp.on_hover_text(name)
}

// ───────────────────────────── settings ─────────────────────────────

fn settings_window(
    ctx: &egui::Context,
    st: &mut UiState,
    show: &ShowState,
    settings: &Settings,
    log_lines: &[String],
    actions: &mut Vec<UiAction>,
) {
    let mut open = st.settings_open;
    egui::Window::new("Settings")
        .open(&mut open)
        .default_width(560.0)
        .default_height(460.0)
        .default_pos(egui::pos2(360.0, 90.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (tab, name) in [
                    (SettingsTab::General, "General"),
                    (SettingsTab::Folders, "Folders"),
                    (SettingsTab::Keys, "Keys"),
                    (SettingsTab::Log, "Log"),
                ] {
                    ui.selectable_value(&mut st.settings_tab, tab, name);
                }
            });
            ui.separator();
            match st.settings_tab {
                SettingsTab::General => settings_general(ui, st, show, settings, actions),
                SettingsTab::Folders => settings_folders(ui, settings, actions),
                SettingsTab::Keys => settings_keys(ui, st, settings),
                SettingsTab::Log => {
                    st.mark_seen(show);
                    settings_log(ui, st, show, log_lines);
                }
            }
        });
    st.settings_open = open;
}

fn settings_general(
    ui: &mut egui::Ui,
    st: &UiState,
    show: &ShowState,
    settings: &Settings,
    actions: &mut Vec<UiAction>,
) {
    ui.heading("Output");
    let mut on = show.output_enabled;
    if ui.checkbox(&mut on, "Output enabled").changed() {
        actions.push(UiAction::Core(Command::OutputEnable(on)));
    }
    let current = settings
        .output
        .monitor_name
        .clone()
        .unwrap_or_else(|| "automatic (not the operator's display)".into());
    egui::ComboBox::from_id_salt("settings-monitor")
        .selected_text(current)
        .width(380.0)
        .show_ui(ui, |ui| {
            for (i, m) in st.monitors.iter().enumerate() {
                if ui.selectable_label(false, m).clicked() {
                    actions.push(UiAction::ChooseMonitor(i));
                }
            }
        });
    ui.separator();
    ui.heading("Audio");
    let label = |d: &media::AudioDevice| {
        if d.description.is_empty() {
            d.name.clone()
        } else {
            format!("{} ({})", d.description, d.name)
        }
    };
    let selected = settings
        .audio_device
        .as_ref()
        .map(|n| {
            st.audio_devices
                .iter()
                .find(|d| &d.name == n)
                .map_or_else(|| n.clone(), label)
        })
        .unwrap_or_else(|| "System default".into());
    egui::ComboBox::from_id_salt("audio")
        .selected_text(selected)
        .width(380.0)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(settings.audio_device.is_none(), "System default")
                .clicked()
            {
                actions.push(UiAction::SetAudioDevice(None));
            }
            for d in &st.audio_devices {
                if ui
                    .selectable_label(
                        settings.audio_device.as_deref() == Some(d.name.as_str()),
                        label(d),
                    )
                    .clicked()
                {
                    actions.push(UiAction::SetAudioDevice(Some(d.name.clone())));
                }
            }
        });
    if st.audio_devices.is_empty() {
        ui.label(
            RichText::new("No audio devices listed (is libmpv-2.dll installed?).")
                .small()
                .weak(),
        );
    }
    ui.separator();
    ui.heading("Phone remote");
    let mut remote_on = settings.web.enabled;
    if ui.checkbox(&mut remote_on, "Enabled").changed() {
        actions.push(UiAction::RemoteEnabled(remote_on));
    }
    match &st.remote.url {
        Some(url) => {
            ui.label(format!(
                "Open {url} on a phone on the same network and enter PIN {}.",
                st.remote.pin
            ));
        }
        None if settings.web.enabled => {
            ui.label(RichText::new("Not running (see Log).").small());
        }
        None => {}
    }
    ui.label(
        RichText::new(format!(
            "Port {} and PIN length are in [web] in settings.toml. On first start Windows asks to allow network access; allow it on private networks.",
            settings.web.port
        ))
        .small()
        .weak(),
    );
}

fn settings_folders(ui: &mut egui::Ui, settings: &Settings, actions: &mut Vec<UiAction>) {
    ui.heading("Library (.pro6)");
    for (i, d) in settings.library_dirs.iter().enumerate() {
        ui.horizontal(|ui| {
            if ui.small_button("Remove").clicked() {
                actions.push(UiAction::RemoveLibraryFolder(i));
            }
            ui.label(d.display().to_string());
        });
    }
    if ui.button("Add library folder…").clicked() {
        actions.push(UiAction::AddLibraryFolder);
    }
    ui.separator();
    ui.heading("Media");
    for (i, d) in settings.media_dirs.iter().enumerate() {
        ui.horizontal(|ui| {
            if ui.small_button("Remove").clicked() {
                actions.push(UiAction::RemoveMediaFolder(i));
            }
            ui.label(d.display().to_string());
        });
    }
    if ui.button("Add media folder…").clicked() {
        actions.push(UiAction::AddMediaFolder);
    }
}

fn settings_keys(ui: &mut egui::Ui, st: &UiState, settings: &Settings) {
    let k = &settings.keys;
    egui::Grid::new("keys")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            for (name, list) in [
                ("Next", &k.next),
                ("Previous", &k.prev),
                ("Clear Text", &k.clear_text),
                ("Clear Background", &k.clear_background),
                ("Clear All", &k.clear_all),
            ] {
                ui.label(name);
                ui.label(list.join(", "));
                ui.end_row();
            }
        });
    ui.add_space(6.0);
    ui.label(
        RichText::new(format!(
            "To change keys, edit [keys] in {} and restart.",
            st.settings_path
        ))
        .small()
        .weak(),
    );
}

fn settings_log(ui: &mut egui::Ui, st: &UiState, show: &ShowState, log_lines: &[String]) {
    egui::ScrollArea::vertical()
        .id_salt("log")
        .auto_shrink(false)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.label(RichText::new("Messages").strong());
            if show.notices.is_empty() && st.app_notices.is_empty() {
                ui.label(RichText::new("No messages.").weak());
            }
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
            ui.separator();
            ui.label(RichText::new("Log").strong());
            for l in log_lines {
                ui.label(
                    RichText::new(l)
                        .small()
                        .monospace()
                        .color(Color32::from_gray(170)),
                );
            }
        });
}

/// The newest problem, briefly, in the bottom-right corner (click to read the log).
fn toast(ctx: &egui::Context, st: &mut UiState, show: &ShowState) {
    if st.settings_open && st.settings_tab == SettingsTab::Log {
        return;
    }
    let now = Instant::now();
    let core = show
        .notices
        .iter()
        .filter(|n| n.level != NoticeLevel::Info)
        .filter(|n| st.seen_notices_at.is_none_or(|t| n.at > t))
        .max_by_key(|n| n.at)
        .map(|n| (n.at, n.text.clone(), n.level == NoticeLevel::Error));
    let app = (st.app_notices.len() > st.seen_app_notices)
        .then(|| st.app_notices.last().cloned().zip(st.app_notice_at))
        .flatten()
        .map(|(t, at)| (at, t, true));
    let newest = match (core, app) {
        (Some(a), Some(b)) => Some(if a.0 >= b.0 { a } else { b }),
        (a, b) => a.or(b),
    };
    let Some((at, text, error)) = newest else {
        return;
    };
    let age = now.saturating_duration_since(at);
    if age > TOAST_FOR {
        return;
    }
    ctx.request_repaint_after(TOAST_FOR - age);
    egui::Area::new(egui::Id::unique("toast"))
        .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-16.0, -16.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let resp = egui::Frame::new()
                .fill(if error {
                    Color32::from_rgb(90, 24, 24)
                } else {
                    Color32::from_rgb(80, 64, 10)
                })
                .stroke(Stroke::new(1.0, Color32::from_gray(140)))
                .corner_radius(6.0)
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.set_max_width(420.0);
                    ui.horizontal(|ui| {
                        ui.label(theme::icon_text(icon::WARNING, 16.0).color(Color32::YELLOW));
                        ui.add(egui::Label::new(RichText::new(text).color(Color32::WHITE)).wrap());
                    });
                })
                .response
                .interact(Sense::click())
                .on_hover_text("Click to open the log");
            if resp.clicked() {
                st.settings_open = true;
                st.settings_tab = SettingsTab::Log;
            }
        });
}
