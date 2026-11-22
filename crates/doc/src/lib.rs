//! OpenPresenter's native presentation model, used app-wide (John, 2026-11-21):
//! core, render, thumbnails and the phone remote all work on [`Document`].
//!
//! - `.pro6` files are converted on load and written back by [`save`] in `Pro6`
//!   mode, byte-identical where nothing changed ([`convert`]).
//! - `.opres` is the native file (JSON, [`FORMAT_VERSION`]); a document switches to
//!   it only when the user accepts a feature `.pro6` can't hold ([`compat`]).
//! - Every change is an [`Edit`]; undo keeps snapshots ([`History`]).

pub mod compat;
pub mod convert;
pub mod edit;
pub mod file;
pub mod history;
pub mod lyrics;
pub mod model;
pub mod rich;
pub mod style;
pub mod theme;

pub use compat::{Feature, Mode, Support, needs_native, pro6_support};
pub use convert::{ExportChoice, ExportError, ExportReport, export_pro6, from_pro6, to_pro6};
pub use edit::{Check, Edit, EditError, check, mark_native, new_uuid};
pub use file::{LoadError, SaveError, is_native, is_pro6, load, open, preferred, save, save_path};
pub use history::History;
pub use model::*;
pub use style::{Scope, Style};
pub use theme::{Theme, ThemeLink, ThemeParts};

/// Native presentation file extension.
pub const NATIVE_EXT: &str = "opres";
/// Theme file extension.
pub const THEME_EXT: &str = "optheme";
/// Current `.opres` format version.
pub const FORMAT_VERSION: u32 = 1;
