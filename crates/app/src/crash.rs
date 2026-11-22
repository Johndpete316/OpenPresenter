//! Logging setup and the crash handler.
//!
//! Logs: daily rolling files (debug for our crates), stderr (info), plus two
//! in-memory rings: the last lines at debug level for crash reports, and recent
//! info lines for the operator UI.
//!
//! Every panic writes a crash report (version, OS, panic + backtrace, show state,
//! recent commands, recent log lines) to the logs folder. On a show-critical thread
//! (`CRITICAL_THREADS`) it's unrecoverable: a native dialog shows the report's path
//! and the program exits. Elsewhere the operator gets a toast and the thread unwinds.

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

/// Threads the show can't run without. A panic anywhere else (phone remote, ESV,
/// thumbnails, exports, library scan, file loading, …) is reported and that work
/// stops, but the output stays up: closing mid-service is worse than a lost feature.
const CRITICAL_THREADS: &[&str] = &["main", "core", "raster", "output-render"];

type Notifier = Box<dyn Fn(String) + Send + Sync>;
static NOTIFY: OnceLock<Notifier> = OnceLock::new();

/// Where non-fatal panics are told to the operator (a toast).
pub fn set_notifier(f: impl Fn(String) + Send + Sync + 'static) {
    let _ = NOTIFY.set(Box::new(f));
}

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
            .with_target("openpresenter", level)
            .with_target("opcore", level)
            .with_target("pro6", level)
            .with_target("render", level)
    };
    let file_layer = match std::fs::create_dir_all(&logs_dir) {
        Ok(()) => {
            let appender = tracing_appender::rolling::daily(&logs_dir, "openpresenter.log");
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
        let where_ = path.as_ref().map_or_else(
            || "(the report couldn't be written)".to_owned(),
            |p| p.display().to_string(),
        );
        if !CRITICAL_THREADS.contains(&thread.as_str()) {
            // Let this thread unwind; the show keeps going.
            tracing::error!(%message, %location, %thread, report = ?path, "internal error in a background task; continuing");
            if let Some(n) = NOTIFY.get() {
                n(format!(
                    "Something went wrong in the background ({thread}: {message}). The show keeps running, but that task may have stopped; restart OpenPresenter after the service. Report: {where_}"
                ));
            }
            return;
        }
        tracing::error!(%message, %location, %thread, report = ?path, "unrecoverable fault; exiting");
        // Flush the file log before exiting.
        if let Some(g) = FILE_GUARD.get()
            && let Ok(mut g) = g.lock()
        {
            drop(g.take());
        }
        eprintln!(
            "\nOpenPresenter hit an unrecoverable error and must close: {message}\nCrash report: {where_}"
        );
        if std::env::var_os("OPENPRESENTER_NO_CRASH_DIALOG").is_none() {
            let _ = rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("OpenPresenter has to close")
                .set_description(format!(
                    "OpenPresenter hit an unrecoverable error and must close.\n\n{message}\n\nA crash report was saved to:\n{where_}\n\nRestart OpenPresenter and choose the slide again (it starts blank)."
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
    let _ = writeln!(r, "OpenPresenter crash report");
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A panic on a background thread is reported and that thread stops, but the
    /// process keeps running (the hook only exits for `CRITICAL_THREADS`).
    #[test]
    fn background_panic_is_reported_not_fatal() {
        let dir = std::env::temp_dir().join(format!("op-crash-test-{}", std::process::id()));
        let ctx = CrashContext {
            logs_dir: dir.clone(),
            crash_log: Ring::new(4),
            ui_log: Ring::new(4),
            show_state: Arc::new(Mutex::new(String::new())),
            recent_commands: Ring::new(4),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        set_notifier(move |m| {
            let _ = tx.send(m);
        });
        install_panic_hook(ctx);
        let r = std::thread::Builder::new()
            .name("esv".into())
            .spawn(|| panic!("boom"))
            .map(std::thread::JoinHandle::join);
        assert!(matches!(r, Ok(Err(_))), "the thread unwound");
        let msg = rx.recv_timeout(std::time::Duration::from_secs(5));
        assert!(msg.is_ok_and(|m| m.contains("esv: boom")));
        let reports = std::fs::read_dir(&dir).map(Iterator::count).unwrap_or(0);
        assert_eq!(reports, 1, "a crash report was written");
        let _ = std::panic::take_hook();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
