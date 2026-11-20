//! `pp6` — the operator app: egui operator window + output window, both on one
//! winit event loop; the output's GL runs on its own thread, and the show state
//! lives in the core actor.

mod crash;
mod error;
mod gl;
mod keys;
mod operator;
mod output;
mod raster;
mod thumbs;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Parser;
use crossbeam_channel::{Receiver, Sender};
use pp6core::library::LibraryIndex;
use pp6core::{Command, CoreHandle, RestoreState, Settings, ShowState};
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
#[command(version, about = "PP6 replacement")]
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
}

struct Paths {
    settings: PathBuf,
    restore: PathBuf,
    logs: PathBuf,
    library_cache: PathBuf,
    thumbs: PathBuf,
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
        restore: data.join("restore.json"),
        logs: data.join("logs"),
        library_cache: cache.join("library.json"),
        thumbs: cache.join("thumbnails"),
    }
}

fn main() -> ExitCode {
    let started = Instant::now();
    let args = Args::parse();
    let paths = paths(args.data_dir.clone());
    let crash = crash::init_logging(paths.logs.clone(), args.verbose);
    crash::install_panic_hook(crash.clone());
    tracing::info!(version = env!("CARGO_PKG_VERSION"), settings = %paths.settings.display(), logs = %paths.logs.display(), "starting");

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

    let restore = if args.no_restore {
        None
    } else {
        RestoreState::load(&paths.restore)
    };
    let restore_writer =
        pp6core::restore::spawn_writer(paths.restore.clone(), Duration::from_millis(500)).ok();
    let (core, _core_thread) = match pp6core::spawn(pp6core::CoreConfig {
        restore,
        restore_writer,
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

    if let Some(p) = &args.open {
        core.send(Command::Open(p.clone()));
        core.send(Command::GoTo {
            presentation: p.clone(),
            position: 0,
        });
    }

    let media_files = list_media(&settings.media_dirs);
    let mut app = App {
        started,
        ui: UiState {
            library: cached,
            library_status: "Checking library…".into(),
            media_files,
            app_notices,
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
        operator: None,
        output: None,
        sent_size: None,
        monitor_override: args.monitor,
        windowed_output: args.windowed_output,
        capture: args.capture,
        capture_ui: args.capture_ui.map(|p| (p, None)),
        bench: args.bench.map(|n| Bench {
            remaining: n,
            interval: Duration::from_millis(args.bench_interval_ms),
            next_at: None,
        }),
        latencies: Vec::new(),
        cold_start_logged: false,
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

fn list_media(dirs: &[PathBuf]) -> Vec<PathBuf> {
    const EXT: &[&str] = &[
        "png", "jpg", "jpeg", "bmp", "gif", "mp4", "mov", "m4v", "mkv", "avi", "wmv", "webm",
    ];
    let mut files: Vec<PathBuf> = dirs
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| EXT.iter().any(|x| e.eq_ignore_ascii_case(x)))
        })
        .collect();
    files.sort();
    files
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
    operator: Option<OperatorWindow>,
    output: Option<OutputWindow>,
    sent_size: Option<(u32, u32)>,
    monitor_override: Option<usize>,
    windowed_output: bool,
    capture: Option<PathBuf>,
    /// Operator-window capture path and when to take it.
    capture_ui: Option<(PathBuf, Option<Instant>)>,
    bench: Option<Bench>,
    latencies: Vec<Duration>,
    cold_start_logged: bool,
    recreate_attempts: u32,
    started_once: bool,
    quitting: bool,
    exit_code: ExitCode,
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
        let windowed = self.windowed_output.then_some((960, 720));
        match OutputWindow::create(el, self.monitor_choice(), windowed, self.proxy.clone()) {
            Ok(o) => {
                if let Ok(mut l) = self.link.lock() {
                    *l = Some(o.tx.clone());
                }
                let (width, height) = o.size();
                let _ = self.raster_tx.send(RasterCmd::Size { width, height });
                self.sent_size = Some((width, height));
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
                    self.ui.media_files = list_media(&self.settings.media_dirs);
                }
            }
            UiAction::OpenPlaylist => {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Playlist", &["toml"])
                    .pick_file()
                {
                    self.send(Command::PlaylistLoad(p));
                }
            }
            UiAction::SavePlaylistAs => {
                if let Some(mut p) = rfd::FileDialog::new()
                    .add_filter("Playlist", &["toml"])
                    .set_file_name("playlist.toml")
                    .save_file()
                {
                    if p.extension().is_none() {
                        p.set_extension("toml");
                    }
                    self.send(Command::PlaylistSave(Some(p)));
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
            WindowEvent::CloseRequested => self.quit(el),
            WindowEvent::Resized(s) => {
                op.resize(s.width, s.height);
                op.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let mut actions = Vec::new();
                let log = self.crash.ui_log.snapshot();
                self.ui.receive_thumbs(&op.egui.egui_ctx, &self.thumbs_rx);
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
            UserEvent::FontsReady { elapsed, faces } => {
                tracing::info!(faces, elapsed_ms = ms(elapsed), "fonts ready")
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
                    if let (Some(path), Some(out)) = (self.capture.take(), self.output.as_ref()) {
                        let _ = out.tx.send(RenderCmd::Capture(path));
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
