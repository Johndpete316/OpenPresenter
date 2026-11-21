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
    /// Load a presentation into memory only: the operator's grid and the output are
    /// untouched (the phone remote uses this to browse).
    Preload(PathBuf),
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
    /// Edit a loaded presentation's editor copy (one undo step; the output and grid
    /// don't change until Save). An edit that uses something `.pro6` can't hold is
    /// refused in a `.pro6` document unless `accept_native` is set (the user chose
    /// "Continue anyway"): then the touched slides are flagged and the document
    /// switches to native mode.
    Edit {
        presentation: PathBuf,
        edit: doc::Edit,
        accept_native: bool,
    },
    /// Like `Edit`, but consecutive edits with the same `key` (within two seconds) are
    /// one undo step: arrow-key nudges, a colour being dragged in a picker.
    EditMerging {
        presentation: PathBuf,
        edit: doc::Edit,
        key: String,
    },
    /// Undo the last edit of a presentation's editor copy.
    Undo(PathBuf),
    Redo(PathBuf),
    /// Save the editor copy under the save rules (`.pro6` with a `.bak`, or `.opres`
    /// next to the original). A live presentation shows the saved version from the
    /// next slide change.
    Save(PathBuf),
    /// Throw away the editor copy and its undo history.
    Revert(PathBuf),
    /// Auto-advance the live (or selected) presentation using its stored slide timing.
    StartLoop,
    StopLoop,
}
