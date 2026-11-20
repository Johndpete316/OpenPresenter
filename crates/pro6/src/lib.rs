//! ProPresenter 6 `.pro6` document model and parser.
//!
//! This crate must stay free of GUI, GPU and media dependencies.
//!
//! Parsing is two-layered: [`raw`] builds a lossless XML tree, and [`model`] builds a
//! typed view on top of it. Every node keeps what the typed model doesn't understand
//! in an [`Extra`], so nothing in the file is ever dropped.

pub mod arrangement;
pub mod diff;
pub mod edit;
pub mod error;
pub mod extra;
pub mod model;
pub mod raw;
pub mod rescale;
pub mod rtf;
pub mod schema;
pub mod write;

use std::path::Path;
use std::time::Instant;

pub use arrangement::Arrangement;
pub use edit::{EditError, WinPayloads};
pub use error::{ParseError, ParseErrorDetail, ParseErrorKind};
pub use extra::{Extra, Leaf, TypedArray, Warning};
pub use model::{
    CanvasSize, DisplayElement, Document, Payload, Rect3D, Rgba, Shadow, Slide, SlideGroup, Stroke,
    TextElement, Timeline,
};
pub use raw::RawDocument;
pub use write::{WriteError, save, to_raw, write_string};

use error::Ctx;

/// Read and parse a `.pro6` file.
pub fn load(path: &Path) -> Result<Document, ParseError> {
    let _span = tracing::info_span!("load_pro6", path = %path.display()).entered();
    let started = Instant::now();
    let src = read_utf8(path)?;
    let doc = parse_str(&src, path)?;
    tracing::debug!(
        elapsed_us = started.elapsed().as_micros() as u64,
        groups = doc.groups().len(),
        warnings = doc.warnings.len(),
        "loaded"
    );
    Ok(doc)
}

/// Parse `.pro6` XML. `path` is used only for error context.
pub fn parse_str(src: &str, path: &Path) -> Result<Document, ParseError> {
    Document::from_raw(parse_raw(src, path)?, path)
}

/// Read a file as UTF-8, with a typed error on failure.
pub fn read_utf8(path: &Path) -> Result<String, ParseError> {
    let ctx = Ctx::root(path);
    let bytes = std::fs::read(path).map_err(|e| ctx.err(ParseErrorKind::Io(e)))?;
    String::from_utf8(bytes).map_err(|e| {
        ctx.err(ParseErrorKind::Encoding {
            offset: e.utf8_error().valid_up_to(),
        })
    })
}

/// Parse only the lossless raw tree (used by tools that inventory unknown structure).
pub fn parse_raw(src: &str, path: &Path) -> Result<RawDocument, ParseError> {
    raw::parse(src, &Ctx::root(path))
}

impl Document {
    /// Build the typed model from a raw tree.
    pub fn from_raw(raw: RawDocument, path: &Path) -> Result<Document, ParseError> {
        model::document_from_raw(raw, path)
    }
}
