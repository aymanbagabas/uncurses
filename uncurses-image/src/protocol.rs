//! Image protocol selection and backend trait.

use std::io::Write;

use uncurses::Rect;
use uncurses::screen::{Capabilities, Screen};

use crate::image_src::Image;
use crate::placement::{Erase, Placement};
use crate::resize::Resize;

pub mod halfblocks;
pub mod kitty;
#[cfg(feature = "sixel")]
pub mod sixel;

/// Image rendering protocol.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {
    /// Pick the best supported protocol from current capabilities.
    /// Resolved on every paint so that capability updates (e.g. a
    /// late XTVERSION reply) take effect on the next frame.
    #[default]
    Auto,
    /// Unicode half-block characters with foreground / background
    /// colors. Always available; no capability required.
    HalfBlocks,
    /// Kitty graphics protocol.
    Kitty,
    /// DEC Sixel raster graphics.
    Sixel,
    /// iTerm2 inline image protocol (also supported by WezTerm, Rio).
    Iterm2,
}

impl ImageProtocol {
    /// Resolve `Auto` against the given capabilities. Concrete
    /// variants pass through unchanged. If a raster protocol is
    /// chosen but no `cell_pixel_size` is known, falls back to
    /// half-blocks so the layer can paint something useful.
    pub fn resolve(self, caps: &Capabilities) -> ImageProtocol {
        let has_pixels = caps.cell_pixel_size.is_some();
        let kitty = caps.kitty_graphics == Some(true);
        let sixel_supported = caps.sixel == Some(true) && cfg!(feature = "sixel");
        let iterm2 = caps.iterm2_graphics == Some(true);
        let resolved = match self {
            Self::Auto => {
                if kitty && has_pixels {
                    Self::Kitty
                } else if sixel_supported && has_pixels {
                    Self::Sixel
                } else if iterm2 && has_pixels {
                    Self::Iterm2
                } else {
                    Self::HalfBlocks
                }
            }
            other => other,
        };

        // Raster protocols need cell_pixel_size to compute output
        // dimensions. Without it, fall back to half-blocks rather
        // than emit something the terminal can't size.
        let resolved = match resolved {
            Self::Kitty | Self::Sixel | Self::Iterm2 if !has_pixels => Self::HalfBlocks,
            other => other,
        };

        // If the sixel backend was explicitly requested but the
        // crate was built without the `sixel` feature, fall back to
        // half-blocks rather than no-op.
        if !cfg!(feature = "sixel") && resolved == Self::Sixel {
            return Self::HalfBlocks;
        }
        resolved
    }
}

/// Backend-specific paint context handed to a [`Backend`] for one
/// placement.
#[allow(dead_code)] // some fields used only by raster backends
pub(crate) struct PaintCtx<'a> {
    pub id: crate::placement::ImageId,
    pub image: &'a Image,
    pub placement: &'a Placement,
    pub caps: &'a Capabilities,
}

/// Trait implemented by each protocol backend.
///
/// Backends operate in two phases:
///
/// 1. `reserve` is called **before** [`Screen::render`]. It stamps
///    cells into the surface (so the renderer's diff sees the
///    placement area) and may emit raw bytes through the screen
///    writer for protocols that need to ship payload **before** the
///    cell-update frame (e.g. Kitty Unicode placeholders need the
///    image transmitted before the placeholder cells appear).
/// 2. `paint` is invoked **after** `screen.render()` has flushed
///    cell-update bytes; backends emit raw protocol payload through
///    the screen writer for protocols that paint **after** cells
///    (sixel, iTerm2 inline images).
pub(crate) trait Backend {
    /// Stamp `area` into the surface and emit any pre-frame bytes.
    fn reserve<W: Write>(
        &mut self,
        ctx: &PaintCtx<'_>,
        screen: &mut Screen<W>,
    ) -> std::io::Result<()>;

    /// Emit raw protocol bytes for this placement after the renderer
    /// has flushed. Default is a no-op (used by protocols that paint
    /// only via cells).
    fn paint<W: Write>(
        &mut self,
        ctx: &PaintCtx<'_>,
        screen: &mut Screen<W>,
    ) -> std::io::Result<()> {
        let _ = (ctx, screen);
        Ok(())
    }

    /// Per-frame finalize. Called once after every per-placement
    /// `paint` for the current frame. Backends use it to flush any
    /// queued terminal-side cleanup (e.g. Kitty `_Ga=d,d=I,…`).
    fn finalize<W: Write>(&mut self, screen: &mut Screen<W>) -> std::io::Result<()> {
        let _ = screen;
        Ok(())
    }

    /// Notify the backend that an image was removed from the layer.
    /// Backends that retain terminal-side state (e.g. Kitty's image
    /// registry) queue the corresponding cleanup here; emission
    /// happens in the next `finalize`.
    fn on_image_removed(&mut self, id: crate::placement::ImageId) {
        let _ = id;
    }

    /// Emit any backend-side cleanup for an erased placement.
    fn erase<W: Write>(&mut self, erase: &Erase, screen: &mut Screen<W>) -> std::io::Result<()> {
        let _ = (erase, screen);
        Ok(())
    }

    /// Final teardown — release any terminal-side resources owned by
    /// this backend.
    fn shutdown<W: Write>(&mut self, screen: &mut Screen<W>) -> std::io::Result<()> {
        let _ = screen;
        Ok(())
    }
}

/// Compute the pixel dimensions the source image should be resized
/// to in order to fill `area` according to `resize`, given a per-cell
/// pixel size. Returns `(width_px, height_px)`.
#[allow(dead_code)] // used by raster backends added in a follow-up
pub(crate) fn target_pixel_size(
    src: (u32, u32),
    area: Rect,
    cell_px: (u16, u16),
    resize: Resize,
) -> (u32, u32) {
    let cell_w = cell_px.0.max(1) as u32;
    let cell_h = cell_px.1.max(1) as u32;
    let area_w = (area.width as u32) * cell_w;
    let area_h = (area.height as u32) * cell_h;

    match resize {
        Resize::Scale(_) => (area_w.max(1), area_h.max(1)),
        Resize::Fit(_) => fit_inside(src, (area_w, area_h)),
        Resize::Crop(_) => fit_cover(src, (area_w, area_h)),
    }
}

#[allow(dead_code)]
fn fit_inside(src: (u32, u32), bounds: (u32, u32)) -> (u32, u32) {
    if src.0 == 0 || src.1 == 0 || bounds.0 == 0 || bounds.1 == 0 {
        return (bounds.0.max(1), bounds.1.max(1));
    }
    let sx = bounds.0 as f64 / src.0 as f64;
    let sy = bounds.1 as f64 / src.1 as f64;
    let s = sx.min(sy).min(1.0);
    let w = ((src.0 as f64) * s).round().max(1.0) as u32;
    let h = ((src.1 as f64) * s).round().max(1.0) as u32;
    (w, h)
}

#[allow(dead_code)]
fn fit_cover(src: (u32, u32), bounds: (u32, u32)) -> (u32, u32) {
    if src.0 == 0 || src.1 == 0 || bounds.0 == 0 || bounds.1 == 0 {
        return (bounds.0.max(1), bounds.1.max(1));
    }
    let sx = bounds.0 as f64 / src.0 as f64;
    let sy = bounds.1 as f64 / src.1 as f64;
    let s = sx.max(sy);
    let w = ((src.0 as f64) * s).round().max(1.0) as u32;
    let h = ((src.1 as f64) * s).round().max(1.0) as u32;
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps_with(
        kitty: bool,
        sixel: bool,
        iterm2: Option<bool>,
        cell_px: Option<(u16, u16)>,
    ) -> Capabilities {
        Capabilities {
            kitty_graphics: Some(kitty),
            sixel: Some(sixel),
            iterm2_graphics: iterm2,
            cell_pixel_size: cell_px,
            ..Default::default()
        }
    }

    #[test]
    fn auto_prefers_kitty_when_pixels_known() {
        let caps = caps_with(true, true, Some(true), Some((10, 20)));
        assert_eq!(ImageProtocol::Auto.resolve(&caps), ImageProtocol::Kitty);
    }

    #[test]
    fn auto_falls_back_to_halfblocks_without_cell_pixels() {
        let caps = caps_with(true, true, Some(true), None);
        assert_eq!(
            ImageProtocol::Auto.resolve(&caps),
            ImageProtocol::HalfBlocks
        );
    }

    #[test]
    fn explicit_kitty_falls_back_when_pixels_unknown() {
        let caps = caps_with(true, false, None, None);
        assert_eq!(
            ImageProtocol::Kitty.resolve(&caps),
            ImageProtocol::HalfBlocks
        );
    }

    #[test]
    fn explicit_halfblocks_passes_through() {
        let caps = caps_with(false, false, None, None);
        assert_eq!(
            ImageProtocol::HalfBlocks.resolve(&caps),
            ImageProtocol::HalfBlocks
        );
    }

    #[test]
    #[cfg(feature = "sixel")]
    fn auto_chooses_sixel_over_iterm2() {
        let caps = caps_with(false, true, Some(true), Some((10, 20)));
        assert_eq!(ImageProtocol::Auto.resolve(&caps), ImageProtocol::Sixel);
    }

    #[test]
    #[cfg(not(feature = "sixel"))]
    fn auto_chooses_iterm2_when_sixel_feature_off() {
        // Without the sixel feature, sixel caps don't promote to Sixel;
        // iTerm2 wins next.
        let caps = caps_with(false, true, Some(true), Some((10, 20)));
        assert_eq!(ImageProtocol::Auto.resolve(&caps), ImageProtocol::Iterm2);
    }

    #[test]
    fn fit_preserves_aspect() {
        // 200x100 image, 100x100 bounds → scale 0.5 → 100x50.
        assert_eq!(fit_inside((200, 100), (100, 100)), (100, 50));
    }

    #[test]
    fn fit_never_enlarges() {
        // 50x50 image, 200x200 bounds → s=min(4,4,1)=1 → 50x50.
        assert_eq!(fit_inside((50, 50), (200, 200)), (50, 50));
    }

    #[test]
    fn cover_overflows_one_axis() {
        // 200x100 image, 100x100 bounds → s=max(0.5,1)=1 → 200x100.
        assert_eq!(fit_cover((200, 100), (100, 100)), (200, 100));
    }
}
