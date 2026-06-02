//! iTerm2 inline image backend (OSC 1337 `File=`).
//!
//! Each placement is encoded as a PNG, base64-wrapped, and emitted
//! after the renderer's frame as an OSC 1337 inline-image sequence:
//!
//! ```text
//! ESC ] 1337 ; File = inline=1 ; width=N ; height=M ; preserveAspectRatio=… : <base64-png> BEL
//! ```
//!
//! When the encoded payload would exceed the per-OSC byte limit
//! (1,048,576 bytes — the cap honored by both the terminal and tmux
//! passthrough), the burst is split across `MultipartFile=` /
//! `FilePart=` / `FileEnd` sequences instead.
//!
//! The placeholder cells are blanked during `reserve` so the
//! renderer's diff wipes any text underneath.
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

        // Position the cursor at the placement origin, emit the OSC
        // 1337 burst, then snap the cursor back via CUU. The
        // surrounding `set_cursor_position` + `invalidate_cursor`
        // pair means the renderer re-asserts the cursor on the next
        // diff regardless of where the terminal actually leaves it.
        //
        // doNotMoveCursor=0 (set in `build_sequence`) forces the
        // terminal to advance the cursor by `area.height` rows after
        // rendering. The explicit CUU(area.height) returns the
        // cursor to the placement origin in sync with the renderer's
        // tracked position. We avoid `doNotMoveCursor=1` because it
        // is iTerm2-specific and not honored by every terminal that
        // implements the OSC 1337 inline-image protocol.
        screen.set_cursor_position(area.x, area.y)?;
        screen.write_all(entry.sequence.as_bytes())?;
        if area.height > 0 {
            uncurses::ansi::cursor::write_cuu(screen, area.height)?;
        }
        screen.invalidate_cursor();
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

/// Maximum size of a single OSC 1337 control sequence, as documented
/// by the inline-image protocol. Sequences longer than this are
/// rejected by the terminal and by tmux passthrough; encoders larger
/// than this must be split using the `MultipartFile` / `FilePart` /
/// `FileEnd` form.
const MAX_OSC_BYTES: usize = 1_048_576;

/// Build the OSC 1337 sequence(s) for one placement, including the
/// base64-encoded PNG payload and the pixel dimensions. Caller has
/// verified `caps.cell_pixel_size.is_some()`.
///
/// The return value is a single byte stream that may concatenate
/// several OSC 1337 sequences when the encoded payload would exceed
/// `MAX_OSC_BYTES` as a single sequence. In that case the multipart
/// form is used: a `MultipartFile=` header, one or more `FilePart=`
/// chunks, then a `FileEnd` terminator.
fn build_sequence(ctx: &PaintCtx<'_>, area: Rect) -> std::io::Result<String> {
    let png_bytes = encode_png(ctx, area)?;
    let (cw, ch) = ctx
        .caps
        .cell_pixel_size
        .expect("paint() guarantees cell_pixel_size is Some");
    let w_px = (area.width as u32) * (cw.max(1) as u32);
    let h_px = (area.height as u32) * (ch.max(1) as u32);

    // The argument list is identical between `File=` (single-shot)
    // and `MultipartFile=` (chunked). Build it once and decide which
    // framing to use after the base64 length is known.
    //
    // doNotMoveCursor=0 keeps the burst's cursor advancement
    // consistent across terminals that don't recognize the flag —
    // those move the cursor as part of normal text flow, which the
    // surrounding CUU (in `paint`) then unwinds.
    //
    // preserveAspectRatio=0 is paired with exact cell-box pixel
    // dimensions: the encoded image has already been resized (and
    // padded/cropped where needed) to those exact dimensions on our
    // side. Letting the terminal also apply aspect-ratio fitting
    // produces inconsistent results across implementers.
    let mut args = String::with_capacity(96);
    args.push_str("inline=1;doNotMoveCursor=0;");
    write!(
        args,
        "size={};width={}px;height={}px;preserveAspectRatio=0",
        png_bytes.len(),
        w_px,
        h_px,
    )
    .expect("write to String");

    // base64 expands by ~4/3; pre-compute the encoded length so we
    // can decide single-shot vs. multipart without growing twice.
    let b64_len = STANDARD.encode(&png_bytes).len();
    // Single-shot framing overhead:
    //   "\x1b]1337;File=" + args + ":" + <base64> + "\x07"
    let single_total = b"\x1b]1337;File=".len() + args.len() + 1 + b64_len + 1;

    let mut out = String::with_capacity(single_total + 64);

    if single_total <= MAX_OSC_BYTES {
        out.push_str("\x1b]1337;File=");
        out.push_str(&args);
        out.push(':');
        STANDARD.encode_string(&png_bytes, &mut out);
        // BEL terminator. ST (`ESC \`) is also accepted; BEL matches
        // the examples in the iTerm2 documentation.
        out.push('\x07');
        return Ok(out);
    }

    // Multipart form. Each sub-sequence (header, every part, the
    // terminator) must fit within `MAX_OSC_BYTES` on its own.
    out.push_str("\x1b]1337;MultipartFile=");
    out.push_str(&args);
    out.push('\x07');

    // Per-part overhead: "\x1b]1337;FilePart=" + chunk + "\x07".
    const FILEPART_PREFIX: &str = "\x1b]1337;FilePart=";
    const FILEPART_OVERHEAD: usize = FILEPART_PREFIX.len() + 1; // + BEL
    let max_chunk = MAX_OSC_BYTES - FILEPART_OVERHEAD;

    // Stream-encode the payload directly into `out` without
    // materialising the full base64 string twice. Each chunk is a
    // multiple of 4 base64 characters so the boundaries fall on
    // whole input groups; that means we encode windows of `3 * (N/4)`
    // input bytes per chunk.
    let chunk_input = (max_chunk / 4) * 3;
    let mut idx = 0;
    while idx < png_bytes.len() {
        let end = (idx + chunk_input).min(png_bytes.len());
        out.push_str(FILEPART_PREFIX);
        STANDARD.encode_string(&png_bytes[idx..end], &mut out);
        out.push('\x07');
        idx = end;
    }

    out.push_str("\x1b]1337;FileEnd\x07");
    Ok(out)
}

/// Encode the source image to PNG at exactly the cell-box pixel
/// dimensions. Each `Resize` mode is implemented host-side so the
/// terminal receives a raster that already matches the target box —
/// the OSC then emits `preserveAspectRatio=0` and the terminal blits
/// the bytes straight into the box.
fn encode_png(ctx: &PaintCtx<'_>, area: Rect) -> std::io::Result<Vec<u8>> {
    let (cw, ch) = ctx
        .caps
        .cell_pixel_size
        .expect("paint() guarantees cell_pixel_size is Some");
    let target_w = ((area.width as u32) * (cw.max(1) as u32)).max(1);
    let target_h = ((area.height as u32) * (ch.max(1) as u32)).max(1);

    let dyn_img = match ctx.placement.resize {
        Resize::Crop(_) => image::DynamicImage::ImageRgba8(
            ctx.image
                .pixels()
                .resize_to_fill(target_w, target_h, image::imageops::FilterType::Triangle)
                .to_rgba8(),
        ),
        Resize::Scale(filter) => image::DynamicImage::ImageRgba8(
            ctx.image
                .pixels()
                .resize_exact(target_w, target_h, filter)
                .to_rgba8(),
        ),
        Resize::Fit(filter) => {
            // Resize-to-fit maintains aspect ratio (image fits inside
            // the box, may be smaller in one dimension). Composite
            // onto a transparent canvas matching the box exactly so
            // the OSC's pixel dimensions and the terminal's cell
            // advance line up perfectly.
            let resized = ctx.image.pixels().resize(target_w, target_h, filter);
            let mut canvas =
                image::RgbaImage::from_pixel(target_w, target_h, image::Rgba([0, 0, 0, 0]));
            let off_x = (target_w.saturating_sub(resized.width())) / 2;
            let off_y = (target_h.saturating_sub(resized.height())) / 2;
            image::imageops::overlay(&mut canvas, &resized, off_x as i64, off_y as i64);
            image::DynamicImage::ImageRgba8(canvas)
        }
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
