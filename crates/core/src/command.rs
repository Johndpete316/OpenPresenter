//! The one command type. Operator UI, clicker/keyboard and the web remote all send these.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Next enabled slide of the live presentation (disabled slides are skipped).
    Next,
    /// Previous enabled slide.
    Prev,
    ClearText,
    ClearBackground,
    ClearAll,
    /// Put a slide live: `position` indexes the presentation's current play order
    /// (see `ShowState::order_for`). Disabled slides can still be shown this way.
    GoTo {
        presentation: PathBuf,
        position: usize,
    },
    /// Choose an arrangement (`None` = natural order). If the presentation is live,
    /// the current slide stays on screen.
    SelectArrangement {
        presentation: PathBuf,
        arrangement: Option<String>,
    },
    /// Load a presentation (for display in the grid) without changing the output.
    Open(PathBuf),
    OutputEnable(bool),
    /// Background media (image or video, looping) under the slide text; `None` clears
    /// it. `next` is pre-loaded so triggering it later is instant.
    SetBackground {
        media: Option<PathBuf>,
        next: Option<PathBuf>,
    },
    /// A new, empty playlist (saved automatically in the playlists folder).
    PlaylistCreate(String),
    PlaylistRename {
        playlist: usize,
        name: String,
    },
    /// Remove a playlist. Its file is moved into the playlists folder's `deleted`
    /// sub-folder, not destroyed.
    PlaylistDelete(usize),
    /// Copy a playlist file in as a new playlist.
    PlaylistImport(PathBuf),
    /// Append to `playlist`. With no playlists at all, one is created first.
    PlaylistAdd {
        playlist: usize,
        path: PathBuf,
        arrangement: Option<String>,
    },
    PlaylistRemove(crate::ItemRef),
    PlaylistMove {
        playlist: usize,
        from: usize,
        to: usize,
    },
    /// Select a playlist item (loads it; the next item is pre-parsed).
    PlaylistSelect(crate::ItemRef),
    /// Edit a loaded presentation (in memory; marks it unsaved).
    Edit {
        presentation: PathBuf,
        op: pro6::ops::Op,
    },
    /// Save a presentation's edits to its file (the previous version is kept as `.pro6.bak`).
    Save(PathBuf),
    /// Throw away unsaved edits and reload from disk.
    Revert(PathBuf),
    /// Auto-advance the live (or selected) presentation using its stored slide timing.
    StartLoop,
    StopLoop,
}
