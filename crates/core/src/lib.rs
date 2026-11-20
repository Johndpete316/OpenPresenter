//! Show core: state, commands, play order, playlists, settings, crash-restore and the
//! library index. No GUI, GPU or media dependencies.

mod actor;
mod command;
pub mod library;
mod order;
mod playlist;
pub mod restore;
mod settings;
mod state;

use std::io::Write as _;
use std::path::Path;

pub use actor::{CoreConfig, CoreHandle, spawn};
pub use command::Command;
pub use order::{SlideRef, find_nearest, next_enabled, prev_enabled, slide_order};
pub use playlist::{Playlist, PlaylistError, PlaylistItem};
pub use restore::{LiveRestore, RestoreState};
pub use settings::{KeyBindings, OutputSettings, Settings, SettingsError};
pub use state::{Live, Notice, NoticeLevel, ShowState};

/// Write a file atomically: temp file in the same folder, flush to disk, rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
