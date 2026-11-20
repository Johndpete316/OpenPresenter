//! Logging setup and the crash handler.
//!
//! Logs: daily rolling files (debug for our crates), stderr (info), plus two
//! in-memory rings: the last lines at debug level for crash reports, and recent
//! info lines for the operator UI.
//!
//! Panics are unrecoverable faults: the hook writes a crash report (version, OS,
//! panic + backtrace, show state, recent commands, recent log lines) to the logs
//! folder, shows a native dialog with the report's path, and exits immediately.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::level_filters::LevelFilter;
use tracing_subscriber::Layer as _;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

const CRASH_LINES: usize = 400;
const UI_LINES: usize = 300;
const RECENT_COMMANDS: usize = 50;

/// Bounded line buffer usable as a tracing writer.
#[derive(Clone, Default)]
pub struct Ring {
    lines: Arc<Mutex<VecDeque<String>>>,
    cap: usize,
}

impl Ring {
    fn new(cap: usize) -> Ring {
        Ring {
            lines: Arc::default(),
            cap,
        }
    }

    pub fn push(&self, line: String) {
        if let Ok(mut l) = self.lines.lock() {
            l.push_back(line);
            while l.len() > self.cap {
                l.pop_front();
            }
        }
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.lines
            .lock()
            .map(|l| l.iter().cloned().collect())
            .unwrap_or_default()
    }
}

pub struct RingWriter(Ring, Vec<u8>);

impl std::io::Write for RingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.1.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for RingWriter {
    fn drop(&mut self) {
        let text = String::from_utf8_lossy(&self.1);
        for line in text.lines().filter(|l| !l.is_empty()) {
            self.0.push(line.to_owned());
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Ring {
    type Writer = RingWriter;
    fn make_writer(&'a self) -> Self::Writer {
        RingWriter(self.clone(), Vec::new())
    }
}

/// Shared context the crash handler reads.
#[derive(Clone)]
pub struct CrashContext {
    pub logs_dir: PathBuf,
    pub crash_log: Ring,
    pub ui_log: Ring,
    /// Latest `ShowState::summary()`.
    pub show_state: Arc<Mutex<String>>,
    pub recent_commands: Ring,
}

static FILE_GUARD: OnceLock<Mutex<Option<tracing_appender::non_blocking::WorkerGuard>>> =
    OnceLock::new();

/// Install logging. Returns the crash context to hand to `install_panic_hook`.
pub fn init_logging(logs_dir: PathBuf, verbose: bool) -> CrashContext {
    let ctx = CrashContext {
        crash_log: Ring::new(CRASH_LINES),
        ui_log: Ring::new(UI_LINES),
        show_state: Arc::new(Mutex::new(String::from("(not started)"))),
        recent_commands: Ring::new(RECENT_COMMANDS),
        logs_dir: logs_dir.clone(),
    };
    let ours = |level: LevelFilter| {
        Targets::new()
            .with_default(LevelFilter::INFO)
            .with_target("pp6", level)
            .with_target("pp6core", level)
            .with_target("pro6", level)
            .with_target("render", level)
    };
    let file_layer = match std::fs::create_dir_all(&logs_dir) {
        Ok(()) => {
            let appender = tracing_appender::rolling::daily(&logs_dir, "pp6.log");
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let _ = FILE_GUARD.set(Mutex::new(Some(guard)));
            Some(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(writer)
                    .with_filter(ours(LevelFilter::DEBUG)),
            )
        }
        Err(e) => {
            eprintln!(
                "Couldn't create the logs folder {}: {e}. Logging to the console only.",
                logs_dir.display()
            );
            None
        }
    };
    let stderr_level = if verbose {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    tracing_subscriber::registry()
        .with(file_layer)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(ours(stderr_level)),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(ctx.crash_log.clone())
                .with_filter(ours(LevelFilter::DEBUG)),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .without_time()
                .with_writer(ctx.ui_log.clone())
                .with_filter(ours(LevelFilter::INFO)),
        )
        .init();
    ctx
}

pub fn install_panic_hook(ctx: CrashContext) {
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_owned();
        let report = build_report(&ctx, &message, &location, &thread, &backtrace.to_string());
        let path = write_report(&ctx, &report);
        tracing::error!(%message, %location, %thread, report = ?path, "unrecoverable fault; exiting");
        // Flush the file log before exiting.
        if let Some(g) = FILE_GUARD.get()
            && let Ok(mut g) = g.lock()
        {
            drop(g.take());
        }
        let where_ = path.as_ref().map_or_else(
            || "(the report couldn't be written)".to_owned(),
            |p| p.display().to_string(),
        );
        eprintln!(
            "\nPP6 hit an unrecoverable error and must close: {message}\nCrash report: {where_}"
        );
        if std::env::var_os("PP6_NO_CRASH_DIALOG").is_none() {
            let _ = rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("PP6 has to close")
                .set_description(format!(
                    "PP6 hit an unrecoverable error and must close.\n\n{message}\n\nA crash report was saved to:\n{where_}\n\nRestart PP6; it will return to where you were."
                ))
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        }
        std::process::exit(101);
    }));
}

fn build_report(
    ctx: &CrashContext,
    message: &str,
    location: &str,
    thread: &str,
    backtrace: &str,
) -> String {
    let mut r = String::new();
    let _ = writeln!(r, "PP6 crash report");
    let _ = writeln!(r, "version: {}", crate::VERSION);
    let _ = writeln!(r, "os: {} {}", std::env::consts::OS, std::env::consts::ARCH);
    let _ = writeln!(r, "time (unix): {}", unix_now());
    let _ = writeln!(r, "thread: {thread}");
    let _ = writeln!(r, "panic: {message}");
    let _ = writeln!(r, "at: {location}");
    let _ = writeln!(
        r,
        "\n== show state ==\n{}",
        ctx.show_state.lock().map(|s| s.clone()).unwrap_or_default()
    );
    let _ = writeln!(r, "== recent commands (oldest first) ==");
    for c in ctx.recent_commands.snapshot() {
        let _ = writeln!(r, "{c}");
    }
    let _ = writeln!(r, "\n== backtrace ==\n{backtrace}");
    let _ = writeln!(r, "== last log lines ==");
    for l in ctx.crash_log.snapshot() {
        let _ = writeln!(r, "{l}");
    }
    r
}

fn write_report(ctx: &CrashContext, report: &str) -> Option<PathBuf> {
    let _ = std::fs::create_dir_all(&ctx.logs_dir);
    let path = ctx.logs_dir.join(format!("crash-{}.txt", unix_now()));
    let mut f = std::fs::File::create(&path).ok()?;
    f.write_all(report.as_bytes()).ok()?;
    f.sync_all().ok()?;
    Some(path)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
