//! Image rendering addon.
//!
//! Drop-in helpers that stamp images into a
//! [`uncurses::screen::Screen`] without modifying the renderer or
//! the cell type. Two protocol backends ship in this stage:
//!
//! - [`Halfblocks`] — uses the `▀` glyph with foreground / background
//!   colors to pack two image rows per terminal row. Works on any
//!   color-capable terminal.
//! - [`Kitty`] — Unicode-placeholder mode of the kitty graphics
//!   protocol. Transmits the image once per host-id and stamps
//!   placeholder cells that bind to the registered virtual placement.
//!
//! Each backend is a small struct. Halfblocks is stateless. Kitty
//! retains a per-host-id table so re-painting the same image without
//! changes incurs zero re-transmits.
//!
//! ## Host id contract
//!
//! Backends that cache (currently [`Kitty`]) key their cache by a
//! `u64` host id supplied at paint time. The host must:
//!
//! - Use the same id while the image's pixels are unchanged.
//! - Use a fresh id (or call [`Kitty::forget`]) when the pixels
//!   change.
//!
//! Re-using an id with different pixels keeps the stale registration.
//!
//! ## Per-cell pixel size
//!
//! Raster backends consult [`uncurses::screen::Screen::cell_pixel_size`].
//! The host populates that cache by feeding terminal size events into
//! [`uncurses::screen::Screen::update_window_size`]. When the cache
//! is unset, a backend that needs it falls back to a non-pixel-aware
//! placement (or returns without writing — see each backend).

#![forbid(unsafe_code)]

mod halfblocks;
mod hash;
mod kitty;
mod resize;
#[cfg(feature = "sixel")]
mod sixel;

pub use halfblocks::Halfblocks;
pub use image::DynamicImage;
pub use image::imageops::FilterType;
pub use kitty::Kitty;
pub use resize::{CropAnchor, Resize};
#[cfg(feature = "sixel")]
pub use sixel::Sixel;
