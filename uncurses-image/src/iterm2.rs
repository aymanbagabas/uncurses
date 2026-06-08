//! iTerm2 inline image backend.
//!
//! Encodes an image as a PNG, base64s it, and emits an `OSC 1337
//! File=inline=1;width=Wpx;height=Hpx;preserveAspectRatio=0:<b64>
//! BEL` sequence as the region payload. The screen stamps
//! [`uncurses::cell::Cell::skip`] placeholders over the cell
//! footprint and emits the sequence on every render after the
//! cell diff, so the image bytes paint over the diff's blanks.
//!
//! The image is pre-resized to the exact target pixel size before
//! encoding, so `preserveAspectRatio=0` is the correct hint —
//! aspect-ratio handling lives in [`Resize::Fit`] / [`Resize::Crop`].
//!
//! ## Cache identity
//!
//! [`Self::paint`] hashes the source pixel data to dedupe encode
//! work. The cache is keyed on `(pixel_hash, cell_rect, cell_px,
//! resize)` so a paint whose inputs would produce byte-identical
//! payload bytes reuses the cached encoding. Caching is decoupled
//! from [`RegionId`]: the same image painted at two ids shares
//! the same cache entry.
//!
//! ## Per-cell pixel size
//!
//! The host passes the terminal's cell pixel size to
//! [`Self::paint`]; with `cell_px == (0, 0)` the painter does
//! nothing — the protocol has no meaningful fallback in cell space.

use std::io::{self, Cursor, Write};
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use image::{DynamicImage, GenericImageView, ImageFormat};
use rustc_hash::FxHashMap;
use uncurses::Rect;
use uncurses::screen::{RegionId, Screen};

use crate::hash::pixel_hash;
use crate::layout::{clip_area, footprint, target_pixels};
use crate::painter::Painter;
use crate::resize::Resize;

/// iTerm2 inline-image painter.
///
/// Caches the encoded payload per `(pixel_hash, cell_rect,
/// cell_px, resize)` so repeated paints whose encoded bytes would
/// be identical reuse the cached sequence.
#[derive(Debug, Default)]
pub struct Iterm2 {
    cache: FxHashMap<CacheKey, Arc<[u8]>>,
}

/// Inputs that fully determine the encoded payload bytes for an
/// iTerm2 paint. Two paints whose [`CacheKey`] compare equal
/// produce byte-identical sequences and can share a cache entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CacheKey {
    pixel_hash: u64,
    cell_rect: (u16, u16),
    cell_px: (u16, u16),
    resize: Resize,
}

impl Iterm2 {
    /// Construct a fresh painter with an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop every cached entry. Equivalent to constructing a fresh
    /// painter, but keeps the allocated table for reuse.
    pub fn clear(&mut self) {
        self.cache.clear();
    }
}

impl Painter for Iterm2 {
    /// Encode `image` and register the resulting payload as a
    /// paint region on `screen` under `id`. The screen stamps
    /// Skip placeholders over the footprint and re-emits the
    /// payload after the cell diff each frame.
    ///
    /// Repeat paints with the same `(image pixels, footprint,
    /// cell_px, resize)` reuse the cached encoding. Repeat paints
    /// with the same `id` replace the previous registration;
    /// distinct paint instances must use distinct ids.
    fn paint<W: Write>(
        &mut self,
        screen: &mut Screen<W>,
        id: RegionId,
        area: Rect,
        image: &DynamicImage,
        resize: Resize,
        cell_px: (u16, u16),
    ) -> io::Result<()> {
        let area = clip_area(area, screen);
        if area.width == 0 || area.height == 0 || cell_px.0 == 0 || cell_px.1 == 0 {
            screen.clear_region(id);
            return Ok(());
        }

        let (target_w, target_h) = target_pixels(image.dimensions(), area, cell_px, resize);
        if target_w == 0 || target_h == 0 {
            screen.clear_region(id);
            return Ok(());
        }

        let footprint = footprint(area, cell_px, (target_w, target_h));
        if footprint.width == 0 || footprint.height == 0 {
            screen.clear_region(id);
            return Ok(());
        }

        let key = CacheKey {
            pixel_hash: pixel_hash(image),
            cell_rect: (footprint.width, footprint.height),
            cell_px,
            resize,
        };
        let payload = match self.cache.get(&key) {
            Some(p) => Arc::clone(p),
            None => {
                let bytes = encode_payload(image, (target_w, target_h), resize)?;
                let arc: Arc<[u8]> = bytes.into();
                self.cache.entry(key).or_insert_with(|| Arc::clone(&arc));
                arc
            }
        };

        screen.set_region(id, footprint, payload);
        Ok(())
    }

    /// Drop the screen-side region registration for `id`.
    /// Idempotent and a no-op for unknown ids.
    fn forget<W: Write>(&mut self, screen: &mut Screen<W>, id: RegionId) -> io::Result<()> {
        screen.clear_region(id);
        Ok(())
    }
}

/// Build the OSC 1337 inline-image sequence wrapped in DECSC/DECRC
/// so the cursor returns to the region anchor after emission.
///
/// The screen positions the cursor at the region's origin before
/// writing the payload, so DECSC saves the anchor and DECRC
/// restores it. The renderer's tracked cursor stays at the
/// anchor, in sync with the terminal.
fn encode_payload(image: &DynamicImage, target: (u32, u32), resize: Resize) -> io::Result<Vec<u8>> {
    let (target_w, target_h) = target;
    if target_w == 0 || target_h == 0 {
        return Ok(Vec::new());
    }

    let png = encode_png(image, target, resize)?;
    let b64 = BASE64.encode(&png);

    let header = format!(
        "\x1b]1337;File=inline=1;width={target_w}px;height={target_h}px;preserveAspectRatio=0:"
    );
    let mut out = Vec::with_capacity(2 + header.len() + b64.len() + 1 + 2);
    out.extend_from_slice(b"\x1b7");
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(b64.as_bytes());
    out.push(0x07);
    out.extend_from_slice(b"\x1b8");
    Ok(out)
}

/// Resize `image` to `target` pixels per `resize` and encode the
/// result as a PNG byte stream.
fn encode_png(image: &DynamicImage, target: (u32, u32), resize: Resize) -> io::Result<Vec<u8>> {
    let (target_w, target_h) = target;

    let filter = match resize {
        Resize::Scale(f) | Resize::Fit(f) => f,
        Resize::Crop(_) => image::imageops::FilterType::Triangle,
    };
    let resized = match resize {
        Resize::Crop(_) => {
            DynamicImage::ImageRgba8(image.resize_to_fill(target_w, target_h, filter).to_rgba8())
        }
        Resize::Fit(_) | Resize::Scale(_) => {
            DynamicImage::ImageRgba8(image.resize_exact(target_w, target_h, filter).to_rgba8())
        }
    };

    let mut buf = Vec::new();
    resized
        .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .map_err(io::Error::other)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> DynamicImage {
        let mut buf = RgbaImage::new(w, h);
        for px in buf.pixels_mut() {
            *px = Rgba(rgba);
        }
        DynamicImage::ImageRgba8(buf)
    }

    #[test]
    fn payload_has_decsc_decrc_bookends() {
        let bytes = encode_payload(&solid(8, 8, [10, 20, 30, 255]), (8, 8), Resize::default())
            .expect("encode");
        assert!(bytes.starts_with(b"\x1b7"), "missing DECSC: {bytes:?}");
        assert!(bytes.ends_with(b"\x1b8"), "missing DECRC: {bytes:?}");
    }

    #[test]
    fn payload_is_osc_1337_inline_image_with_pixel_dimensions() {
        let bytes = encode_payload(&solid(8, 8, [10, 20, 30, 255]), (8, 8), Resize::default())
            .expect("encode");
        let body = &bytes[2..bytes.len() - 2];
        let s = std::str::from_utf8(body).expect("ascii header + base64");
        assert!(s.starts_with("\x1b]1337;File=inline=1;"), "header: {s:?}");
        assert!(s.contains("width=8px"));
        assert!(s.contains("height=8px"));
        assert!(s.contains("preserveAspectRatio=0"));
        assert!(s.ends_with('\x07'), "missing BEL terminator: {s:?}");

        let colon = s.find(':').expect("payload separator");
        let bel = s.len() - 1;
        let b64 = &s[colon + 1..bel];
        let png = BASE64.decode(b64).expect("valid base64");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "PNG signature: {png:?}");
    }

    #[test]
    fn empty_target_yields_empty_payload() {
        let bytes = encode_payload(&solid(8, 8, [0, 0, 0, 255]), (0, 0), Resize::default())
            .expect("encode");
        assert!(bytes.is_empty());
    }

    #[test]
    fn paint_caches_identical_inputs() {
        use uncurses::screen::Screen;

        let img = solid(16, 16, [10, 20, 30, 255]);
        let mut painter = Iterm2::new();
        let mut screen = Screen::new(Vec::<u8>::new()).with_size(20, 4);

        let area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        };
        painter
            .paint(
                &mut screen,
                RegionId(1),
                area,
                &img,
                Resize::default(),
                (8, 16),
            )
            .unwrap();
        assert_eq!(painter.cache.len(), 1);

        // Same inputs at a different id must reuse the cache entry.
        painter
            .paint(
                &mut screen,
                RegionId(2),
                area,
                &img,
                Resize::default(),
                (8, 16),
            )
            .unwrap();
        assert_eq!(painter.cache.len(), 1);
    }

    #[test]
    fn paint_distinct_images_produce_distinct_cache_entries() {
        use uncurses::screen::Screen;

        let a = solid(16, 16, [10, 20, 30, 255]);
        let b = solid(16, 16, [40, 50, 60, 255]);
        let mut painter = Iterm2::new();
        let mut screen = Screen::new(Vec::<u8>::new()).with_size(20, 4);

        let area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        };
        painter
            .paint(
                &mut screen,
                RegionId(1),
                area,
                &a,
                Resize::default(),
                (8, 16),
            )
            .unwrap();
        painter
            .paint(
                &mut screen,
                RegionId(2),
                area,
                &b,
                Resize::default(),
                (8, 16),
            )
            .unwrap();
        assert_eq!(painter.cache.len(), 2);
    }

    #[test]
    fn paint_with_zero_cell_px_clears_region() {
        use uncurses::screen::Screen;

        let img = solid(16, 16, [10, 20, 30, 255]);
        let mut painter = Iterm2::new();
        let mut screen = Screen::new(Vec::<u8>::new()).with_size(20, 4);

        painter
            .paint(
                &mut screen,
                RegionId(1),
                Rect {
                    x: 0,
                    y: 0,
                    width: 4,
                    height: 2,
                },
                &img,
                Resize::default(),
                (0, 0),
            )
            .unwrap();
        // No region was registered, no cache entry was inserted.
        assert!(painter.cache.is_empty());
    }
}
