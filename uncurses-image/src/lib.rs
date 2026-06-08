//! Image rendering addon.
//!
//! Drop-in helpers that stamp images into a
//! [`uncurses::screen::Screen`] without modifying the renderer or
//! the cell type. Pixels live entirely outside the cell grid: each
//! painter registers an external paint region with the screen,
//! which stamps [`uncurses::cell::Cell::skip`] placeholders over
//! the image's cell footprint and emits the protocol-specific
//! byte sequence into the screen's output stream after the
//! renderer's cell diff.
//!
//! ## Frame ordering
//!
//! The host calls [`Painter::paint`] before [`uncurses::screen::Screen::render`]:
//!
//! ```text
//! painter.paint(&mut screen, id, area, &image, resize, cell_px)?;
//! screen.render()?;        // cell diff, then region payloads
//! screen.flush()?;
//! ```
//!
//! The cell diff clears the footprint to blanks first, then the
//! image bytes paint on top. To erase a previously painted image
//! call [`Painter::forget`] with the same id.
//!
//! ## Per-cell pixel size
//!
//! Raster backends size their output in pixels and need to know
//! the terminal's per-cell pixel dimensions. The host passes
//! `cell_px` to each [`Painter::paint`]; values typically come
//! from [`uncurses::terminal::get_window_size`] (the `xpixel /
//! col`, `ypixel / row` ratio) or from a CSI-14/16 query.

#![forbid(unsafe_code)]

mod hash;
#[cfg(feature = "iterm2")]
mod iterm2;
mod layout;
mod painter;
mod resize;
#[cfg(feature = "sixel")]
mod sixel;

pub use image::DynamicImage;
pub use image::imageops::FilterType;
#[cfg(feature = "iterm2")]
pub use iterm2::Iterm2;
pub use painter::Painter;
pub use resize::{CropAnchor, Resize};
#[cfg(feature = "sixel")]
pub use sixel::Sixel;
pub use uncurses::screen::RegionId;
