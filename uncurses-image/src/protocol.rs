//! Backend-agnostic wrapper over the image painters.
//!
//! [`Protocol`] is an enum that owns one of the concrete painters
//! ([`Halfblocks`], [`Sixel`], [`Kitty`]) and re-exposes a uniform
//! `paint` / `forget` surface. Hosts that select a backend at
//! runtime (capability detection, CLI flag, config) operate on
//! `Protocol`; hosts that know their backend up front can keep
//! using the typed APIs directly.

use std::io::{self, Write};

use image::DynamicImage;
use uncurses::Rect;
use uncurses::screen::Screen;

use crate::halfblocks::Halfblocks;
use crate::kitty::Kitty;
use crate::resize::Resize;
#[cfg(feature = "sixel")]
use crate::sixel::Sixel;

/// One of the supported image-rendering backends.
#[derive(Debug)]
pub enum Protocol {
    /// Two-rows-per-cell `▀` glyph rendering. Stateless.
    Halfblocks(Halfblocks),
    /// Sixel DCS sequences anchored at a single cell.
    #[cfg(feature = "sixel")]
    Sixel(Sixel),
    /// Kitty graphics protocol, Unicode placeholder mode.
    Kitty(Kitty),
}

impl Protocol {
    /// Stamp `image` into `area` of `screen`. Returns the
    /// pixel-content id the caller can later pass to [`Self::forget`].
    /// The returned id is `0` for [`Protocol::Halfblocks`], which
    /// has no cached state to drop.
    pub fn paint<W: Write>(
        &mut self,
        screen: &mut Screen<W>,
        area: Rect,
        image: &DynamicImage,
        resize: Resize,
    ) -> io::Result<u64> {
        match self {
            Protocol::Halfblocks(p) => {
                p.paint(screen, area, image, resize);
                Ok(0)
            }
            #[cfg(feature = "sixel")]
            Protocol::Sixel(p) => p.paint(screen, area, image, resize),
            Protocol::Kitty(p) => p.paint(screen, area, image, resize),
        }
    }

    /// Drop any cached state (host-side and terminal-side) for `id`.
    /// `id` is the value returned by a prior [`Self::paint`]. A
    /// no-op for [`Protocol::Halfblocks`] and for backends that
    /// never registered the id.
    pub fn forget<W: Write>(&mut self, screen: &mut Screen<W>, id: u64) -> io::Result<()> {
        match self {
            Protocol::Halfblocks(_) => Ok(()),
            #[cfg(feature = "sixel")]
            Protocol::Sixel(p) => {
                let _ = screen;
                p.forget(id);
                Ok(())
            }
            Protocol::Kitty(p) => p.forget(screen, id),
        }
    }
}
