//! What a `.pro6` file can hold. Every editor feature declares its compatibility
//! here, in one place (docs/EDITOR-PLAN.md, Compatibility Matrix).

use serde::{Deserialize, Serialize};

/// How a document is saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Saved as `.pro6` (opened from one, or new).
    #[default]
    Pro6,
    /// Uses something `.pro6` can't hold; saved as `.opres` next to the original.
    Native,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Supported,
    NativeOnly,
    /// Not checked against PP6 yet; treated as [`Support::NativeOnly`].
    Unverified,
}

/// Editor features, one per row of the compatibility matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    /// Slides, groups, arrangements, labels, enable/disable, notes.
    SlideStructure,
    /// Text content in the existing style.
    Text,
    FontFamily,
    FontSize,
    TextColor,
    Bold,
    Italic,
    Underline,
    HorizontalAlign,
    VerticalAlign,
    LineSpacing,
    LetterSpacing,
    ElementFrame,
    ElementRotation,
    ElementOpacity,
    BackgroundColor,
    TextShadow,
    TextOutline,
    ImageElement,
    ImageBackground,
    ImageAdjustColor,
    Vignette,
    LayerLock,
    LayerHide,
}

impl Feature {
    /// Name for the "won't work in ProPresenter 6" message.
    pub fn name(self) -> &'static str {
        match self {
            Feature::SlideStructure => "Slides and groups",
            Feature::Text => "Text",
            Feature::FontFamily => "Font",
            Feature::FontSize => "Font size",
            Feature::TextColor => "Text colour",
            Feature::Bold => "Bold",
            Feature::Italic => "Italic",
            Feature::Underline => "Underline",
            Feature::HorizontalAlign => "Text alignment",
            Feature::VerticalAlign => "Vertical alignment",
            Feature::LineSpacing => "Line spacing",
            Feature::LetterSpacing => "Letter spacing",
            Feature::ElementFrame => "Position and size",
            Feature::ElementRotation => "Rotation",
            Feature::ElementOpacity => "Opacity",
            Feature::BackgroundColor => "Background colour",
            Feature::TextShadow => "Drop shadow",
            Feature::TextOutline => "Outline",
            Feature::ImageElement => "Image",
            Feature::ImageBackground => "Background image",
            Feature::ImageAdjustColor => "Image adjustments",
            Feature::Vignette => "Vignette",
            Feature::LayerLock => "Lock",
            Feature::LayerHide => "Hide",
        }
    }
}

/// Whether `.pro6` can hold `feature`.
pub fn pro6_support(feature: Feature) -> Support {
    use Feature::*;
    match feature {
        SlideStructure | Text | FontFamily | FontSize | TextColor | Bold | Italic | Underline
        | HorizontalAlign | VerticalAlign | ElementFrame | ElementRotation | ElementOpacity
        | BackgroundColor => Support::Supported,
        LineSpacing | LetterSpacing | TextShadow | TextOutline | ImageElement | ImageBackground
        | ImageAdjustColor | LayerLock | LayerHide => Support::Unverified,
        Vignette => Support::NativeOnly,
    }
}

/// True when using `feature` would make a `.pro6` document native-only.
pub fn needs_native(feature: Feature) -> bool {
    pro6_support(feature) != Support::Supported
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_start_state() {
        assert_eq!(pro6_support(Feature::FontFamily), Support::Supported);
        assert_eq!(pro6_support(Feature::Vignette), Support::NativeOnly);
        assert_eq!(pro6_support(Feature::TextShadow), Support::Unverified);
        assert!(
            needs_native(Feature::TextShadow),
            "unverified counts as native-only"
        );
        assert!(!needs_native(Feature::SlideStructure));
    }
}
