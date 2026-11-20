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
    #[error(transparent)]
    Text(#[from] pro6::ParseError),
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
            RenderError::Text(e) => e.user_message(),
            RenderError::BadTarget { width, height } => {
                format!("The output size {width}×{height} can't be drawn to. Check the output display settings.")
            }
        }
    }

    pub(crate) fn logged(self) -> Self {
        tracing::warn!(error = %self, "render error");
        self
    }
}

/// Something drawn differently from PP6 (or not at all). Shown in tools and logs.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderWarning {
    pub element_uuid: String,
    pub message: String,
}
