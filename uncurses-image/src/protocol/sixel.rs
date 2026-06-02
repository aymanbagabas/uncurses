//! Sixel raster graphics backend.
//!
//! Each placement is encoded as a sixel DCS sequence (`\x1bPq…\x1b\\`)
//! and emitted **after** the renderer's frame so the diff doesn't
//! overwrite the rasterized pixels. The backend stamps blanks into
//! the placement's cells during `reserve` so the renderer wipes any
//! stale text underneath the image; once the diff settles, those
//! blank cells are stable and the sixel pixels persist between
//! frames without needing retransmission.
//!
//! Cursor handling: the entire post-frame burst is wrapped in
//! DECSC / DECRC (`\x1b7` / `\x1b8`) so terminal-side cursor state
//! returns to wherever the renderer last left it.

use std::fmt::Write as _;
use std::io::Write;

use icy_sixel::{EncodeOptions, sixel_encode};
use rustc_hash::FxHashMap;
use uncurses::Rect;
use uncurses::cell::Cell;
use uncurses::screen::Screen;

use crate::placement::{Erase, ImageId};
use crate::resize::Resize;

use super::{Backend, PaintCtx};

#[derive(Debug, Default)]
pub(crate) struct Sixel {
    /// Pre-encoded sixel sequence per image, keyed by content hash
    /// and target pixel size. A cache miss forces re-encoding.
    cache: FxHashMap<ImageId, SixelCache>,
}

#[derive(Debug, Clone)]
struct SixelCache {
    sequence: String,
    signature: CacheKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CacheKey {
    content_hash: u64,
    target_px: (u32, u32),
}

impl Backend for Sixel {
    fn reserve<W: Write>(
        &mut self,
        ctx: &PaintCtx<'_>,
        screen: &mut Screen<W>,
    ) -> std::io::Result<()> {
        // Stamp blanks so the renderer's diff wipes any text in the
        // placement region. The actual sixel payload goes out in
        // `paint`, after the renderer has flushed.
        let area = clip_area(ctx.placement.area, screen);
        for y in 0..area.height {
            for x in 0..area.width {
                screen.set_cell((area.x + x, area.y + y), &Cell::BLANK);
            }
        }
        Ok(())
    }

    fn paint<W: Write>(
        &mut self,
        ctx: &PaintCtx<'_>,
        screen: &mut Screen<W>,
    ) -> std::io::Result<()> {
        let area = clip_area(ctx.placement.area, screen);
        if area.width == 0 || area.height == 0 {
            return Ok(());
        }
        let cell_px = match ctx.caps.cell_pixel_size {
            Some(px) => px,
            // The layer's protocol resolver should have already fallen
            // back to half-blocks when cell_pixel_size is unknown.
            None => return Ok(()),
        };

        let key = CacheKey {
            content_hash: ctx.image.content_hash(),
            target_px: target_pixels(area, cell_px, ctx.image.dimensions(), ctx.placement.resize),
        };

        if !self.cache.get(&ctx.id).is_some_and(|c| c.signature == key) {
            let sequence = encode(ctx, key.target_px).map_err(io::other)?;
            self.cache.insert(
                ctx.id,
                SixelCache {
                    sequence,
                    signature: key,
                },
            );
        }
        let cache_entry = &self.cache[&ctx.id];

        // DECSC, CUP to placement origin (1-based), payload, DECRC.
        let mut buf = String::with_capacity(cache_entry.sequence.len() + 16);
        buf.push_str("\x1b7");
        write!(buf, "\x1b[{};{}H", area.y as u32 + 1, area.x as u32 + 1).expect("write to String");
        buf.push_str(&cache_entry.sequence);
        buf.push_str("\x1b8");
        screen.write_all(buf.as_bytes())?;
        Ok(())
    }

    fn erase<W: Write>(&mut self, _erase: &Erase, _screen: &mut Screen<W>) -> std::io::Result<()> {
        // The layer already blanked the cells; the renderer's diff
        // wipes the sixel pixels by repainting the cells.
        Ok(())
    }

    fn on_image_removed(&mut self, id: ImageId) {
        self.cache.remove(&id);
    }
}

mod io {
    /// Convert any error implementing `Display` into an `io::Error`
    /// with kind `Other`. Used to bridge `icy_sixel`'s error type.
    pub(super) fn other<E: std::fmt::Display>(err: E) -> std::io::Error {
        std::io::Error::other(err.to_string())
    }
}

/// Compute the encoded image dimensions in pixels, in the placement's
/// pixel space. `Resize::Fit` shrinks to preserve aspect ratio,
/// `Resize::Crop` and `Resize::Scale` fill the full area.
fn target_pixels(area: Rect, cell_px: (u16, u16), src: (u32, u32), resize: Resize) -> (u32, u32) {
    let cw = cell_px.0.max(1) as u32;
    let ch = cell_px.1.max(1) as u32;
    let area_px_w = (area.width as u32) * cw;
    let area_px_h = (area.height as u32) * ch;
    if area_px_w == 0 || area_px_h == 0 {
        return (area_px_w.max(1), area_px_h.max(1));
    }
    match resize {
        Resize::Scale(_) | Resize::Crop(_) => (area_px_w, area_px_h),
        Resize::Fit(_) => {
            if src.0 == 0 || src.1 == 0 {
                return (area_px_w, area_px_h);
            }
            let sx = area_px_w as f64 / src.0 as f64;
            let sy = area_px_h as f64 / src.1 as f64;
            let s = sx.min(sy).min(1.0);
            let w = ((src.0 as f64) * s).round().max(1.0) as u32;
            let h = ((src.1 as f64) * s).round().max(1.0) as u32;
            (w, h)
        }
    }
}

/// Resize the source image to `target` and run it through the sixel
/// encoder. The returned string includes the leading `\x1bPq` and
/// trailing `\x1b\\` markers.
fn encode(
    ctx: &PaintCtx<'_>,
    target: (u32, u32),
) -> std::result::Result<String, icy_sixel::SixelError> {
    let filter = match ctx.placement.resize {
        Resize::Scale(f) | Resize::Fit(f) => f,
        Resize::Crop(_) => image::imageops::FilterType::Triangle,
    };

    let resized = match ctx.placement.resize {
        Resize::Crop(_) => ctx
            .image
            .pixels()
            .resize_to_fill(target.0, target.1, filter)
            .to_rgba8(),
        Resize::Fit(_) | Resize::Scale(_) => ctx
            .image
            .pixels()
            .resize_exact(target.0, target.1, filter)
            .to_rgba8(),
    };

    sixel_encode(
        resized.as_raw(),
        resized.width() as usize,
        resized.height() as usize,
        &EncodeOptions::default(),
    )
}

fn clip_area<W: Write>(area: Rect, screen: &Screen<W>) -> Rect {
    let sw = screen.width();
    let sh = screen.height();
    let x = area.x.min(sw);
    let y = area.y.min(sh);
    let w = area.width.min(sw.saturating_sub(x));
    let h = area.height.min(sh.saturating_sub(y));
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_pixels_scale_uses_full_area() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        };
        let px = target_pixels(
            area,
            (10, 20),
            (100, 100),
            Resize::Scale(image::imageops::FilterType::Triangle),
        );
        assert_eq!(px, (40, 40));
    }

    #[test]
    fn target_pixels_fit_preserves_aspect() {
        // 200x100 src into 40x40 px area → scale 0.2 → 40x20.
        let area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        };
        let px = target_pixels(
            area,
            (10, 20),
            (200, 100),
            Resize::Fit(image::imageops::FilterType::Triangle),
        );
        assert_eq!(px, (40, 20));
    }
}
