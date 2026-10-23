//! Native graphics and ordinary-cell images for uncurses.

mod color;
pub mod dither;
mod geometry;
mod image;
mod native;
mod options;
mod protocols;
pub mod quant;

pub use ::image::{ImageFormat, imageops::FilterType};
pub use image::Image;
pub use native::PreparedImage;
pub use options::{Crop, Fit, ImageOptions};
pub use protocols::{kitty_placeholders::CellImage, symbols::SymbolSet};

/// A native terminal image protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// Kitty graphics with shared uploads and separate placements.
    Kitty,
    /// iTerm2 inline PNG graphics.
    Iterm2,
    /// Sixel indexed graphics.
    Sixel,
}

fn invalid(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message)
}
