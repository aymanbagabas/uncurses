//! Image-painter trait.
//!
//! Each backend (sixel, …) is a concrete struct that implements
//! [`Painter`]. Hosts that know their backend up front construct
//! it directly and call [`Painter::paint`] / [`Painter::forget`].
//! Hosts that want runtime selection wrap the concrete painters
//! in their own enum or generic type.

use std::io::{self, Write};

use image::DynamicImage;
use uncurses::Rect;
use uncurses::screen::{RegionId, Screen};

use crate::resize::Resize;

/// Image-painter contract shared by every backend in this crate.
///
/// `paint` stamps Skip cells over the image footprint in the
/// screen's front buffer and registers a paint region whose
/// payload is the encoded protocol byte sequence. The screen
/// emits the payload on every [`Screen::render`] after the cell
/// diff, so the image bytes paint over the diff's blanks.
///
/// `forget` releases any cached state plus the screen-side
/// region registration.
pub trait Painter {
    /// Stamp `image` into `area` of `screen` using `resize` and
    /// `cell_px` (terminal cell pixel size, width × height) to map
    /// pixels into the cell rectangle.
    ///
    /// `id` is a caller-allocated [`RegionId`]: distinct paint
    /// instances must use distinct ids — even when they paint the
    /// same pixels at different positions. Calling [`Self::paint`]
    /// twice with the same id replaces the earlier registration:
    /// any cells the previous footprint owned that aren't in the
    /// new footprint (and aren't covered by another region) are
    /// released back to blank.
    fn paint<W: Write>(
        &mut self,
        screen: &mut Screen<W>,
        id: RegionId,
        area: Rect,
        image: &DynamicImage,
        resize: Resize,
        cell_px: (u16, u16),
    ) -> io::Result<()>;

    /// Release any cached state associated with `id` plus the
    /// screen-side region registration. Idempotent and a no-op for
    /// unknown ids.
    fn forget<W: Write>(&mut self, screen: &mut Screen<W>, id: RegionId) -> io::Result<()>;
}
