//! ProPresenter 6 `.pro6` document model and parser.
//!
//! This crate must stay free of GUI, GPU and media dependencies.

pub mod error;
pub mod raw;

pub use error::{ParseError, ParseErrorKind};
