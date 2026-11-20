//! PDF pages for the built-in PDF viewer.
//!
//! [`messages`] is what the web client and its PDF worker say to each other; it has no
//! dependencies beyond serde, so the web client can speak it without building the renderer in.
//! With the `render` feature, [`Document`] renders pages with hayro, for the desktop client
//! and the worker.

pub mod messages;
#[cfg(feature = "render")]
mod render;
#[cfg(any(test, feature = "sample"))]
mod sample;

#[cfg(feature = "render")]
pub use render::{Document, Rendered};
#[cfg(any(test, feature = "sample"))]
pub use sample::sample_pdf;

use serde::{Deserialize, Serialize};

/// The longest side, in pixels, a page is rendered at; zooming further only enlarges it.
pub const MAX_SIDE_PIXELS: f32 = 8192.0;

/// The size of a page, in PDF points (1/72 inch), as it is shown: rotated and cropped.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

impl PageSize {
    /// The scale that renders this page `width_pixels` wide, kept under [`MAX_SIDE_PIXELS`].
    pub fn scale_for_width(self, width_pixels: f32) -> f32 {
        self.clamp_scale(width_pixels / self.width.max(1.0))
    }

    /// `scale`, lowered as far as needed for the page to fit [`MAX_SIDE_PIXELS`].
    pub fn clamp_scale(self, scale: f32) -> f32 {
        let longest = self.width.max(self.height).max(1.0);
        scale.clamp(f32::EPSILON, MAX_SIDE_PIXELS / longest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scale_for_a_width_fills_it() {
        let page = PageSize {
            width: 612.0,
            height: 792.0,
        };
        assert!((page.scale_for_width(1224.0) - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn huge_scales_stay_within_the_limit() {
        let page = PageSize {
            width: 100.0,
            height: 400.0,
        };
        assert!((page.clamp_scale(1000.0) - MAX_SIDE_PIXELS / 400.0).abs() < f32::EPSILON);
    }
}
