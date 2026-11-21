use std::path::PathBuf;

/// A slide that can't be drawn faithfully. Recoverable: the slide isn't shown,
/// everything else keeps running.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("{file}: slide {slide_uuid} uses font {font:?}, which isn't installed", file = .file.display())]
    MissingFont {
        font: String,
        file: PathBuf,
        slide_uuid: String,
        element_uuid: String,
    },
    #[error("{file}: slide {slide_uuid} text uses font index {index}, which isn't in its RTF font table", file = .file.display())]
    FontNotInTable {
        index: i32,
        file: PathBuf,
        slide_uuid: String,
        element_uuid: String,
    },
    /// The stored text can't be read (`message` is already a user message).
    #[error("{file}: slide {slide_uuid} text can't be read: {message}", file = .file.display())]
    BadText {
        message: String,
        file: PathBuf,
        slide_uuid: String,
        element_uuid: String,
    },
    #[error("output size {width}x{height} is not usable")]
    BadTarget { width: u32, height: u32 },
}

impl RenderError {
    /// One plain sentence for the operator, plus what happens next.
    pub fn user_message(&self) -> String {
        match self {
            RenderError::MissingFont { font, .. } => format!(
                "Slide uses font '{font}', which isn't installed on this PC. The slide can't be shown until the font is installed."
            ),
            RenderError::FontNotInTable { .. } => {
                "Slide text refers to a font that isn't listed in the slide, so it can't be drawn. The slide is not shown."
                    .to_owned()
            }
            RenderError::BadText { message, .. } => message.clone(),
            RenderError::BadTarget { width, height } => {
                format!("The output size {width}×{height} can't be drawn to. Check the output display settings.")
            }
        }
    }

    /// Debug-level here: callers decide how loudly to report (the output reports
    /// once when a slide fails to go live; thumbnails would repeat it per size).
    pub(crate) fn logged(self) -> Self {
        tracing::debug!(error = %self, "render error");
        self
    }
}

/// Something drawn differently from PP6 (or not at all). Shown in tools and logs.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderWarning {
    pub element_uuid: String,
    pub message: String,
    /// The text is taller than its box (the editor badges the box and slide).
    pub overflow: bool,
    /// An image that can't be shown (missing or unreadable file).
    pub media: bool,
}
