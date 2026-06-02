//! Image rendering addon for the uncurses terminal library.
//!
//! Place images into a [`uncurses::screen::Screen`] without modifying
//! the renderer or the cell type. Supports the half-blocks fallback
//! (no extra dependencies beyond `image`), Kitty graphics, Sixel, and
//! iTerm2 inline images.
//!
//! See [`ImageLayer`] for the entry point.

#![forbid(unsafe_code)]

mod error;
mod image_src;
mod layer;
mod placement;
mod protocol;
mod resize;

pub use error::{Error, Result};
pub use image::imageops::FilterType;
pub use image_src::Image;
pub use layer::ImageLayer;
pub use placement::ImageId;
pub use protocol::ImageProtocol;
pub use resize::{CropAnchor, Resize};
