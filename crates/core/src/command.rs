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
    PlaylistNew,
    PlaylistLoad(PathBuf),
    /// Save to the given path, or the current playlist file.
    PlaylistSave(Option<PathBuf>),
    PlaylistAdd {
        path: PathBuf,
        arrangement: Option<String>,
    },
    PlaylistRemove(usize),
    PlaylistMove {
        from: usize,
        to: usize,
    },
    /// Select a playlist item (loads it; the next item is pre-parsed).
    PlaylistSelect(usize),
}
