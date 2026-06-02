//! iTerm2 inline image backend (OSC 1337 `File=`).
//!
//! Each placement is encoded as a PNG, base64-wrapped, and emitted
//! after the renderer's frame as an OSC 1337 inline-image sequence:
//!
//! ```text
//! ESC ] 1337 ; File = inline=1 ; width=N ; height=M ; preserveAspectRatio=… : <base64-png> BEL
//! ```
//!
//! The placeholder cells are blanked during `reserve` so the
//! renderer's diff wipes any text underneath; the OSC burst itself
//! is wrapped in DECSC/DECRC so terminal-side cursor state survives.
//!
//! Reference: <https://iterm2.com/documentation-images.html>

use std::fmt::Write as _;
use std::io::{Cursor, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::ImageFormat;
use rustc_hash::FxHashMap;
use uncurses::Rect;
use uncurses::cell::Cell;
use uncurses::screen::Screen;

use crate::placement::{Erase, ImageId};
use crate::resize::Resize;

use super::{Backend, PaintCtx};

#[derive(Debug, Default)]
pub(crate) struct Iterm2 {
    /// Pre-encoded OSC sequence per image, keyed by content hash and
    /// area in cells. The terminal does the cell-to-pixel scaling, so
    /// only the cell-dimensioned area matters for cache validity.
    cache: FxHashMap<ImageId, Iterm2Cache>,
}

#[derive(Debug, Clone)]
struct Iterm2Cache {
    sequence: String,
    signature: CacheKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CacheKey {
    content_hash: u64,
    /// Area in cells passed to the terminal as `width=` / `height=`.
    cells: (u16, u16),
    /// Per-cell pixel size at encode time. Affects whether we emit
    /// the size args in cells (`N`) or pixels (`Npx`); changing
    /// invalidates the cached OSC payload.
    cell_px: Option<(u16, u16)>,
    /// Resize policy fingerprint; affects `preserveAspectRatio` and
    /// whether we pre-resize the source pixels.
    resize: ResizeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResizeKind {
    Fit,
    Crop,
    Scale,
}

impl From<Resize> for ResizeKind {
    fn from(r: Resize) -> Self {
        match r {
            Resize::Fit(_) => Self::Fit,
            Resize::Crop(_) => Self::Crop,
            Resize::Scale(_) => Self::Scale,
        }
    }
}

impl Backend for Iterm2 {
    fn reserve<W: Write>(
        &mut self,
        ctx: &PaintCtx<'_>,
        screen: &mut Screen<W>,
    ) -> std::io::Result<()> {
        // Stamp blanks so the renderer wipes any underlying text. The
        // image bytes ship from `paint` after the renderer flushes.
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

        // Defer painting until cell_pixel_size is known. The OSC 1337
        // bare-cell width/height form is part of the spec but is
        // accepted inconsistently across implementers; once a raster
        // burst lands on the terminal canvas it stays there until the
        // host explicitly clears the cells underneath. Emitting the
        // pixel form (`Npx`) only after the probe reply means we paint
        // exactly once, with the size args every implementer honors.
        if ctx.caps.cell_pixel_size.is_none() {
            return Ok(());
        }

        let key = CacheKey {
            content_hash: ctx.image.content_hash(),
            cells: (area.width, area.height),
            cell_px: ctx.caps.cell_pixel_size,
            resize: ctx.placement.resize.into(),
        };

        if !self.cache.get(&ctx.id).is_some_and(|c| c.signature == key) {
            let sequence = build_sequence(ctx, area)?;
            self.cache.insert(
                ctx.id,
                Iterm2Cache {
                    sequence,
                    signature: key,
                },
            );
        }
        let entry = &self.cache[&ctx.id];

        // Position the cursor at the placement origin (planner-aware,
        // handles inline / fullscreen modes), emit the OSC 1337
        // burst, then snap the cursor back to the origin via CUU so
        // the renderer's tracked position stays in sync.
        //
        // The OSC includes `doNotMoveCursor=0`, which makes the
        // terminal advance the cursor by `area.height` rows after
        // rendering — emitting an explicit CUU by the same amount
        // returns the cursor to the placement origin without needing
        // to invalidate the renderer's bookkeeping.
        screen.set_cursor_position(area.x, area.y)?;
        screen.write_all(entry.sequence.as_bytes())?;
        if area.height > 0 {
            uncurses::ansi::cursor::write_cuu(screen, area.height)?;
        }
        Ok(())
    }

    fn erase<W: Write>(&mut self, _erase: &Erase, _screen: &mut Screen<W>) -> std::io::Result<()> {
        // Cells are blanked by the layer; the renderer's diff wipes
        // the image pixels by repainting the placement region.
        Ok(())
    }

    fn on_image_removed(&mut self, id: ImageId) {
        self.cache.remove(&id);
    }
}

/// Build the OSC 1337 sequence for one placement, including the
/// base64-encoded PNG payload and the pixel dimensions. Caller has
/// verified `caps.cell_pixel_size.is_some()`.
fn build_sequence(ctx: &PaintCtx<'_>, area: Rect) -> std::io::Result<String> {
    let png_bytes = encode_png(ctx, area)?;
    let (cw, ch) = ctx
        .caps
        .cell_pixel_size
        .expect("paint() guarantees cell_pixel_size is Some");
    let w_px = (area.width as u32) * (cw.max(1) as u32);
    let h_px = (area.height as u32) * (ch.max(1) as u32);

    // Reserve a generous amount: header + base64(payload). base64
    // expands by ~4/3.
    let mut out = String::with_capacity(64 + (png_bytes.len() * 4 / 3) + 8);
    out.push_str("\x1b]1337;File=");
    out.push_str("inline=1;");
    // doNotMoveCursor=0 makes the terminal advance the cursor by
    // `area.height` rows after rendering. Coupled with an explicit
    // CUU after the burst this leaves the cursor exactly at the
    // placement origin, in sync with the renderer's tracked
    // position. (Some implementations honor this flag; on those
    // that ignore it the explicit CUU still works because they
    // also advance the cursor by the row count.)
    out.push_str("doNotMoveCursor=0;");
    write!(
        out,
        "size={};width={}px;height={}px;preserveAspectRatio={}:",
        png_bytes.len(),
        w_px,
        h_px,
        match ctx.placement.resize {
            Resize::Scale(_) => 0,
            Resize::Fit(_) | Resize::Crop(_) => 1,
        },
    )
    .expect("write to String");
    STANDARD.encode_string(&png_bytes, &mut out);
    // BEL terminator. iTerm2 also accepts ST (`ESC \`); BEL is
    // shorter and what the iTerm2 docs use in their examples.
    out.push('\x07');
    Ok(out)
}

/// Encode the (possibly pre-resized) source image to PNG. For
/// `Resize::Crop` the source is pre-resized via `resize_to_fill` so
/// the rendered cell area is filled without distortion; for the
/// other modes the terminal does the scaling.
fn encode_png(ctx: &PaintCtx<'_>, area: Rect) -> std::io::Result<Vec<u8>> {
    let cell_px = ctx.caps.cell_pixel_size;
    let dyn_img = match ctx.placement.resize {
        Resize::Crop(_) => match cell_px {
            Some((cw, ch)) => {
                let target_w = (area.width as u32) * (cw.max(1) as u32);
                let target_h = (area.height as u32) * (ch.max(1) as u32);
                image::DynamicImage::ImageRgba8(
                    ctx.image
                        .pixels()
                        .resize_to_fill(
                            target_w.max(1),
                            target_h.max(1),
                            image::imageops::FilterType::Triangle,
                        )
                        .to_rgba8(),
                )
            }
            None => ctx.image.pixels().clone(),
        },
        Resize::Fit(_) | Resize::Scale(_) => ctx.image.pixels().clone(),
    };

    let mut bytes = Vec::with_capacity(64 * 1024);
    dyn_img
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(bytes)
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
    fn resize_kind_round_trip() {
        assert_eq!(
            ResizeKind::from(Resize::Fit(image::imageops::FilterType::Triangle)),
            ResizeKind::Fit
        );
        assert_eq!(
            ResizeKind::from(Resize::Scale(image::imageops::FilterType::Triangle)),
            ResizeKind::Scale
        );
        assert_eq!(
            ResizeKind::from(Resize::Crop(crate::resize::CropAnchor::Center)),
            ResizeKind::Crop
        );
    }
}
