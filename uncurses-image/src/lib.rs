//! Image rendering addon.
//!
//! Drop-in helpers that stamp images into a
//! [`uncurses::screen::Screen`] without modifying the renderer or
//! the cell type. Pixels live entirely outside the cell grid: each
//! painter stamps [`uncurses::cell::Cell::skip`] placeholders over
//! the image's cell footprint and emits the protocol-specific byte
//! sequence into the screen's output stream after the renderer's
//! cell diff.
//!
//! ## Frame ordering
//!
//! Each painter is a two-phase emitter:
//!
//! 1. [`Painter::paint`] — stamps Skip cells over the footprint in
//!    the screen's front buffer and queues the encoded byte
//!    sequence for emission.
//! 2. [`Painter::draw`] — drains the queue into the screen's output
//!    buffer.
//!
//! Hosts call them around [`uncurses::screen::Screen::render`]:
//!
//! ```text
//! painter.paint(&mut screen, area, &image, resize, cell_px)?;
//! screen.render()?;        // cell diff (Skip cells emit as blanks)
//! painter.draw(&mut screen)?;  // image bytes paint over the blanks
//! screen.flush()?;
//! ```
//!
//! The cell diff clears the footprint to blanks first, then the
//! image bytes paint on top. To erase a previously painted image,
//! the host overwrites its old footprint cells through the screen
//! API (e.g. [`uncurses::buffer::SurfaceMut::fill_rect`] with
//! [`uncurses::cell::Cell::BLANK`]) before the next paint.
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
mod painter;
mod resize;
#[cfg(feature = "sixel")]
mod sixel;

pub use image::DynamicImage;
pub use image::imageops::FilterType;
pub use painter::{ImageId, Painter};
pub use resize::{CropAnchor, Resize};
#[cfg(feature = "sixel")]
pub use sixel::Sixel;
