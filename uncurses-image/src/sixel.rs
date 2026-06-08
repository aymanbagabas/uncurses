//! Sixel backend.
//!
//! Encodes an image to a DCS sixel sequence and registers an
//! external paint region with the screen. The screen stamps
//! [`uncurses::cell::Cell::skip`] placeholders over the cell
//! footprint and emits the sequence on every render after the
//! cell diff, so the image bytes paint over the diff's blanks.
//!
//! ## Cache identity
//!
//! [`Self::paint`] hashes the source pixel data to dedupe encode
//! work. The cache is keyed on `(pixel_hash, cell_rect, cell_px,
//! resize)` so a paint whose inputs would produce byte-identical
//! DCS bytes reuses the cached encoding. Caching is decoupled
//! from [`RegionId`]: the same image painted at two ids shares
//! the same cache entry.
//!
//! ## Per-cell pixel size
//!
//! Sixel images are sized in pixels. The host passes the
//! terminal's cell pixel size to [`Self::paint`]; with `cell_px ==
//! (0, 0)` the painter does nothing — sixel has no meaningful
//! fallback in cell space.

use std::io::{self, Write};
use std::sync::Arc;

use icy_sixel::SixelImage;
use image::{DynamicImage, GenericImageView};
use rustc_hash::FxHashMap;
use uncurses::Rect;
use uncurses::screen::{RegionId, Screen};

use crate::hash::pixel_hash;
use crate::layout::{clip_area, footprint, target_pixels};
use crate::painter::Painter;
use crate::resize::Resize;

/// Sixel painter.
///
/// Caches the encoded sixel sequence per `(pixel_hash, cell_rect,
/// cell_px, resize)` so repeated paints whose encoded bytes would
/// be identical reuse the cached sequence.
#[derive(Debug, Default)]
pub struct Sixel {
    cache: FxHashMap<CacheKey, Arc<[u8]>>,
}

/// Inputs that fully determine the encoded DCS bytes for a sixel
/// paint. Two paints whose [`CacheKey`] compare equal produce
/// byte-identical sequences and can share a cache entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CacheKey {
    pixel_hash: u64,
    cell_rect: (u16, u16),
    cell_px: (u16, u16),
    resize: Resize,
}

impl Sixel {
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

impl Painter for Sixel {
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

/// Encode the DCS sixel sequence wrapped in DECSC/DECRC so the
/// cursor returns to the region anchor after emission.
///
/// The screen positions the cursor at the region's origin before
/// writing the payload, so DECSC saves the anchor and DECRC
/// restores it. The renderer's tracked cursor stays at the
/// anchor, in sync with the terminal.
fn encode_payload(image: &DynamicImage, target: (u32, u32), resize: Resize) -> io::Result<Vec<u8>> {
    let dcs = encode(image, target, resize)?;
    let mut out = Vec::with_capacity(dcs.len() + 4);
    out.extend_from_slice(b"\x1b7");
    out.extend_from_slice(dcs.as_bytes());
    out.extend_from_slice(b"\x1b8");
    Ok(out)
}

/// Encode `image` resized to `target` pixels, using `resize` to
/// pick filter and aspect strategy.
fn encode(image: &DynamicImage, target: (u32, u32), resize: Resize) -> io::Result<String> {
    let (target_w, target_h) = target;
    if target_w == 0 || target_h == 0 {
        return Ok(String::new());
    }

    let filter = match resize {
        Resize::Scale(f) | Resize::Fit(f) => f,
        Resize::Crop(_) => image::imageops::FilterType::Triangle,
    };
    let resized = match resize {
        Resize::Crop(_) => image.resize_to_fill(target_w, target_h, filter).to_rgba8(),
        Resize::Fit(_) | Resize::Scale(_) => {
            image.resize_exact(target_w, target_h, filter).to_rgba8()
        }
    };

    let (w, h) = (resized.width() as usize, resized.height() as usize);
    let raw = resized.into_raw();
    let sixel = SixelImage::from_rgba(raw, w, h);
    sixel.encode().map_err(|e| io::Error::other(e.to_string()))
}
