//! Crash-restore state: where the operator was, so a relaunch can pick up again.
//! Written off the core thread (debounced), atomically.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use serde::{Deserialize, Serialize};

use crate::Playlist;

/// Restores older than this bring back the selection but don't put a slide live.
pub const LIVE_RESTORE_MAX_AGE: Duration = Duration::from_secs(6 * 3600);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RestoreState {
    pub saved_at_unix: u64,
    pub playlist_path: Option<PathBuf>,
    /// The playlist itself (it may have unsaved changes).
    pub playlist: Playlist,
    pub playlist_item: Option<usize>,
    pub live: Option<LiveRestore>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveRestore {
    pub path: PathBuf,
    pub arrangement: Option<String>,
    pub position: usize,
    pub text_visible: bool,
    pub background_visible: bool,
}

impl RestoreState {
    pub fn load(path: &Path) -> Option<RestoreState> {
        let text = std::fs::read_to_string(path).ok()?;
        match serde_json::from_str(&text) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "restore file unreadable; starting fresh");
                None
            }
        }
    }

    pub fn age(&self) -> Duration {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Duration::from_secs(now.saturating_sub(self.saved_at_unix))
    }

    /// Whether the live slide should be put back on the output.
    pub fn restore_live(&self) -> bool {
        self.live.is_some() && self.age() <= LIVE_RESTORE_MAX_AGE
    }
}

/// Background writer: keeps only the latest state and writes it at most every `debounce`.
pub fn spawn_writer(path: PathBuf, debounce: Duration) -> std::io::Result<Sender<RestoreState>> {
    let (tx, rx) = crossbeam_channel::unbounded::<RestoreState>();
    std::thread::Builder::new()
        .name("restore-writer".into())
        .spawn(move || writer(&path, &rx, debounce))?;
    Ok(tx)
}

fn writer(path: &Path, rx: &Receiver<RestoreState>, debounce: Duration) {
    let mut pending: Option<RestoreState> = None;
    loop {
        let msg = if pending.is_some() {
            rx.recv_timeout(debounce)
        } else {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        match msg {
            Ok(mut s) => {
                // Drain to the newest.
                while let Ok(newer) = rx.try_recv() {
                    s = newer;
                }
                pending = Some(s);
            }
            Err(RecvTimeoutError::Timeout) => write(path, pending.take()),
            Err(RecvTimeoutError::Disconnected) => {
                write(path, pending.take());
                return;
            }
        }
    }
}

fn write(path: &Path, state: Option<RestoreState>) {
    let Some(mut s) = state else { return };
    s.saved_at_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    match serde_json::to_vec_pretty(&s) {
        Ok(bytes) => {
            if let Err(e) = crate::write_atomic(path, &bytes) {
                tracing::warn!(path = %path.display(), error = %e, "couldn't save restore state");
            }
        }
        Err(e) => tracing::warn!(error = %e, "couldn't encode restore state"),
    }
}
