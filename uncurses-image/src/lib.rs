//! Image rendering addon.
//!
//! Drop-in helpers that stamp images into a
//! [`uncurses::screen::Screen`] without modifying the renderer or
//! the cell type. Three protocol backends ship today:
//!
//! - [`Halfblocks`] — uses the `▀` glyph with foreground / background
//!   colors to pack two image rows per terminal row. Works on any
//!   color-capable terminal. Stateless.
//! - [`Sixel`] — DCS-encoded sixel sequence anchored at a single
//!   cell. Caches encoded bytes per `(pixel content, cell rect, cell
//!   pixel size, resize)`.
//! - [`Kitty`] — Unicode-placeholder mode of the kitty graphics
//!   protocol. Transmits each unique image once and stamps
//!   placeholder cells that bind to the registered virtual placement.
//!
//! Backends that cache derive image identity from the source pixel
//! data; callers do not supply identities. Each [`Painter::paint`]
//! returns an [`ImageId`] the host can later pass to
//! [`Painter::forget`] to drop the cached state.
//!
//! Every backend implements the [`Painter`] trait. Hosts that select
//! a backend at runtime wrap the concrete painters in their own
//! enum or generic; this crate does not ship a built-in dispatch
//! wrapper.
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
mod painter;
mod resize;
#[cfg(feature = "sixel")]
mod sixel;

pub use halfblocks::Halfblocks;
pub use image::DynamicImage;
pub use image::imageops::FilterType;
pub use kitty::Kitty;
pub use painter::{ImageId, Painter};
pub use resize::{CropAnchor, Resize};
#[cfg(feature = "sixel")]
pub use sixel::Sixel;
