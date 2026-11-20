//! `pp6` — the operator app: egui operator window + output window, both on one
//! winit event loop; the output's GL runs on its own thread, and the show state
//! lives in the core actor.

mod crash;
mod editor;
mod error;
mod gl;
mod keys;
mod media_scan;
mod media_thumbs;
mod operator;
mod output;
mod raster;
mod theme;
mod thumbs;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Parser;
use crossbeam_channel::{Receiver, Sender};
use pp6core::library::LibraryIndex;
use pp6core::{Command, CoreHandle, Settings, ShowState};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::WindowId;

use crate::crash::CrashContext;
use crate::operator::OperatorWindow;
use crate::output::{MonitorChoice, OutputWindow};
use crate::raster::{OutputLink, RasterCmd, RenderCmd, ms};
use crate::thumbs::{ThumbRequest, ThumbResult};
use crate::ui::{UiAction, UiState};

#[derive(Parser)]
#[command(name = "pp6", version = VERSION, about = "PP6 replacement")]
struct Args {
    /// Put this presentation's first slide live at startup.
    #[arg(long)]
    open: Option<PathBuf>,
    /// Use this folder for settings, logs and caches (default: the per-user folders).
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Add a library folder (saved to settings).
    #[arg(long)]
    library: Vec<PathBuf>,
    /// Extra font directory (repeatable; not saved).
    #[arg(long = "fonts")]
    font_dirs: Vec<PathBuf>,
    /// Output display index for this run (see the toolbar list).
    #[arg(long)]
    monitor: Option<usize>,
    /// Output in a normal window instead of fullscreen (development).
    #[arg(long)]
    windowed_output: bool,
    /// Size of the --windowed-output window, WIDTHxHEIGHT.
    #[arg(long, value_parser = parse_size, default_value = "960x720")]
    output_size: (u32, u32),
    /// Ignore the crash-restore file.
    #[arg(long)]
    no_restore: bool,
    /// Send Next N times after the first slide is live, print timing statistics, and exit.
    #[arg(long)]
    bench: Option<usize>,
    #[arg(long, default_value_t = 250)]
    bench_interval_ms: u64,
    /// After the first slide is on screen, save the output's pixels to this PNG and exit.
    #[arg(long)]
    capture: Option<PathBuf>,
    /// Start with this background media (image or video) under the text.
    #[arg(long)]
    background: Option<PathBuf>,
    /// Pre-load this media as the next background (development / testing).
    #[arg(long)]
    background_next: Option<PathBuf>,
    /// Switch to --background-next this long after startup (development / testing).
    #[arg(long)]
    switch_background_ms: Option<u64>,
    /// With --capture: wait this long after the first slide before capturing.
    #[arg(long, default_value_t = 0)]
    capture_delay_ms: u64,
    /// Start the loop (auto-advance) after --open (development / testing).
    #[arg(long)]
    start_loop: bool,
    /// Start the grid in edit mode, optionally with slide GROUP:SLIDE selected (0-based).
    #[arg(long)]
    edit_mode: bool,
    #[arg(long)]
    edit_slide: Option<String>,
    /// Open the Settings window at startup.
    #[arg(long)]
    open_settings: bool,
    /// Save the operator window as PNG 3 s after it opens, then exit.
    #[arg(long)]
    capture_ui: Option<PathBuf>,
    /// Debug logging on the console.
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Debug)]
pub enum UserEvent {
    /// A core snapshot is waiting.
    Snapshot,
    Repaint(Duration),
    ThumbReady,
    FontsReady {
        elapsed: Duration,
        faces: usize,
    },
    Presented {
        index: Option<usize>,
        latency: Option<Duration>,
        upload: Duration,
        at: Instant,
    },
    RenderFailed {
        index: usize,
        message: String,
    },
    OutputFailed(String),
    Captured {
        path: PathBuf,
        result: Result<(), String>,
    },
    Library(Box<LibraryIndex>, String),
    MediaFailed(String),
    Media(Box<media_scan::MediaScan>),
    AudioDevices(Result<Vec<media::AudioDevice>, String>),
    /// The output thread has loaded libmpv and set up its players (start-up, idle).
    MediaWarm,
}

fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s.split_once(['x', 'X']).ok_or("expected WIDTHxHEIGHT")?;
    match (w.trim().parse::<u32>(), h.trim().parse::<u32>()) {
        (Ok(w), Ok(h)) if w > 0 && h > 0 => Ok((w, h)),
        _ => Err(format!("bad size {s:?}")),
    }
}

/// Version and the git commit it was built from, e.g. "0.1.0 (851e7e5)".
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("PP6_BUILD"), ")");

/// Give a window the PP6 icon (title bar, taskbar, Alt-Tab). A missing icon is cosmetic:
/// logged, never fatal.
pub fn with_icon(attrs: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    #[cfg(windows)]
    {
        use winit::platform::windows::{IconExtWindows, WindowAttributesExtWindows};
        use winit::window::Icon;
        // Resource 1 is the multi-size icon embedded by build.rs.
        let small = Icon::from_resource(1, None);
        let big = Icon::from_resource(1, Some(winit::dpi::PhysicalSize::new(64, 64)));
        match (small, big) {
            (Ok(small), Ok(big)) => attrs
                .with_window_icon(Some(small))
                .with_taskbar_icon(Some(big)),
            (Err(e), _) | (_, Err(e)) => {
                tracing::warn!(error = %e, "program icon couldn't be loaded");
                attrs
            }
        }
    }
    #[cfg(not(windows))]
    {
        let icon = image::load_from_memory(include_bytes!("../assets/pp6-64.png"))
            .map_err(|e| e.to_string())
            .and_then(|img| {
                let img = img.into_rgba8();
                let (w, h) = img.dimensions();
                winit::window::Icon::from_rgba(img.into_raw(), w, h).map_err(|e| e.to_string())
            });
        match icon {
            Ok(icon) => attrs.with_window_icon(Some(icon)),
            Err(e) => {
                tracing::warn!(error = %e, "program icon couldn't be loaded");
                attrs
            }
        }
    }
}

struct Paths {
    settings: PathBuf,
    logs: PathBuf,
    library_cache: PathBuf,
    thumbs: PathBuf,
    playlists: PathBuf,
}

fn paths(data_dir: Option<PathBuf>) -> Paths {
    let (config, data, cache) = match data_dir {
        Some(d) => (d.clone(), d.clone(), d.join("cache")),
        None => match directories::ProjectDirs::from("", "", "PP6") {
            Some(p) => (
                p.config_dir().to_path_buf(),
                p.data_local_dir().to_path_buf(),
                p.cache_dir().to_path_buf(),
            ),
            None => {
                let d = PathBuf::from("pp6-data");
                (d.clone(), d.clone(), d.join("cache"))
            }
        },
    };
    Paths {
        settings: config.join("settings.toml"),
        logs: data.join("logs"),
        library_cache: cache.join("library.json"),
        playlists: data.join("playlists"),
        thumbs: cache.join("thumbnails"),
    }
}

fn main() -> ExitCode {
    let started = Instant::now();
    let args = Args::parse();
    let paths = paths(args.data_dir.clone());
    let crash = crash::init_logging(paths.logs.clone(), args.verbose);
    crash::install_panic_hook(crash.clone());
    tracing::info!(version = VERSION, settings = %paths.settings.display(), logs = %paths.logs.display(), "starting");

    let mut app_notices = Vec::new();
    let mut settings = Settings::load(&paths.settings).unwrap_or_else(|e| {
        tracing::error!(error = %e, "settings");
        app_notices.push(e.user_message());
        Settings::default()
    });
    let mut changed = false;
    for d in &args.library {
        if !settings.library_dirs.contains(d) {
            settings.library_dirs.push(d.clone());
            changed = true;
        }
    }
    if changed && let Err(e) = settings.save(&paths.settings) {
        app_notices.push(e.user_message());
    }

    // Start blank (John, 2026-11-20): nothing live, nothing on the output.
    let _ = args.no_restore;
    let (core, _core_thread) = match pp6core::spawn(pp6core::CoreConfig {
        playlists_dir: Some(paths.playlists.clone()),
        output_enabled: settings.output.enabled,
    }) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Couldn't start: {e}");
            return ExitCode::FAILURE;
        }
    };

    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("Couldn't connect to the display system: {e}");
            return ExitCode::FAILURE;
        }
    };
    let proxy = event_loop.create_proxy();
    let wake = proxy.clone();
    let snaps = core.subscribe(Some(Box::new(move || {
        let _ = wake.send_event(UserEvent::Snapshot);
    })));

    // Library: cached index now, background revalidation after.
    let cached = LibraryIndex::load_cache(&paths.library_cache).unwrap_or_default();
    let lib_proxy = proxy.clone();
    let _ = pp6core::library::spawn_revalidate(
        cached.clone(),
        settings.library_dirs.clone(),
        Some(paths.library_cache.clone()),
        move |idx, s| {
            let status = format!(
                "{} presentations ({} re-read, {} unreadable)",
                idx.entries.len(),
                s.parsed,
                s.failed
            );
            let _ = lib_proxy.send_event(UserEvent::Library(Box::new(idx), status));
        },
    );

    let link: OutputLink = Arc::new(Mutex::new(None));
    let (fonts_tx, fonts_rx) = crossbeam_channel::bounded(1);
    let mut font_dirs = settings.font_dirs.clone();
    font_dirs.extend(args.font_dirs.iter().cloned());
    let raster = match raster::spawn(
        core.subscribe(None),
        font_dirs,
        link.clone(),
        proxy.clone(),
        fonts_tx,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Couldn't start the slide renderer: {e}");
            return ExitCode::FAILURE;
        }
    };
    let (thumbs_tx, thumbs_rx) =
        match thumbs::spawn(fonts_rx, Some(paths.thumbs.clone()), proxy.clone()) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Couldn't start the thumbnail renderer: {e}");
                return ExitCode::FAILURE;
            }
        };

    let mpv_ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let media_thumbs =
        match media_thumbs::spawn(paths.thumbs.join("media"), mpv_ready.clone(), proxy.clone()) {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::error!(error = %e, "couldn't start the media thumbnail worker");
                None
            }
        };

    if let Some(b) = &args.background {
        core.send(Command::SetBackground {
            media: Some(b.clone()),
            next: args.background_next.clone(),
        });
    }
    if let Some(p) = &args.open {
        // Absolute, like library and playlist paths, so they compare equal.
        let p = std::path::absolute(p).unwrap_or_else(|_| p.clone());
        core.send(Command::Open(p.clone()));
        core.send(Command::GoTo {
            presentation: p.clone(),
            position: 0,
        });
    }
    if args.start_loop {
        core.send(Command::StartLoop);
    }

    let mut remote = None;
    if settings.web.enabled {
        match start_remote(&core, &settings) {
            Ok(r) => remote = Some(r),
            Err(msg) => app_notices.push(msg),
        }
    }
    scan_media(settings.media_dirs.clone(), proxy.clone());
    let mut ui_state_edit = editor::EditState::default();
    if args.edit_mode {
        ui_state_edit.on = true;
    }
    if let Some((g, s)) = args.edit_slide.as_deref().and_then(|v| v.split_once(':'))
        && let (Ok(g), Ok(s)) = (g.parse(), s.parse())
    {
        ui_state_edit.select(g, s);
    }
    let mut app = App {
        started,
        ui: UiState {
            library: cached,
            library_status: "Checking library…".into(),
            media_scanning: true,
            app_notices,
            settings_path: paths.settings.display().to_string(),
            playlists_path: paths.playlists.display().to_string(),
            settings_open: args.open_settings,
            edit: ui_state_edit,
            media_thumbs,
            ..UiState::default()
        },
        show: Arc::new(ShowState::default()),
        snaps,
        core,
        crash,
        paths,
        settings,
        proxy,
        link,
        raster_tx: raster.tx,
        raster_thread: Some(raster.thread),
        thumbs_tx,
        thumbs_rx,
        mpv_ready,
        operator: None,
        output: None,
        remote,
        sent_size: None,
        monitor_override: args.monitor,
        windowed_output: args.windowed_output,
        output_size: args.output_size,
        capture: args.capture,
        capture_delay: Duration::from_millis(args.capture_delay_ms),
        capture_due: None,
        switch_background: args
            .switch_background_ms
            .zip(args.background_next.clone())
            .map(|(ms, p)| (started + Duration::from_millis(ms), p)),
        capture_ui: args.capture_ui.map(|p| (p, None)),
        bench: args.bench.map(|n| Bench {
            remaining: n,
            interval: Duration::from_millis(args.bench_interval_ms),
            next_at: None,
        }),
        latencies: Vec::new(),
        // Cold start is measured only when a slide is put up at launch (--open); otherwise
        // the first click is an ordinary, measured slide change.
        cold_start_logged: args.open.is_none(),
        audio_listed: false,
        recreate_attempts: 0,
        started_once: false,
        quitting: false,
        exit_code: ExitCode::SUCCESS,
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("The display event loop stopped unexpectedly: {e}");
        return ExitCode::FAILURE;
    }
    app.exit_code
}

/// Scan the media folders on a background thread (they can be large or on a slow
/// drive); the result arrives as `UserEvent::Media`.
fn scan_media(dirs: Vec<PathBuf>, proxy: EventLoopProxy<UserEvent>) {
    let spawned = std::thread::Builder::new()
        .name("media-scan".into())
        .spawn(move || {
            let scan = media_scan::scan(&dirs);
            let _ = proxy.send_event(UserEvent::Media(Box::new(scan)));
        });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "couldn't start the media folder scan");
    }
}

fn start_remote(core: &CoreHandle, settings: &Settings) -> Result<web::WebRemote, String> {
    let bind = match settings
        .web
        .bind
        .as_deref()
        .map(str::parse::<std::net::IpAddr>)
    {
        Some(Ok(ip)) => Some(ip),
        Some(Err(_)) => {
            return Err(format!(
                "The phone remote address {:?} in settings isn't valid.",
                settings.web.bind
            ));
        }
        None => None,
    };
    let config = web::WebConfig {
        bind,
        port: settings.web.port,
        pin_length: settings.web.pin_length,
    };
    web::WebRemote::start(core.clone(), core.subscribe(None), &config).map_err(|e| {
        tracing::error!(error = %e, "phone remote");
        e.user_message()
    })
}

struct Bench {
    remaining: usize,
    interval: Duration,
    next_at: Option<Instant>,
}

struct App {
    started: Instant,
    ui: UiState,
    show: Arc<ShowState>,
    snaps: Receiver<Arc<ShowState>>,
    core: CoreHandle,
    crash: CrashContext,
    paths: Paths,
    settings: Settings,
    proxy: EventLoopProxy<UserEvent>,
    link: OutputLink,
    raster_tx: Sender<RasterCmd>,
    raster_thread: Option<std::thread::JoinHandle<()>>,
    thumbs_tx: Sender<ThumbRequest>,
    thumbs_rx: Receiver<ThumbResult>,
    /// Set once the output has loaded libmpv (video thumbnails wait for it).
    mpv_ready: Arc<std::sync::atomic::AtomicBool>,
    operator: Option<OperatorWindow>,
    output: Option<OutputWindow>,
    remote: Option<web::WebRemote>,
    sent_size: Option<(u32, u32)>,
    monitor_override: Option<usize>,
    windowed_output: bool,
    output_size: (u32, u32),
    capture: Option<PathBuf>,
    capture_delay: Duration,
    capture_due: Option<Instant>,
    /// (when, media) for --switch-background-ms.
    switch_background: Option<(Instant, PathBuf)>,
    /// Operator-window capture path and when to take it.
    capture_ui: Option<(PathBuf, Option<Instant>)>,
    bench: Option<Bench>,
    latencies: Vec<Duration>,
    cold_start_logged: bool,
    audio_listed: bool,
    recreate_attempts: u32,
    started_once: bool,
    quitting: bool,
    exit_code: ExitCode,
}

fn media_scaling(s: pp6core::MediaScaling) -> media::Scaling {
    match s {
        pp6core::MediaScaling::Fill => media::Scaling::Fill,
        pp6core::MediaScaling::Fit => media::Scaling::Fit,
        pp6core::MediaScaling::Stretch => media::Scaling::Stretch,
    }
}

impl App {
    /// Every command source goes through here (recorded for crash reports).
    fn send(&self, cmd: Command) {
        self.crash.recent_commands.push(format!(
            "{:>10.3}s {cmd:?}",
            self.started.elapsed().as_secs_f64()
        ));
        self.core.send(cmd);
    }

    fn monitor_choice(&self) -> MonitorChoice {
        if let Some(i) = self.monitor_override {
            return MonitorChoice::Index(i);
        }
        match &self.settings.output.monitor_name {
            Some(name) => MonitorChoice::Name {
                name: name.clone(),
                index: self.settings.output.monitor_index,
            },
            None => MonitorChoice::Secondary {
                avoid: self
                    .operator
                    .as_ref()
                    .and_then(|o| o.window.current_monitor()),
            },
        }
    }

    fn enable_output(&mut self, el: &ActiveEventLoop) {
        if self.output.is_some() {
            return;
        }
        let windowed = self.windowed_output.then_some(self.output_size);
        match OutputWindow::create(
            el,
            self.monitor_choice(),
            windowed,
            self.settings.audio_device.clone(),
            media_scaling(self.settings.output.media_scaling),
            self.settings.output.resolution.map(|r| (r.width, r.height)),
            self.proxy.clone(),
        ) {
            Ok(o) => {
                if let Ok(mut l) = self.link.lock() {
                    *l = Some(o.tx.clone());
                }
                let (width, height) = o.size();
                let _ = self.raster_tx.send(RasterCmd::Size { width, height });
                self.sent_size = Some((width, height));
                self.ui.output_resolutions = o.resolutions.clone();
                self.output = Some(o);
            }
            Err(e) => {
                tracing::error!(error = %e, "output window failed");
                self.ui.app_notices.push(e.user_message());
                self.request_redraw();
            }
        }
    }

    fn disable_output(&mut self) {
        if let Ok(mut l) = self.link.lock() {
            *l = None;
        }
        if let Some(o) = self.output.take() {
            o.destroy();
        }
    }

    /// List mpv's audio devices once, at launch, after the windows are up and the output
    /// has loaded libmpv (never on a slide click: starting mpv's audio loads Windows audio
    /// DLLs, which can stall other threads for a moment).
    fn list_audio_devices(&mut self) {
        if self.audio_listed {
            return;
        }
        self.audio_listed = true;
        let audio_proxy = self.proxy.clone();
        let started = Instant::now();
        let spawned = std::thread::Builder::new()
            .name("audio-devices".into())
            .spawn(move || {
                let r = media::audio_devices().map_err(|e| e.user_message());
                tracing::info!(elapsed_ms = ms(started.elapsed()), "audio devices listed");
                let _ = audio_proxy.send_event(UserEvent::AudioDevices(r));
            });
        if let Err(e) = spawned {
            tracing::error!(error = %e, "couldn't start listing audio devices");
        }
    }

    fn request_redraw(&self) {
        if let Some(o) = &self.operator {
            o.window.request_redraw();
        }
    }

    fn quit(&mut self, el: &ActiveEventLoop) {
        if self.quitting {
            return;
        }
        self.quitting = true;
        self.disable_output();
        let _ = self.raster_tx.send(RasterCmd::Stop);
        if let Some(t) = self.raster_thread.take()
            && let Err(panic) = t.join()
        {
            std::panic::resume_unwind(panic);
        }
        self.remote = None; // stop the phone remote
        self.core.shutdown();
        if let Some(mut o) = self.operator.take() {
            o.destroy();
        }
        el.exit();
    }

    fn on_snapshot(&mut self, el: &ActiveEventLoop) {
        let mut latest = None;
        while let Ok(s) = self.snaps.try_recv() {
            latest = Some(s);
        }
        let Some(s) = latest else { return };
        if s.output_enabled != self.show.output_enabled || self.show.seq == 0 {
            if s.output_enabled && self.started_once {
                self.enable_output(el);
            } else if !s.output_enabled {
                self.disable_output();
            }
        }
        if let Ok(mut summary) = self.crash.show_state.lock() {
            *summary = s.summary();
        }
        self.show = s;
        self.request_redraw();
    }

    fn handle_key(&mut self, event: &winit::event::KeyEvent, from_operator: bool) -> bool {
        if event.state != ElementState::Pressed || event.repeat {
            return false;
        }
        // Typing in a text field (e.g. library search) must not trigger slides.
        if from_operator
            && self
                .operator
                .as_ref()
                .is_some_and(|o| o.egui.egui_ctx.egui_wants_keyboard_input())
        {
            return false;
        }
        let Some(cmd) =
            keys::key_name(&event.logical_key).and_then(|k| self.settings.keys.command_for(&k))
        else {
            return false;
        };
        self.send(cmd);
        true
    }

    /// Ask what to do with unsaved edits before closing. `false` = don't close.
    fn confirm_unsaved(&mut self) -> bool {
        let unsaved: Vec<PathBuf> = self.show.unsaved.iter().cloned().collect();
        if unsaved.is_empty() {
            return true;
        }
        let names: Vec<String> = unsaved.iter().map(|p| ui::title(p)).collect();
        let answer = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Unsaved changes")
            .set_description(format!(
                "Save changes to {} before closing?",
                names.join(", ")
            ))
            .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
                "Save".into(),
                "Don't save".into(),
                "Cancel".into(),
            ))
            .show();
        match answer {
            rfd::MessageDialogResult::Custom(s) if s == "Save" => {
                let mut ok = true;
                for p in &unsaved {
                    if let Some(doc) = self.show.docs.get(p)
                        && let Err(e) = pp6core::save_with_backup(doc)
                    {
                        ok = false;
                        rfd::MessageDialog::new()
                            .set_level(rfd::MessageLevel::Error)
                            .set_title("Not saved")
                            .set_description(e)
                            .show();
                    }
                }
                ok
            }
            rfd::MessageDialogResult::Custom(s) if s == "Don't save" => true,
            _ => false,
        }
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save(&self.paths.settings) {
            self.ui.app_notices.push(e.user_message());
        }
    }

    fn act(&mut self, el: &ActiveEventLoop, action: UiAction) {
        match action {
            UiAction::Core(c) => self.send(c),
            UiAction::AddLibraryFolder => {
                if let Some(d) = rfd::FileDialog::new()
                    .set_title("Choose a library folder")
                    .pick_folder()
                {
                    if !self.settings.library_dirs.contains(&d) {
                        self.settings.library_dirs.push(d);
                        self.save_settings();
                    }
                    self.ui.library_status = "Checking library…".into();
                    let proxy = self.proxy.clone();
                    let _ = pp6core::library::spawn_revalidate(
                        self.ui.library.clone(),
                        self.settings.library_dirs.clone(),
                        Some(self.paths.library_cache.clone()),
                        move |idx, s| {
                            let status = format!(
                                "{} presentations ({} re-read, {} unreadable)",
                                idx.entries.len(),
                                s.parsed,
                                s.failed
                            );
                            let _ = proxy.send_event(UserEvent::Library(Box::new(idx), status));
                        },
                    );
                }
            }
            UiAction::AddMediaFolder => {
                if let Some(d) = rfd::FileDialog::new()
                    .set_title("Choose a media folder")
                    .pick_folder()
                {
                    if !self.settings.media_dirs.contains(&d) {
                        self.settings.media_dirs.push(d);
                        self.save_settings();
                    }
                    self.ui.media_scanning = true;
                    scan_media(self.settings.media_dirs.clone(), self.proxy.clone());
                }
            }
            UiAction::ImportPlaylist => {
                if let Some(p) = rfd::FileDialog::new()
                    .set_title("Import a playlist")
                    .add_filter("Playlist", &["toml"])
                    .pick_file()
                {
                    self.send(Command::PlaylistImport(p));
                }
            }
            UiAction::DeletePlaylist(i) => {
                let name = self
                    .show
                    .playlists
                    .get(i)
                    .map(|p| p.playlist.name.clone())
                    .unwrap_or_default();
                let answer = rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Warning)
                    .set_title("Delete playlist")
                    .set_description(format!(
                        "Delete the playlist '{name}'? The presentations themselves aren't touched. (The playlist file is kept in the 'deleted' folder.)"
                    ))
                    .set_buttons(rfd::MessageButtons::OkCancelCustom(
                        "Delete".into(),
                        "Cancel".into(),
                    ))
                    .show();
                if matches!(answer, rfd::MessageDialogResult::Custom(ref b) if b == "Delete")
                    || matches!(answer, rfd::MessageDialogResult::Ok)
                {
                    self.send(Command::PlaylistDelete(i));
                }
            }
            UiAction::NewPin => {
                if let Some(r) = &self.remote {
                    r.regenerate_pin();
                }
            }
            UiAction::RemoteEnabled(on) => {
                self.settings.web.enabled = on;
                self.save_settings();
                self.remote = None; // stops a running server
                if on {
                    match start_remote(&self.core, &self.settings) {
                        Ok(r) => self.remote = Some(r),
                        Err(msg) => self.ui.app_notices.push(msg),
                    }
                }
            }
            UiAction::SetAudioDevice(d) => {
                self.settings.audio_device = d.clone();
                self.save_settings();
                if let Some(o) = &self.output {
                    let _ = o.tx.send(RenderCmd::AudioDevice(d));
                }
            }
            UiAction::RemoveLibraryFolder(i) => {
                if i < self.settings.library_dirs.len() {
                    self.settings.library_dirs.remove(i);
                    self.save_settings();
                }
            }
            UiAction::RemoveMediaFolder(i) => {
                if i < self.settings.media_dirs.len() {
                    self.settings.media_dirs.remove(i);
                    self.save_settings();
                    self.ui.media_scanning = true;
                    scan_media(self.settings.media_dirs.clone(), self.proxy.clone());
                }
            }
            UiAction::SetMediaScaling(sc) => {
                self.settings.output.media_scaling = sc;
                self.save_settings();
                if let Some(o) = &self.output {
                    let _ = o.tx.send(RenderCmd::MediaScaling(media_scaling(sc)));
                }
            }
            UiAction::SetResolution(r) => {
                self.settings.output.resolution = r;
                self.save_settings();
                self.disable_output();
                if self.show.output_enabled {
                    self.enable_output(el);
                }
            }
            UiAction::ChooseMonitor(i) => {
                let name = el.available_monitors().nth(i).and_then(|m| m.name());
                self.settings.output.monitor_name = name;
                self.settings.output.monitor_index = Some(i);
                self.monitor_override = None;
                self.save_settings();
                self.disable_output();
                if self.show.output_enabled {
                    self.enable_output(el);
                }
            }
        }
    }

    fn print_bench_summary(&self) {
        let mut l: Vec<f64> = self.latencies.iter().map(|d| ms(*d)).collect();
        l.sort_by(f64::total_cmp);
        if l.is_empty() {
            return;
        }
        let pct = |p: f64| l[((l.len() - 1) as f64 * p).round() as usize];
        println!(
            "slide change latency (command → buffer swap), n={}: min {:.2} ms, median {:.2} ms, p95 {:.2} ms, max {:.2} ms",
            l.len(),
            l[0],
            pct(0.5),
            pct(0.95),
            l[l.len() - 1]
        );
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.started_once {
            return;
        }
        self.started_once = true;
        if let Some((_, due)) = &mut self.capture_ui {
            *due = Some(Instant::now() + Duration::from_secs(3));
        }
        match OperatorWindow::create(el, self.proxy.clone()) {
            Ok(o) => self.operator = Some(o),
            Err(e) => panic!("the operator window couldn't be created: {e}"),
        }
        self.ui.monitors = output::describe_monitors(el);
        tracing::info!(monitors = ?self.ui.monitors, "displays");
        if self.show.output_enabled {
            self.enable_output(el);
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.quitting {
            return;
        }
        let is_output = self.output.as_ref().is_some_and(|o| o.id() == id);
        let is_operator = self.operator.as_ref().is_some_and(|o| o.window.id() == id);
        if let WindowEvent::KeyboardInput { event: k, .. } = &event
            && (is_output || is_operator)
            && self.handle_key(k, is_operator)
        {
            return; // bound key: don't also hand it to egui
        }
        if is_output {
            match event {
                WindowEvent::Resized(s) => {
                    let (width, height) = (s.width.max(1), s.height.max(1));
                    if let Some(o) = &self.output {
                        let _ = o.tx.send(RenderCmd::Resize { width, height });
                    }
                    if self.sent_size != Some((width, height)) {
                        self.sent_size = Some((width, height));
                        let _ = self.raster_tx.send(RasterCmd::Size { width, height });
                    }
                }
                WindowEvent::RedrawRequested => {
                    if let Some(o) = &self.output {
                        let _ = o.tx.send(RenderCmd::Redraw);
                    }
                }
                WindowEvent::CloseRequested => self.send(Command::OutputEnable(false)),
                _ => {}
            }
            return;
        }
        if !is_operator {
            return;
        }
        let Some(op) = self.operator.as_mut() else {
            return;
        };
        let response = op.egui.on_window_event(&op.window, &event);
        match event {
            WindowEvent::CloseRequested => {
                if self.confirm_unsaved() {
                    self.quit(el);
                }
            }
            WindowEvent::Resized(s) => {
                op.resize(s.width, s.height);
                op.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let mut actions = Vec::new();
                let log = self.crash.ui_log.snapshot();
                self.ui.receive_thumbs(&op.egui.egui_ctx, &self.thumbs_rx);
                if let Some(m) = &mut self.ui.media_thumbs {
                    m.receive(&op.egui.egui_ctx);
                }
                self.ui.remote = match &self.remote {
                    Some(r) => ui::RemoteStatus {
                        url: Some(r.url().to_owned()),
                        pin: r.pin(),
                        clients: r.clients(),
                    },
                    None => ui::RemoteStatus::default(),
                };
                let (show, settings, ui_state, thumbs) =
                    (&self.show, &self.settings, &mut self.ui, &self.thumbs_tx);
                let capture = self
                    .capture_ui
                    .as_ref()
                    .filter(|(_, due)| due.is_some_and(|d| Instant::now() >= d))
                    .map(|(p, _)| p.clone());
                op.frame(
                    |ui| actions = ui::draw(ui, ui_state, show, settings, thumbs, &log),
                    capture.as_deref(),
                );
                if capture.is_some() {
                    self.capture_ui = None;
                    self.quit(el);
                    return;
                }
                for a in actions {
                    self.act(el, a);
                }
            }
            _ => {
                if response.repaint {
                    op.window.request_redraw();
                }
            }
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, event: UserEvent) {
        if self.quitting {
            return;
        }
        match event {
            UserEvent::Snapshot => self.on_snapshot(el),
            UserEvent::Repaint(delay) => {
                if delay.is_zero() {
                    self.request_redraw();
                } else {
                    el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + delay));
                }
            }
            UserEvent::ThumbReady => self.request_redraw(),
            UserEvent::Library(idx, status) => {
                self.ui.library = *idx;
                self.ui.library_status = status;
                self.request_redraw();
            }
            UserEvent::Media(scan) => {
                if !scan.unreadable.is_empty() {
                    let names: Vec<String> = scan
                        .unreadable
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect();
                    self.ui.app_notices.push(format!(
                        "Some media folders couldn't be read: {}. Their files aren't listed; check the drive is connected and you have access.",
                        names.join(", ")
                    ));
                }
                if scan.truncated {
                    self.ui.app_notices.push(
                        "The media folders hold more than 50,000 files; only the first 50,000 are listed. Choose smaller folders."
                            .into(),
                    );
                }
                self.ui.media = *scan;
                self.ui.media_scanning = false;
                self.request_redraw();
            }
            UserEvent::MediaFailed(msg) => {
                self.ui.app_notices.push(msg);
                if self.ui.app_notices.len() > 20 {
                    self.ui.app_notices.remove(0);
                }
                self.request_redraw();
            }
            UserEvent::AudioDevices(r) => {
                match r {
                    Ok(d) => self.ui.audio_devices = d,
                    Err(e) => self.ui.app_notices.push(e),
                }
                self.request_redraw();
            }
            UserEvent::FontsReady { elapsed, faces } => {
                tracing::info!(faces, elapsed_ms = ms(elapsed), "fonts ready");
                // With no output window there's no media warm-up to wait for.
                if self.output.is_none() {
                    self.list_audio_devices();
                }
            }
            UserEvent::MediaWarm => {
                self.mpv_ready
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                self.list_audio_devices();
            }
            UserEvent::RenderFailed { index, message } => {
                tracing::warn!(slide = index + 1, %message, "slide not shown");
                self.ui
                    .app_notices
                    .push(format!("Slide {}: {message}", index + 1));
                if self.ui.app_notices.len() > 20 {
                    self.ui.app_notices.remove(0);
                }
                self.request_redraw();
            }
            UserEvent::Presented {
                index,
                latency,
                upload,
                at,
            } => {
                if !self.cold_start_logged && index.is_some() {
                    self.cold_start_logged = true;
                    let cold = at.duration_since(self.started);
                    tracing::info!(cold_start_ms = ms(cold), "first slide on screen");
                    println!(
                        "cold start (process start → first slide presented): {:.1} ms",
                        ms(cold)
                    );
                    if self.capture.is_some() {
                        self.capture_due = Some(Instant::now() + self.capture_delay);
                    }
                    if let Some(b) = &mut self.bench {
                        b.next_at = Some(Instant::now() + b.interval);
                    }
                    return;
                }
                // Only after the first slide is up: earlier events include startup time.
                if let Some(l) = latency.filter(|_| self.cold_start_logged) {
                    tracing::info!(
                        slide = index.map(|i| i + 1),
                        latency_ms = ms(l),
                        upload_ms = ms(upload),
                        "slide change presented"
                    );
                    if let Some(b) = &mut self.bench {
                        self.latencies.push(l);
                        b.remaining = b.remaining.saturating_sub(1);
                        if b.remaining == 0 {
                            self.print_bench_summary();
                            self.quit(el);
                            return;
                        }
                        b.next_at = Some(Instant::now() + b.interval);
                    }
                }
            }
            UserEvent::Captured { path, result } => {
                match result {
                    Ok(()) => println!("captured output to {}", path.display()),
                    Err(e) => {
                        eprintln!("capture failed: {e}");
                        self.exit_code = ExitCode::FAILURE;
                    }
                }
                if self.bench.is_none() {
                    self.quit(el);
                }
            }
            UserEvent::OutputFailed(msg) => {
                let e = error::OutputError::Lost(msg);
                self.ui.app_notices.push(e.user_message());
                self.recreate_attempts += 1;
                self.disable_output();
                self.enable_output(el);
                if self.recreate_attempts > 1 || self.output.is_none() {
                    // Unrecoverable per the error-handling rules: the output can't be restored.
                    panic!("output GL context lost and could not be recreated: {e}");
                }
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if let Some((due, media)) = self.switch_background.clone() {
            if Instant::now() >= due {
                self.switch_background = None;
                self.send(Command::SetBackground {
                    media: Some(media),
                    next: None,
                });
            } else {
                el.set_control_flow(ControlFlow::WaitUntil(due));
            }
        }
        if let Some(due) = self.capture_due {
            if Instant::now() >= due {
                self.capture_due = None;
                if let (Some(path), Some(out)) = (self.capture.take(), self.output.as_ref()) {
                    let _ = out.tx.send(RenderCmd::Capture(path));
                }
            } else {
                el.set_control_flow(ControlFlow::WaitUntil(due));
                return;
            }
        }
        if let Some((_, Some(due))) = &self.capture_ui {
            if Instant::now() >= *due {
                self.request_redraw();
            } else {
                el.set_control_flow(ControlFlow::WaitUntil(*due));
            }
        }
        let Some(due) = self.bench.as_ref().and_then(|b| b.next_at) else {
            return;
        };
        if Instant::now() >= due {
            if let Some(b) = &mut self.bench {
                b.next_at = None;
            }
            // Wrap to the start so long benches keep going.
            let at_end = self
                .show
                .live
                .as_ref()
                .is_some_and(|l| pp6core::next_enabled(&l.order, l.position).is_none());
            if at_end && let Some(l) = &self.show.live {
                let first = pp6core::next_enabled(&l.order, None).unwrap_or(0);
                self.send(Command::GoTo {
                    presentation: l.path.clone(),
                    position: first,
                });
            } else {
                self.send(Command::Next);
            }
        } else {
            el.set_control_flow(ControlFlow::WaitUntil(due));
        }
    }
}
