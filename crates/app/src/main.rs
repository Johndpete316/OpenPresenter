//! `pp6` — M2 output spike: show a presentation on an output display.
//!
//! Controls (output window focused): → / PageDown / Space next, ← / PageUp previous,
//! D disable output, Esc quit. On stdin: `n`, `p`, `g <slide>`, `d` (disable),
//! `e` (enable), `m <display>` (move output), `q`.
//! The operator UI replaces these controls in M3.

mod error;
mod gl;
mod output;
mod raster;

use std::io::BufRead as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use clap::Parser;
use crossbeam_channel::Sender;
use tracing::level_filters::LevelFilter;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowId;

use crate::output::{MonitorChoice, OutputWindow};
use crate::raster::{OutputLink, RasterCmd, RenderCmd, ms};

#[derive(Parser)]
#[command(version, about = "PP6 replacement — output spike (M2)")]
struct Args {
    /// Presentation to show.
    file: PathBuf,
    /// Output display index (see --list-monitors). Default: first non-primary display.
    #[arg(long)]
    monitor: Option<usize>,
    /// Print connected displays and exit.
    #[arg(long)]
    list_monitors: bool,
    /// Use a normal window instead of borderless fullscreen (development).
    #[arg(long)]
    windowed: bool,
    /// Extra font directory (repeatable).
    #[arg(long = "fonts")]
    font_dirs: Vec<PathBuf>,
    /// Advance automatically N times, print timing statistics, and exit.
    #[arg(long)]
    bench: Option<usize>,
    /// Delay between automatic advances in --bench mode.
    #[arg(long, default_value_t = 250)]
    bench_interval_ms: u64,
    /// After the first slide is on screen, save the output's pixels to this PNG and exit.
    #[arg(long)]
    capture: Option<PathBuf>,
    /// Size of the --windowed output window, WIDTHxHEIGHT.
    #[arg(long, value_parser = parse_size, default_value = "960x720")]
    window_size: (u32, u32),
    /// More logging (-v debug).
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Debug, Clone)]
pub enum Control {
    Next,
    Prev,
    GoTo(usize),
    Disable,
    Enable,
    Monitor(usize),
    Quit,
}

#[derive(Debug)]
pub enum UserEvent {
    Control(Control),
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
}

fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s.split_once(['x', 'X']).ok_or("expected WIDTHxHEIGHT")?;
    match (w.trim().parse::<u32>(), h.trim().parse::<u32>()) {
        (Ok(w), Ok(h)) if w > 0 && h > 0 => Ok((w, h)),
        _ => Err(format!("bad size {s:?}")),
    }
}

fn main() -> ExitCode {
    let started = Instant::now();
    let args = Args::parse();
    let level = if args.verbose > 0 {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(level)
        .init();

    let doc = match pro6::load(&args.file) {
        Ok(d) => Arc::new(d),
        Err(e) => {
            eprintln!("{}\n  detail: {e}", e.user_message());
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(
        load_ms = ms(started.elapsed()),
        slides = doc.slides().count(),
        "presentation loaded"
    );

    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("Couldn't connect to the display system: {e}");
            return ExitCode::FAILURE;
        }
    };
    let proxy = event_loop.create_proxy();
    spawn_stdin(proxy.clone());

    let link: OutputLink = Arc::new(Mutex::new(None));
    let (raster_tx, raster_thread) = match raster::spawn(
        doc.clone(),
        args.font_dirs.clone(),
        link.clone(),
        proxy.clone(),
    ) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Couldn't start the slide renderer: {e}");
            return ExitCode::FAILURE;
        }
    };

    let enabled: Vec<bool> = doc.slides().map(|s| s.enabled).collect();
    let mut app = App {
        started,
        choice: args
            .monitor
            .map_or(MonitorChoice::PreferSecondary, MonitorChoice::Index),
        windowed: args.windowed.then_some(args.window_size),
        capture: args.capture.clone(),
        list_monitors: args.list_monitors,
        current: enabled.iter().position(|e| *e).unwrap_or(0),
        enabled,
        proxy,
        link,
        raster_tx,
        raster_thread: Some(raster_thread),
        output: None,
        sent_size: None,
        started_once: false,
        cold_start_logged: false,
        recreate_attempts: 0,
        latencies: Vec::new(),
        bench: args.bench.map(|n| Bench {
            remaining: n,
            interval: Duration::from_millis(args.bench_interval_ms),
            next_at: None,
        }),
        exit_code: ExitCode::SUCCESS,
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("The display event loop stopped unexpectedly: {e}");
        return ExitCode::FAILURE;
    }
    app.exit_code
}

fn spawn_stdin(proxy: EventLoopProxy<UserEvent>) {
    let _ = std::thread::Builder::new()
        .name("stdin".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                let mut parts = line.split_whitespace();
                let num = |p: Option<&str>| p.and_then(|s| s.parse::<usize>().ok());
                let cmd = match parts.next() {
                    Some("n") => Control::Next,
                    Some("p") => Control::Prev,
                    Some("g") => match num(parts.next()) {
                        Some(n) if n > 0 => Control::GoTo(n - 1),
                        _ => continue,
                    },
                    Some("d") => Control::Disable,
                    Some("e") => Control::Enable,
                    Some("m") => match num(parts.next()) {
                        Some(n) => Control::Monitor(n),
                        None => continue,
                    },
                    Some("q") => Control::Quit,
                    _ => continue,
                };
                if proxy.send_event(UserEvent::Control(cmd)).is_err() {
                    break;
                }
            }
        });
}

struct Bench {
    remaining: usize,
    interval: Duration,
    next_at: Option<Instant>,
}

struct App {
    started: Instant,
    choice: MonitorChoice,
    /// `Some(size)` for a plain development window instead of fullscreen.
    windowed: Option<(u32, u32)>,
    capture: Option<PathBuf>,
    list_monitors: bool,
    enabled: Vec<bool>,
    current: usize,
    proxy: EventLoopProxy<UserEvent>,
    link: OutputLink,
    raster_tx: Sender<RasterCmd>,
    raster_thread: Option<JoinHandle<()>>,
    output: Option<OutputWindow>,
    /// Output size last sent to the raster worker (avoids duplicate re-renders).
    sent_size: Option<(u32, u32)>,
    started_once: bool,
    cold_start_logged: bool,
    recreate_attempts: u32,
    latencies: Vec<Duration>,
    bench: Option<Bench>,
    exit_code: ExitCode,
}

impl App {
    fn enable_output(&mut self, el: &ActiveEventLoop) -> bool {
        if self.output.is_some() {
            return true;
        }
        match OutputWindow::create(el, self.choice, self.windowed, self.proxy.clone()) {
            Ok(o) => {
                if let Ok(mut l) = self.link.lock() {
                    *l = Some(o.tx.clone());
                }
                let (width, height) = o.size();
                let _ = self.raster_tx.send(RasterCmd::Size { width, height });
                self.sent_size = Some((width, height));
                self.output = Some(o);
                true
            }
            Err(e) => {
                tracing::error!(error = %e, "output window failed");
                eprintln!("{}", e.user_message());
                false
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

    fn go(&mut self, index: usize) {
        if index < self.enabled.len() {
            self.current = index;
            println!("slide {}/{}", index + 1, self.enabled.len());
            let _ = self.raster_tx.send(RasterCmd::Show {
                index,
                requested_at: Some(Instant::now()),
            });
        }
    }

    /// Next/previous *enabled* slide. Returns false at the ends.
    fn step(&mut self, forward: bool) -> bool {
        let found = if forward {
            (self.current + 1..self.enabled.len()).find(|&i| self.enabled[i])
        } else {
            (0..self.current).rev().find(|&i| self.enabled[i])
        };
        match found {
            Some(i) => {
                self.go(i);
                true
            }
            None => false,
        }
    }

    fn control(&mut self, el: &ActiveEventLoop, c: Control) {
        tracing::debug!(?c, "control");
        match c {
            Control::Next => {
                self.step(true);
            }
            Control::Prev => {
                self.step(false);
            }
            Control::GoTo(i) => self.go(i),
            Control::Disable => self.disable_output(),
            Control::Enable => {
                self.enable_output(el);
            }
            Control::Monitor(i) => {
                self.disable_output();
                self.choice = MonitorChoice::Index(i);
                self.enable_output(el);
            }
            Control::Quit => self.quit(el),
        }
    }

    fn quit(&mut self, el: &ActiveEventLoop) {
        self.disable_output();
        let _ = self.raster_tx.send(RasterCmd::Stop);
        if let Some(t) = self.raster_thread.take()
            && let Err(panic) = t.join()
        {
            std::panic::resume_unwind(panic);
        }
        el.exit();
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
        let monitors = output::describe_monitors(el);
        if self.list_monitors {
            for m in &monitors {
                println!("{m}");
            }
            self.quit(el);
            return;
        }
        tracing::info!(monitors = ?monitors, "displays");
        if !self.enable_output(el) {
            self.exit_code = ExitCode::FAILURE;
            self.quit(el);
            return;
        }
        let first = self.current;
        let _ = self.raster_tx.send(RasterCmd::Show {
            index: first,
            requested_at: None,
        });
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(out) = self.output.as_ref().filter(|o| o.id() == id) else {
            return;
        };
        match event {
            WindowEvent::Resized(s) => {
                let (width, height) = (s.width.max(1), s.height.max(1));
                let _ = out.tx.send(RenderCmd::Resize { width, height });
                if self.sent_size != Some((width, height)) {
                    self.sent_size = Some((width, height));
                    let _ = self.raster_tx.send(RasterCmd::Size { width, height });
                }
            }
            WindowEvent::RedrawRequested => {
                let _ = out.tx.send(RenderCmd::Redraw);
            }
            WindowEvent::CloseRequested => self.quit(el),
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                let c = match event.logical_key.as_ref() {
                    Key::Named(NamedKey::ArrowRight | NamedKey::PageDown | NamedKey::Space) => {
                        Control::Next
                    }
                    Key::Named(NamedKey::ArrowLeft | NamedKey::PageUp) => Control::Prev,
                    Key::Named(NamedKey::Escape) => Control::Quit,
                    Key::Character(s) if s.eq_ignore_ascii_case("d") => Control::Disable,
                    _ => return,
                };
                self.control(el, c);
            }
            _ => {}
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Control(c) => self.control(el, c),
            UserEvent::FontsReady { elapsed, faces } => {
                tracing::info!(faces, elapsed_ms = ms(elapsed), "fonts ready");
            }
            UserEvent::RenderFailed { index, message } => {
                tracing::warn!(slide = index + 1, %message, "slide not shown");
                eprintln!("slide {}: {message}", index + 1);
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
                }
                if let Some(l) = latency {
                    tracing::info!(
                        slide = index.map(|i| i + 1),
                        latency_ms = ms(l),
                        upload_ms = ms(upload),
                        "slide change presented"
                    );
                    self.latencies.push(l);
                    if let Some(b) = &mut self.bench {
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
                self.quit(el);
            }
            UserEvent::OutputFailed(msg) => {
                let e = error::OutputError::Lost(msg);
                eprintln!("{}", e.user_message());
                self.recreate_attempts += 1;
                self.disable_output();
                if self.recreate_attempts > 1 || !self.enable_output(el) {
                    // Unrecoverable per the error-handling rules: the output can't be restored.
                    panic!("output GL context lost and could not be recreated: {e}");
                }
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let Some(due) = self.bench.as_ref().and_then(|b| b.next_at) else {
            el.set_control_flow(ControlFlow::Wait);
            return;
        };
        if Instant::now() >= due {
            if let Some(b) = &mut self.bench {
                b.next_at = None;
            }
            if !self.step(true) {
                // Wrap to the first enabled slide so long benches keep going.
                let first = self.enabled.iter().position(|e| *e).unwrap_or(0);
                self.go(first);
            }
            el.set_control_flow(ControlFlow::Wait);
        } else {
            el.set_control_flow(ControlFlow::WaitUntil(due));
        }
    }
}
