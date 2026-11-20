//! Slide → pixels. Text layout with cosmic-text using the RTF font face and size,
//! element positions from `RVRect3D` in canvas pixels, canvas fitted to the output
//! (uniform scale, centred, black bars; never stretched).
//!
//! No window or GL dependencies: the same frames feed `pp6tool render` PNGs and
//! the output window's textures, so both show identical pixels.

mod error;
mod fonts;
mod frame;
mod slide;

pub use error::{RenderError, RenderWarning};
pub use fonts::{Fonts, ResolvedFont};
pub use frame::{PixelRect, Placement, SlideFrame};
pub use slide::render_slide;
