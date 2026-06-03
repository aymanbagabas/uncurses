//! Image-painter trait.
//!
//! Each backend (half-blocks, sixel, kitty, …) is a concrete struct
//! that implements [`Painter`]. Hosts that know their backend up
//! front construct it directly and call [`Painter::paint`] /
//! [`Painter::forget`]. Hosts that want runtime selection wrap the
//! concrete painters in their own enum or generic type — the crate
//! does not ship a built-in dispatch wrapper.

use std::io::{self, Write};

use image::DynamicImage;
use uncurses::Rect;
use uncurses::screen::Screen;

use crate::resize::Resize;

/// Opaque identifier returned by [`Painter::paint`] and accepted by
/// [`Painter::forget`].
///
/// The encoding is private to each painter. Callers should treat
/// [`ImageId`] values as opaque tokens — equal values from the same
/// painter denote the same cached entry, and unknown values are
/// silently ignored by [`Painter::forget`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ImageId(pub u64);

impl ImageId {
    /// Sentinel returned by stateless painters that have no cached
    /// state to drop.
    pub const NONE: ImageId = ImageId(0);

    /// Inner integer value. Useful for diagnostics or logging.
    pub fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for ImageId {
    fn from(v: u64) -> Self {
        ImageId(v)
    }
}

/// Image-painter contract shared by every backend in this crate.
///
/// `paint` stamps an image into a cell rectangle of `screen`,
/// returning a backend-specific [`ImageId`]. `forget` releases any
/// cached state — host-side caches and, where the protocol provides
/// one, the terminal-side registration. `forget` does not blank the
/// painted cells; the host overwrites them through the screen API
/// when it wants them gone.
pub trait Painter {
    /// Paint `image` into `area` of `screen` using `resize` to map
    /// pixels into the cell rectangle.
    ///
    /// Returns a backend-specific id that identifies the cached
    /// entry created (or refreshed) by this paint, or
    /// [`ImageId::NONE`] for backends that hold no cached state.
    fn paint<W: Write>(
        &mut self,
        screen: &mut Screen<W>,
        area: Rect,
        image: &DynamicImage,
        resize: Resize,
    ) -> io::Result<ImageId>;

    /// Release any cached state associated with `id` — host-side
    /// caches and, where the protocol provides one, the
    /// terminal-side registration. Idempotent and a no-op for
    /// unknown ids.
    fn forget<W: Write>(&mut self, screen: &mut Screen<W>, id: ImageId) -> io::Result<()>;
}
