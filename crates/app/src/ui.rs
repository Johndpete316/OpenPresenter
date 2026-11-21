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
use pp6core::{Command, ItemRef, NoticeLevel, Settings, ShowState, SlideRef};

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
    /// Pick a playlist file and copy it in.
    ImportPlaylist,
    /// Delete a playlist (after asking).
    DeletePlaylist(usize),
    /// Output display chosen by description index (see `UiState::monitors`).
    ChooseMonitor(usize),
    /// Output display mode (`None` = the display's current one).
    SetResolution(Option<pp6core::Resolution>),
    /// How background media fills the output.
    SetMediaScaling(pp6core::MediaScaling),
    SetFitMinSize(f64),
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
    /// Resolutions the output display offers (filled when the output opens).
    pub output_resolutions: Vec<(u32, u32)>,
    /// Media folders, scanned recursively (see `media_scan`).
    pub media: crate::media_scan::MediaScan,
    pub media_scanning: bool,
    /// Video/Image tile thumbnails (`None` if the worker couldn't start).
    pub media_thumbs: Option<crate::media_thumbs::MediaThumbs>,
    pub app_notices: Vec<String>,
    pub audio_devices: Vec<media::AudioDevice>,
    pub settings_open: bool,
    pub settings_tab: SettingsTab,
    pub settings_path: String,
    pub playlists_path: String,
    /// Playlist whose name is being edited, and the text so far.
    pub renaming: Option<(usize, String)>,
    /// Playlist that library double-click / + adds to (the last one used).
    pub target_playlist: usize,
    pub remote: RemoteStatus,
    pub edit: crate::editor::EditState,
    /// Editor canvas renderer (`None` if it couldn't start).
    pub canvas: Option<crate::canvas::CanvasRenderer>,
    /// Video/Image panel hidden (toolbar toggle).
    pub media_hidden: bool,
    /// Folder shown in the Video/Image panel (`None` = everything).
    pub media_bin: Option<PathBuf>,
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
    /// Thumbnails whose slide has overflowing text.
    pub(crate) thumb_overflow: HashSet<ThumbKey>,
    /// Fonts for the editor (caret layout, font picker), once loaded.
    pub fonts_rx: Option<crossbeam_channel::Receiver<render::Fonts>>,
    pub fonts: Option<render::Fonts>,
    /// Installed font families (from `fonts`).
    pub families: Vec<String>,
    pub(crate) requested: HashSet<ThumbKey>,
}

impl UiState {
    /// Where library double-click / + adds: the playlist of the selected item, else the
    /// last playlist clicked, else the first.
    fn add_target(&self, show: &ShowState) -> usize {
        show.playlist_item
            .map(|r| r.playlist)
            .filter(|&p| p < show.playlists.len())
            .unwrap_or(
                self.target_playlist
                    .min(show.playlists.len().saturating_sub(1)),
            )
    }

    /// Take finished thumbnails into egui textures.
    pub fn receive_thumbs(
        &mut self,
        ctx: &egui::Context,
        rx: &crossbeam_channel::Receiver<ThumbResult>,
    ) {
        while let Ok(r) = rx.try_recv() {
            if r.overflow {
                self.thumb_overflow.insert(r.key);
            } else {
                self.thumb_overflow.remove(&r.key);
            }
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
            self.thumb_overflow.clear();
        }
        if self.fonts.is_none()
            && let Some(f) = self.fonts_rx.as_ref().and_then(|rx| rx.try_recv().ok())
        {
            self.families = f.families();
            self.fonts = Some(f);
        }
    }

    /// Editor thumbnail: rendered fresh (the editor copy isn't the file on disk).
    pub(crate) fn thumb_public(
        &mut self,
        tx: &Sender<ThumbRequest>,
        doc: &Arc<doc::Document>,
        flat: usize,
    ) -> (ThumbKey, Option<&Result<TextureHandle, String>>) {
        self.thumb_with(tx, doc, flat, false)
    }

    fn thumb(
        &mut self,
        tx: &Sender<ThumbRequest>,
        doc: &Arc<doc::Document>,
        flat: usize,
    ) -> (ThumbKey, Option<&Result<TextureHandle, String>>) {
        self.thumb_with(tx, doc, flat, true)
    }

    fn thumb_with(
        &mut self,
        tx: &Sender<ThumbRequest>,
        doc: &Arc<doc::Document>,
        flat: usize,
        disk: bool,
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
            disk,
        };
        if !self.requested.contains(&key) {
            self.requested.insert(key);
            let _ = tx.send(ThumbRequest {
                key,
                doc: doc.clone(),
                disk,
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

    /// Tell the operator about a problem (toast, and Settings → Log).
    pub fn toast(&mut self, message: impl Into<String>) {
        let m = message.into();
        tracing::warn!(message = %m, "app notice");
        self.app_notices.push(m);
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
    st.edit.fit_min_size = settings.editor.fit_min_size;
    // The editor takes the whole window under the toolbar. It shows its own copy
    // (unsaved edits), not the saved document.
    if st.edit.on
        && let Some(path) = show.selected.clone()
        && let Some(d) = show.edited_doc(&path).cloned()
    {
        crate::editor::editor(ui, st, show, thumbs, &mut actions, &path, &d);
        settings_window(ui.ctx(), st, show, settings, log_lines, &mut actions);
        toast(ui.ctx(), st, show);
        return actions;
    }
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
                .show(ui, |ui| playlists(ui, st, show, &mut actions));
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
    drag_label(ui.ctx(), show);
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
                if tool_button(
                    ui,
                    icon::X,
                    "Clear All",
                    Color32::from_rgb(110, 110, 116),
                    false,
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
                    false,
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
                    false,
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
                .on_hover_text("Edit slides: text, layout, groups and arrangements")
                .clicked()
                {
                    if st.edit.on {
                        // Leaving asks about unsaved edits first.
                        st.edit.close_requested = true;
                    } else {
                        st.edit.on = true;
                    }
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
    });
    loop_panel(ui, show, actions);
    ui.add_space(4.0);
}

pub(crate) fn paint_error(ui: &egui::Ui, rect: Rect, message: &str) {
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
    let target = st.add_target(show);
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
                let (rect, resp) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), row_h),
                    Sense::click_and_drag(),
                );
                // Drag into the playlist, as in PP6.
                if e.error.is_none() {
                    resp.dnd_set_drag_payload(PlaylistDrag::Library(e.path.clone()));
                }
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
                #[cfg(test)]
                st_probe::record(&e.title, rect, add_rect);
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
                let add_to = |playlist: usize| {
                    UiAction::Core(Command::PlaylistAdd {
                        playlist,
                        path: e.path.clone(),
                        arrangement: None,
                    })
                };
                let add_to_playlist = || add_to(target);
                if add.clicked() || resp.double_clicked() {
                    actions.push(add_to_playlist());
                } else if resp.clicked() {
                    actions.push(UiAction::Core(Command::Open(e.path.clone())));
                }
                resp.context_menu(|ui| {
                    if show.playlists.is_empty() && ui.button("Add to a new playlist").clicked() {
                        actions.push(add_to(0));
                        ui.close();
                    }
                    for (pi, p) in show.playlists.iter().enumerate() {
                        if ui.button(format!("Add to '{}'", p.playlist.name)).clicked() {
                            actions.push(add_to(pi));
                            ui.close();
                        }
                    }
                });
            }
        });
}

// ───────────────────────────── playlist ─────────────────────────────

fn playlists(ui: &mut egui::Ui, st: &mut UiState, show: &ShowState, actions: &mut Vec<UiAction>) {
    section_header(ui, "Playlists", |ui| {
        if small_icon(ui, icon::PLUS, "New playlist").clicked() {
            actions.push(UiAction::Core(Command::PlaylistCreate(
                "New playlist".into(),
            )));
            // Name it straight away (it appears at the end once the core has it).
            st.renaming = Some((show.playlists.len(), "New playlist".into()));
        }
    });
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
                    actions.push(UiAction::Core(Command::PlaylistCreate(
                        "New playlist".into(),
                    )));
                    st.renaming = Some((show.playlists.len(), "New playlist".into()));
                }
                if small_icon(ui, icon::FOLDER_OPEN, "Import a playlist file…").clicked() {
                    actions.push(UiAction::ImportPlaylist);
                }
                ui.label(RichText::new("Saved automatically").small().weak());
            });
        });
    egui::ScrollArea::vertical()
        .id_salt("playlists")
        .auto_shrink(false)
        .show(ui, |ui| {
            for (pi, p) in show.playlists.iter().enumerate() {
                playlist_node(ui, st, show, pi, p, actions);
            }
            // The space below: dropping there adds to the last playlist (or a new one).
            let h = ui.available_height().max(48.0);
            let (rest, rest_resp) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
            let dragging = rest_resp.dnd_hover_payload::<PlaylistDrag>().is_some();
            let last = show.playlists.len().checked_sub(1);
            if dragging {
                ui.painter()
                    .hline(rest.x_range(), rest.top(), Stroke::new(2.0, color::LIVE));
            }
            if let Some(d) = rest_resp.dnd_release_payload::<PlaylistDrag>() {
                let pl = last.unwrap_or(0);
                let n = show.playlists.get(pl).map_or(0, |p| p.playlist.items.len());
                drop_into_playlist(&d, pl, n, n, show, actions);
            }
            if show.playlists.is_empty() {
                let painter = ui.painter();
                painter.text(
                    rest.center_top() + Vec2::new(0.0, 24.0),
                    Align2::CENTER_CENTER,
                    if dragging {
                        "Drop to start a playlist"
                    } else {
                        "No playlists yet: click + to make one"
                    },
                    FontId::proportional(13.0),
                    Color32::from_gray(140),
                );
                painter.text(
                    rest.center_top() + Vec2::new(0.0, 44.0),
                    Align2::CENTER_CENTER,
                    "(or drag a presentation here from the library)",
                    FontId::proportional(11.0),
                    Color32::from_gray(110),
                );
            }
        });
}

/// One playlist: a collapsible header (name, count) with its items under it.
fn playlist_node(
    ui: &mut egui::Ui,
    st: &mut UiState,
    show: &ShowState,
    pi: usize,
    p: &pp6core::PlaylistFile,
    actions: &mut Vec<UiAction>,
) {
    let items = &p.playlist.items;
    let n = items.len();
    let has_live = items.iter().any(|it| show.is_live(&it.path));
    let id = ui.make_persistent_id(("playlist", pi, p.file.as_deref()));
    let mut state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true);
    let open = state.is_open();
    // Header row.
    let (rect, head) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, Color32::from_rgb(48, 48, 52));
    painter.text(
        rect.left_center() + Vec2::new(10.0, 0.0),
        Align2::CENTER_CENTER,
        if open {
            icon::CARET_DOWN
        } else {
            icon::CARET_RIGHT
        },
        theme::icon_font(11.0),
        Color32::from_gray(180),
    );
    painter.text(
        rect.left_center() + Vec2::new(22.0, 0.0),
        Align2::LEFT_CENTER,
        icon::LIST,
        theme::icon_font(13.0),
        if has_live {
            color::LIVE
        } else {
            Color32::from_gray(170)
        },
    );
    let renaming = st.renaming.as_ref().is_some_and(|(i, _)| *i == pi);
    if renaming {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    rect.min + Vec2::new(40.0, 2.0),
                    rect.max - Vec2::new(6.0, 2.0),
                ))
                .layout(Layout::left_to_right(Align::Center)),
        );
        if let Some((_, text)) = st.renaming.as_mut() {
            let edit = child.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
            if !edit.has_focus() && !edit.lost_focus() {
                edit.request_focus();
            }
            let enter = child.input(|i| i.key_pressed(egui::Key::Enter));
            let escape = child.input(|i| i.key_pressed(egui::Key::Escape));
            if escape {
                st.renaming = None;
            } else if enter || edit.lost_focus() {
                let name = text.trim().to_owned();
                if !name.is_empty() && name != p.playlist.name {
                    actions.push(UiAction::Core(Command::PlaylistRename {
                        playlist: pi,
                        name,
                    }));
                }
                st.renaming = None;
            }
        }
    } else {
        painter
            .with_clip_rect(Rect::from_min_max(
                rect.min,
                rect.max - Vec2::new(36.0, 0.0),
            ))
            .text(
                rect.left_center() + Vec2::new(40.0, 0.0),
                Align2::LEFT_CENTER,
                &p.playlist.name,
                FontId::proportional(13.5),
                Color32::from_gray(235),
            );
        painter.text(
            rect.right_center() - Vec2::new(8.0, 0.0),
            Align2::RIGHT_CENTER,
            n.to_string(),
            FontId::proportional(11.0),
            Color32::from_gray(140),
        );
    }
    if head.clicked() && !renaming {
        state.toggle(ui);
        st.target_playlist = pi;
    }
    if head.double_clicked() {
        st.renaming = Some((pi, p.playlist.name.clone()));
    }
    head.context_menu(|ui| {
        if ui.button("Rename").clicked() {
            st.renaming = Some((pi, p.playlist.name.clone()));
            ui.close();
        }
        if ui.button("Delete…").clicked() {
            actions.push(UiAction::DeletePlaylist(pi));
            ui.close();
        }
    });
    // Dropping on the header adds to the end of this playlist.
    if head.dnd_hover_payload::<PlaylistDrag>().is_some() {
        ui.painter()
            .rect_stroke(rect, 2.0, Stroke::new(2.0, color::LIVE), StrokeKind::Inside);
    }
    if let Some(d) = head.dnd_release_payload::<PlaylistDrag>() {
        drop_into_playlist(&d, pi, n, n, show, actions);
    }
    state.store(ui.ctx());
    if !open {
        return;
    }

    for (i, item) in items.iter().enumerate() {
        let me = ItemRef {
            playlist: pi,
            item: i,
        };
        let live = show.is_live(&item.path) && show.playlist_ref_of(&item.path) == Some(me);
        let selected = show.playlist_item == Some(me);
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), 22.0),
            Sense::click_and_drag(),
        );
        resp.dnd_set_drag_payload(PlaylistDrag::Item(me));
        // Dropping on a row inserts above or below it (whichever half).
        let below = ui
            .ctx()
            .pointer_interact_pos()
            .is_some_and(|p| p.y > rect.center().y);
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
                Rect::from_min_size(
                    rect.min + Vec2::new(14.0, 0.0),
                    Vec2::new(4.0, rect.height()),
                ),
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
        p.with_clip_rect(Rect::from_min_max(
            rect.min,
            rect.max - Vec2::new(70.0, 0.0),
        ))
        .text(
            rect.left_center() + Vec2::new(24.0, 0.0),
            Align2::LEFT_CENTER,
            format!("{}. {}", i + 1, title(&item.path)),
            FontId::proportional(13.0),
            text_color,
        );
        if resp.dnd_hover_payload::<PlaylistDrag>().is_some() {
            let y = if below { rect.bottom() } else { rect.top() };
            ui.painter()
                .hline(rect.x_range(), y, Stroke::new(2.0, color::LIVE));
        }
        if let Some(d) = resp.dnd_release_payload::<PlaylistDrag>() {
            drop_into_playlist(&d, pi, i + usize::from(below), n, show, actions);
        }
        if resp.clicked() {
            st.target_playlist = pi;
            actions.push(UiAction::Core(Command::PlaylistSelect(me)));
        }
        // Row controls (shown on hover / selection).
        if resp.hovered() || selected || ui.rect_contains_pointer(rect) {
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink2(Vec2::new(4.0, 1.0)))
                    .layout(Layout::right_to_left(Align::Center)),
            );
            if small_icon(&mut child, icon::TRASH, "Remove from playlist").clicked() {
                actions.push(UiAction::Core(Command::PlaylistRemove(me)));
            }
            if i + 1 < n && small_icon(&mut child, icon::ARROW_DOWN, "Move down").clicked() {
                actions.push(UiAction::Core(Command::PlaylistMove {
                    playlist: pi,
                    from: i,
                    to: i + 1,
                }));
            }
            if i > 0 && small_icon(&mut child, icon::ARROW_UP, "Move up").clicked() {
                actions.push(UiAction::Core(Command::PlaylistMove {
                    playlist: pi,
                    from: i,
                    to: i - 1,
                }));
            }
        }
    }
    if n == 0 {
        ui.horizontal(|ui| {
            ui.add_space(26.0);
            ui.label(
                RichText::new("Empty: drag presentations here")
                    .small()
                    .italics()
                    .weak(),
            );
        });
    }
    ui.add_space(2.0);
}

/// What's being dragged onto the playlist.
#[derive(Debug, Clone)]
enum PlaylistDrag {
    /// A presentation from the library.
    Library(PathBuf),
    /// An existing playlist item (reorder, or move to another playlist).
    Item(ItemRef),
}

/// Put the dragged thing at `index` (0..=n) of playlist `pl`, which has `n` items.
fn drop_into_playlist(
    d: &PlaylistDrag,
    pl: usize,
    index: usize,
    n: usize,
    show: &ShowState,
    actions: &mut Vec<UiAction>,
) {
    let add = |path: PathBuf, arrangement: Option<String>, actions: &mut Vec<UiAction>| {
        actions.push(UiAction::Core(Command::PlaylistAdd {
            playlist: pl,
            path,
            arrangement,
        }));
        if index < n {
            actions.push(UiAction::Core(Command::PlaylistMove {
                playlist: pl,
                from: n,
                to: index,
            }));
        }
    };
    match d {
        PlaylistDrag::Library(path) => add(path.clone(), None, actions),
        // Within the same playlist: reorder.
        &PlaylistDrag::Item(from) if from.playlist == pl => {
            let to = if index > from.item { index - 1 } else { index };
            if to != from.item && to < n {
                actions.push(UiAction::Core(Command::PlaylistMove {
                    playlist: pl,
                    from: from.item,
                    to,
                }));
            }
        }
        // To another playlist: move it (keeping its arrangement).
        &PlaylistDrag::Item(from) => {
            if let Some(item) = show.item(from) {
                add(item.path.clone(), item.arrangement.clone(), actions);
                actions.push(UiAction::Core(Command::PlaylistRemove(from)));
            }
        }
    }
}

/// While dragging, show what's being dragged next to the pointer.
fn drag_label(ctx: &egui::Context, show: &ShowState) {
    let Some(p) = egui::DragAndDrop::payload::<PlaylistDrag>(ctx) else {
        return;
    };
    let Some(pos) = ctx.pointer_latest_pos() else {
        return;
    };
    let text = match &*p {
        PlaylistDrag::Library(path) => title(path),
        PlaylistDrag::Item(r) => show.item(*r).map(|it| title(&it.path)).unwrap_or_default(),
    };
    egui::Area::new(egui::Id::unique("drag-label"))
        .order(egui::Order::Tooltip)
        .fixed_pos(pos + Vec2::new(14.0, 8.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(color::ACCENT)
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| ui.label(RichText::new(text).color(Color32::WHITE)));
        });
}

// ─────────────────────────── presentation ───────────────────────────

pub(crate) fn color32(c: Option<doc::Rgba>) -> Color32 {
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
        let r = doc.cue_report;
        if r.timeline_cues > 0 || r.other_cues > 0 {
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
    // A folder that disappeared (removed, or no media left in it) falls back to all.
    if let Some(sel) = &st.media_bin
        && !st.media.files.iter().any(|f| f.starts_with(sel))
    {
        st.media_bin = None;
    }
    // Folder tree on the left, mirroring the folders on disk.
    egui::Panel::left("media-bins")
        .resizable(true)
        .default_size(180.0)
        .size_range(110.0..=380.0)
        .frame(egui::Frame::new().fill(color::PANEL).inner_margin(4))
        .show(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt("media-bins")
                .auto_shrink(false)
                .show(ui, |ui| {
                    let all = format!("All ({})", st.media.files.len());
                    if ui.selectable_label(st.media_bin.is_none(), all).clicked() {
                        st.media_bin = None;
                    }
                    let roots = st.media.roots.clone();
                    for root in &roots {
                        folder_node(ui, &mut st.media_bin, root, true);
                    }
                    if st.media_scanning {
                        ui.label(RichText::new("Scanning folders…").small().weak());
                    }
                });
        });
    let groups = crate::media_scan::grouped(&st.media.files, st.media_bin.as_deref());
    // Click order for pre-loading "next": the order shown.
    let shown: Vec<&PathBuf> = groups.iter().flat_map(|(_, v)| v.iter().copied()).collect();
    let base = st.media_bin.clone();
    egui::ScrollArea::vertical()
        .id_salt("media")
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.add_space(6.0);
            if shown.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    let msg = if st.media_scanning {
                        "Scanning folders…"
                    } else {
                        "No images or videos here."
                    };
                    ui.label(RichText::new(msg).weak());
                });
            }
            let (w, h) = (128.0, 72.0);
            let gap = 8.0;
            let per_row = ((ui.available_width() - 12.0 + gap) / (w + gap))
                .floor()
                .max(1.0) as usize;
            let mut idx = 0;
            for (folder, files) in &groups {
                // Sub-folder heading (the selected folder's own files need none).
                if Some(folder) != base.as_ref() {
                    let label = folder_label(folder, base.as_deref(), &st.media.roots);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        ui.label(theme::icon_text(icon::FOLDER_OPEN, 12.0).color(Color32::GRAY));
                        ui.label(RichText::new(label).strong().color(Color32::from_gray(200)))
                            .on_hover_text(folder.display().to_string());
                    });
                    ui.add_space(2.0);
                }
                for row in files.chunks(per_row) {
                    ui.horizontal(|ui| {
                        ui.add_space(6.0);
                        ui.spacing_mut().item_spacing.x = gap;
                        for f in row {
                            let live = show.background_media.as_deref() == Some(f.as_path());
                            let resp =
                                media_tile(ui, st.media_thumbs.as_mut(), f, Vec2::new(w, h), live);
                            if resp.clicked() {
                                // Pre-load the next file shown so moving on is instant.
                                let next = shown.get(idx + 1).map(|p| (*p).clone());
                                actions.push(UiAction::Core(Command::SetBackground {
                                    media: Some((*f).clone()),
                                    next,
                                }));
                            }
                            idx += 1;
                        }
                    });
                    ui.add_space(gap);
                }
            }
        });
}

/// Heading for a group of files: its path below the selected folder, or (showing
/// everything) below its media folder, prefixed with that folder's name.
fn folder_label(
    folder: &Path,
    base: Option<&Path>,
    roots: &[crate::media_scan::MediaNode],
) -> String {
    let join = |rel: &Path| {
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" / ")
    };
    if let Some(rel) = base.and_then(|b| folder.strip_prefix(b).ok()) {
        return join(rel);
    }
    match roots
        .iter()
        .find_map(|r| folder.strip_prefix(&r.path).ok().map(|rel| (r, rel)))
    {
        Some((r, rel)) if rel.as_os_str().is_empty() => r.name.clone(),
        Some((r, rel)) => format!("{} / {}", r.name, join(rel)),
        None => folder.display().to_string(),
    }
}

/// One folder in the media tree: click to show it; the arrow opens its sub-folders.
fn folder_node(
    ui: &mut egui::Ui,
    selected: &mut Option<PathBuf>,
    node: &crate::media_scan::MediaNode,
    top: bool,
) {
    let label = format!("{} ({})", node.name, node.total);
    let is_sel = selected.as_deref() == Some(node.path.as_path());
    if node.children.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(18.0); // line up with folders that have an arrow
            if ui
                .selectable_label(is_sel, label)
                .on_hover_text(node.path.display().to_string())
                .clicked()
            {
                *selected = Some(node.path.clone());
            }
        });
        return;
    }
    let id = ui.make_persistent_id(("media-folder", &node.path));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, top)
        .show_header(ui, |ui| {
            if ui
                .selectable_label(is_sel, label)
                .on_hover_text(node.path.display().to_string())
                .clicked()
            {
                *selected = Some(node.path.clone());
            }
        })
        .body(|ui| {
            for c in &node.children {
                folder_node(ui, selected, c, false);
            }
        });
}

fn media_tile(
    ui: &mut egui::Ui,
    thumbs: Option<&mut crate::media_thumbs::MediaThumbs>,
    path: &Path,
    img: Vec2,
    live: bool,
) -> egui::Response {
    let bar_h = 16.0;
    let (rect, resp) = ui.allocate_exact_size(img + Vec2::new(0.0, bar_h), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let img_rect = Rect::from_min_size(rect.min, img);
    let p = ui.painter();
    p.rect_filled(img_rect, 0.0, Color32::from_rgb(20, 20, 24));
    let image = is_image(path);
    let tex = thumbs
        .and_then(|t| t.get(path))
        .and_then(|r| r.as_ref().ok());
    if let Some(t) = tex {
        // Fit inside the tile, centred.
        let size = t.size_vec2();
        let s = (img.x / size.x).min(img.y / size.y);
        let r = Rect::from_center_size(img_rect.center(), size * s);
        egui::Image::new(t).paint_at(ui, r);
        if !image {
            // Small film badge so videos stand out from stills.
            let badge = Rect::from_min_size(img_rect.min + Vec2::splat(3.0), Vec2::splat(18.0));
            p.rect_filled(badge, 3.0, Color32::from_black_alpha(170));
            p.text(
                badge.center(),
                Align2::CENTER_CENTER,
                icon::FILM_STRIP,
                theme::icon_font(12.0),
                Color32::from_gray(230),
            );
        }
    } else {
        let (glyph, tint) = if image {
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
    }
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
                SettingsTab::Folders => settings_folders(ui, st, settings, actions),
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
    ui.weak(format!("PP6 {}", crate::VERSION));
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
    let res_label = |r: Option<pp6core::Resolution>| match r {
        Some(r) => format!("{} × {}", r.width, r.height),
        None => "Display's current setting".into(),
    };
    ui.horizontal(|ui| {
        ui.label("Resolution");
        egui::ComboBox::from_id_salt("settings-resolution")
            .selected_text(res_label(settings.output.resolution))
            .width(300.0)
            .show_ui(ui, |ui| {
                let mut choices: Vec<Option<pp6core::Resolution>> = vec![None];
                choices.extend(
                    st.output_resolutions
                        .iter()
                        .map(|&(width, height)| Some(pp6core::Resolution { width, height })),
                );
                // Keep a saved choice listed even if this display doesn't offer it.
                if settings.output.resolution.is_some()
                    && !choices.contains(&settings.output.resolution)
                {
                    choices.push(settings.output.resolution);
                }
                for c in choices {
                    if ui
                        .selectable_label(settings.output.resolution == c, res_label(c))
                        .clicked()
                        && settings.output.resolution != c
                    {
                        actions.push(UiAction::SetResolution(c));
                    }
                }
            });
    });
    if st.output_resolutions.is_empty() {
        ui.label(
            RichText::new("Turn the output on to list its display's resolutions.")
                .small()
                .weak(),
        );
    }
    ui.horizontal(|ui| {
        use pp6core::MediaScaling as M;
        ui.label("Backgrounds");
        for (m, label, tip) in [
            (
                M::Fill,
                "Fill",
                "Cover the whole screen; edges that don't fit are cropped.",
            ),
            (
                M::Fit,
                "Fit",
                "Show the whole picture; black bars where the shapes differ.",
            ),
            (M::Stretch, "Stretch", "Stretch to the screen's shape."),
        ] {
            if ui
                .selectable_label(settings.output.media_scaling == m, label)
                .on_hover_text(tip)
                .clicked()
            {
                actions.push(UiAction::SetMediaScaling(m));
            }
        }
    });
    ui.separator();
    ui.heading("Editor");
    ui.horizontal(|ui| {
        ui.label("Fit Text minimum size");
        let mut v = settings.editor.fit_min_size;
        if ui
            .add(
                egui::DragValue::new(&mut v)
                    .range(4.0..=400.0)
                    .speed(0.5)
                    .suffix(" px"),
            )
            .on_hover_text("Fit Text never makes the main text smaller than this (canvas pixels)")
            .changed()
        {
            actions.push(UiAction::SetFitMinSize(v));
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

fn settings_folders(
    ui: &mut egui::Ui,
    st: &UiState,
    settings: &Settings,
    actions: &mut Vec<UiAction>,
) {
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
    ui.separator();
    ui.heading("Playlists");
    ui.label(
        RichText::new(format!(
            "Saved automatically, one file each, in {} (deleted ones go to its 'deleted' folder).",
            st.playlists_path
        ))
        .small()
        .weak(),
    );
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

/// Where library rows were drawn (tests click on them).
#[cfg(test)]
mod st_probe {
    use std::cell::RefCell;
    thread_local! {
        pub static ROWS: RefCell<Vec<(String, egui::Rect, egui::Rect)>> = const { RefCell::new(Vec::new()) };
    }
    pub fn record(title: &str, row: egui::Rect, add: egui::Rect) {
        ROWS.with(|r| r.borrow_mut().push((title.to_owned(), row, add)));
    }
    pub fn take() -> Vec<(String, egui::Rect, egui::Rect)> {
        ROWS.with(|r| std::mem::take(&mut *r.borrow_mut()))
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    use egui::{Event, PointerButton, Pos2, RawInput};
    use pp6core::library::IndexEntry;

    struct Harness {
        ctx: egui::Context,
        st: UiState,
        show: ShowState,
        settings: Settings,
        thumbs: Sender<ThumbRequest>,
        time: f64,
    }

    impl Harness {
        fn new() -> Self {
            let mut st = UiState::default();
            for t in ["AA001 Welcome", "BB002 Blest Be", "HG135 O Worship"] {
                st.library.entries.push(IndexEntry {
                    path: PathBuf::from(format!("/lib/{t}.pro6")),
                    title: t.into(),
                    ..Default::default()
                });
            }
            let settings = Settings {
                library_dirs: vec![PathBuf::from("/lib")],
                ..Default::default()
            };
            let (thumbs, _rx) = crossbeam_channel::unbounded();
            Harness {
                ctx: egui::Context::default(),
                st,
                show: ShowState::default(),
                settings,
                thumbs,
                time: 0.0,
            }
        }

        /// One UI frame with `events`; returns the actions and the library row rects.
        fn frame(&mut self, events: Vec<Event>) -> (Vec<UiAction>, Vec<(String, Rect, Rect)>) {
            self.time += 0.05;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let _ = st_probe::take();
            let mut actions = Vec::new();
            let (st, show, settings, thumbs) =
                (&mut self.st, &self.show, &self.settings, &self.thumbs);
            let mut out = self.ctx.run_ui(input, |ui| {
                actions = draw(ui, st, show, settings, thumbs, &[]);
            });
            out.textures_delta.clear(); // no GPU here
            (actions, st_probe::take())
        }

        fn click(&mut self, at: Pos2) -> Vec<UiAction> {
            let mut out = Vec::new();
            out.extend(self.frame(vec![Event::PointerMoved(at)]).0);
            out.extend(self.frame(vec![press(at, true)]).0);
            out.extend(self.frame(vec![press(at, false)]).0);
            out
        }
    }

    fn press(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// "AM" with A, B and "PM" with C.
    fn two_playlists() -> ShowState {
        let pl = |name: &str, items: &[&str]| pp6core::PlaylistFile {
            file: None,
            playlist: pp6core::Playlist {
                name: name.into(),
                items: items
                    .iter()
                    .map(|t| pp6core::PlaylistItem {
                        path: PathBuf::from(format!("/lib/{t}.pro6")),
                        arrangement: None,
                    })
                    .collect(),
            },
        };
        ShowState {
            playlists: Arc::new(vec![pl("AM", &["A", "B"]), pl("PM", &["C"])]),
            ..Default::default()
        }
    }

    fn added(actions: &[UiAction]) -> Vec<PathBuf> {
        actions
            .iter()
            .filter_map(|a| match a {
                UiAction::Core(Command::PlaylistAdd { path, .. }) => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    fn rows(h: &mut Harness) -> Vec<(String, Rect, Rect)> {
        h.frame(vec![]);
        let r = h.frame(vec![]).1;
        assert_eq!(r.len(), 3, "library rows drawn: {r:?}");
        r
    }

    #[test]
    fn plus_button_adds_to_playlist() {
        let mut h = Harness::new();
        let r = rows(&mut h);
        let a = h.click(r[1].2.center());
        assert_eq!(added(&a), [PathBuf::from("/lib/BB002 Blest Be.pro6")]);
    }

    #[test]
    fn dragging_a_library_row_onto_the_playlist_adds_it() {
        let mut h = Harness::new();
        let r = rows(&mut h);
        let from = r[2].1.center();
        let to = Pos2::new(150.0, 860.0); // playlist panel, bottom-left
        let mut a = Vec::new();
        a.extend(h.frame(vec![Event::PointerMoved(from)]).0);
        a.extend(h.frame(vec![press(from, true)]).0);
        for k in 1..=10 {
            let p = from + (to - from) * (k as f32 / 10.0);
            a.extend(h.frame(vec![Event::PointerMoved(p)]).0);
        }
        a.extend(h.frame(vec![press(to, false)]).0);
        a.extend(h.frame(vec![]).0);
        assert_eq!(added(&a), [PathBuf::from("/lib/HG135 O Worship.pro6")]);
    }

    #[test]
    fn drop_positions() {
        let mut a = Vec::new();
        let show = two_playlists();
        let r = |playlist, item| ItemRef { playlist, item };
        drop_into_playlist(&PlaylistDrag::Library("/x".into()), 0, 1, 2, &show, &mut a);
        assert!(matches!(
            a.as_slice(),
            [
                UiAction::Core(Command::PlaylistAdd { playlist: 0, .. }),
                UiAction::Core(Command::PlaylistMove {
                    playlist: 0,
                    from: 2,
                    to: 1
                })
            ]
        ));
        let mut a = Vec::new();
        drop_into_playlist(&PlaylistDrag::Item(r(0, 0)), 0, 2, 2, &show, &mut a); // to the end
        assert!(matches!(
            a.as_slice(),
            [UiAction::Core(Command::PlaylistMove {
                playlist: 0,
                from: 0,
                to: 1
            })]
        ));
        let mut a = Vec::new();
        drop_into_playlist(&PlaylistDrag::Item(r(0, 1)), 0, 1, 2, &show, &mut a); // onto itself
        assert!(a.is_empty());
        // Into the other playlist: added there (top), removed here.
        let mut a = Vec::new();
        drop_into_playlist(&PlaylistDrag::Item(r(0, 1)), 1, 0, 1, &show, &mut a);
        assert!(matches!(
            a.as_slice(),
            [
                UiAction::Core(Command::PlaylistAdd { playlist: 1, .. }),
                UiAction::Core(Command::PlaylistMove {
                    playlist: 1,
                    from: 1,
                    to: 0
                }),
                UiAction::Core(Command::PlaylistRemove(ItemRef {
                    playlist: 0,
                    item: 1
                }))
            ]
        ));
    }

    #[test]
    fn clicking_a_playlist_row_selects_it() {
        let mut h = Harness::new();
        h.show = two_playlists();
        h.frame(vec![]);
        h.frame(vec![]);
        // Playlist panel rows start under its header (bottom half of the left column).
        let mut picked = None;
        for y in (450..880).step_by(4) {
            let a = h.click(Pos2::new(60.0, y as f32));
            if let Some(i) = a.iter().find_map(|a| match a {
                UiAction::Core(Command::PlaylistSelect(r)) => Some(r.item + 10 * r.playlist),
                _ => None,
            }) {
                picked = Some(i);
                break;
            }
        }
        assert_eq!(picked, Some(0));
    }

    #[test]
    fn double_click_adds_to_playlist() {
        let mut h = Harness::new();
        let r = rows(&mut h);
        let at = r[0].1.center();
        let mut a = h.click(at);
        a.extend(h.click(at));
        assert_eq!(added(&a), [PathBuf::from("/lib/AA001 Welcome.pro6")]);
    }
}
