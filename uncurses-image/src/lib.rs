//! Images for cell surfaces and native terminal placements.
//!
//! Keep an [`Image`] across frames to share its immutable pixels. [`ImageOptions`]
//! controls fitting, scaling, source cropping, and the terminal cell geometry.
//! [`SymbolSet`] selects glyphs for ordinary cells and the normal text diff.
//! [`ImageRenderer`] selects a detected protocol or falls back to those cells.
//! [`ImageCapabilities::detect`] combines terminal replies and environment hints.
//! [`Placeholders`] provides Kitty images through ordinary text.
//! [`ImageGroup`] combines relative images and can follow placeholder anchors.
//!
//! Native drawing requires protocol support. Sixel also requires
//! compatible cursor and character-erasure modes. All screen drawing methods
//! stage desired content; the screen performs terminal output during `render`.

mod color;
pub mod dither;
mod geometry;
mod image;
mod options;
mod protocols;
pub mod quant;
mod renderer;

pub use ::image::{DynamicImage, RgbImage, RgbaImage, imageops::FilterType};
pub use image::Image;
pub use options::{Crop, Fit, ImageOptions};
pub use protocols::{ImageCapabilities, ImageGroup, Placeholders, Protocol, SymbolSet};
pub use renderer::ImageRenderer;

use std::io;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
