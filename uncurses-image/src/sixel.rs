//! Sixel backend.
//!
//! Encodes an image to a DCS sixel sequence and stamps it as a
//! single rect-anchored cell. The renderer emits the anchor's bytes
//! verbatim and skips every body cell, so the painted region never
//! interferes with surrounding text the differ owns.
//!
//! ## Cache identity
//!
//! [`Self::paint`] hashes the source pixel data to recognize "same
//! image as last paint" — the host does not supply an identity.
//! The cache is keyed on `(pixel_hash, cell_rect)`, so painting the
//! same image into the same cell footprint reuses the previously
//! encoded bytes; changing either the pixels or the footprint
//! re-encodes.
//!
//! ## Per-cell pixel size
//!
//! Sixel images are sized in pixels. The painter consults
//! [`uncurses::screen::Screen::cell_pixel_size`] to translate
//! `area` (in cells) into the target pixel rectangle. When the
//! cache is unset, [`Self::paint`] returns without writing — sixel
//! has no meaningful fallback in cell space.

use std::io::{self, Write};

use icy_sixel::SixelImage;
use image::{DynamicImage, GenericImageView};
use rustc_hash::FxHashMap;
use uncurses::Rect;
use uncurses::screen::Screen;
use uncurses::style::Style;

use crate::hash::pixel_hash;
use crate::painter::{ImageId, Painter};
use crate::resize::Resize;

/// Sixel painter.
///
/// Caches the encoded sixel sequence per
/// `(pixel_hash, cell_rect, cell_pixel_size, resize)` so repeated
/// paints whose encoded bytes would be identical reuse the
/// previously encoded sequence. Stateless beyond the cache.
#[derive(Debug, Default)]
pub struct Sixel {
    cache: FxHashMap<CacheKey, String>,
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
    /// Stamp `image` into `area` of `screen`, encoding the image as
    /// a sixel DCS sequence and storing it as a single rect-anchored
    /// cell at `(area.x, area.y)`.
    ///
    /// On first paint of a given `(image pixels, cell footprint,
    /// cell pixel size, resize strategy)` combination the painter
    /// encodes; subsequent paints with the same combination reuse
    /// the cached encoding. A change to any of those inputs forces
    /// a re-encode because the resulting DCS bytes differ.
    ///
    /// Returns the pixel-content id, which the caller can later
    /// pass to [`Painter::forget`] to drop every cached encoding
    /// for these pixels. Returns I/O errors from sequence assembly.
    /// When the screen's cell pixel size is unknown, this still
    /// returns the id but does not stamp anything.
    fn paint<W: Write>(
        &mut self,
        screen: &mut Screen<W>,
        area: Rect,
        image: &DynamicImage,
        resize: Resize,
    ) -> io::Result<ImageId> {
        let id = pixel_hash(image);
        let area = clip_area(area, screen);
        if area.width == 0 || area.height == 0 {
            return Ok(ImageId(id));
        }
        let Some((cw, ch)) = screen.cell_pixel_size() else {
            return Ok(ImageId(id));
        };

        let key = CacheKey {
            pixel_hash: id,
            cell_rect: (area.width, area.height),
            cell_px: (cw, ch),
            resize,
        };
        let sequence = match self.cache.get(&key) {
            Some(s) => s.clone(),
            None => {
                let encoded = encode(image, area, (cw, ch), resize)?;
                self.cache.entry(key).or_insert(encoded).clone()
            }
        };

        stamp(screen, area, sequence);
        Ok(ImageId(id))
    }

    /// Drop every cached entry whose pixel-content id matches `id`.
    /// `id` is the value returned by a prior [`Painter::paint`].
    /// Sixel has no terminal-side state to release, so `screen` is
    /// unused.
    fn forget<W: Write>(&mut self, _screen: &mut Screen<W>, id: ImageId) -> io::Result<()> {
        let id = id.0;
        self.cache.retain(|key, _| key.pixel_hash != id);
        Ok(())
    }
}

fn clip_area<W: Write>(area: Rect, screen: &Screen<W>) -> Rect {
    let sw = screen.width();
    let sh = screen.height();
    let x = area.x.min(sw);
    let y = area.y.min(sh);
    let width = area.width.min(sw.saturating_sub(x));
    let height = area.height.min(sh.saturating_sub(y));
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// Resize `image` to the target pixel dimensions implied by `area`,
/// `cell_px`, and `resize`, then encode it to sixel. The returned
/// string is the complete DCS sequence (`\x1bPq…\x1b\\`) emitted by
/// the encoder; callers stamp it verbatim.
fn encode(
    image: &DynamicImage,
    area: Rect,
    cell_px: (u16, u16),
    resize: Resize,
) -> io::Result<String> {
    let (target_w, target_h) = target_pixels(image.dimensions(), area, cell_px, resize);
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

/// Compute the pixel dimensions the source image should be resized
/// to in order to fill `area` according to `resize`, given a per-cell
/// pixel size. For `Fit` this preserves aspect ratio (the resulting
/// image may be smaller than the area in one dimension), avoiding
/// the need for transparent padding which sixel cannot represent.
fn target_pixels(src: (u32, u32), area: Rect, cell_px: (u16, u16), resize: Resize) -> (u32, u32) {
    let cw = cell_px.0.max(1) as u32;
    let ch = cell_px.1.max(1) as u32;
    let area_w = (area.width as u32) * cw;
    let area_h = (area.height as u32) * ch;
    if area_w == 0 || area_h == 0 || src.0 == 0 || src.1 == 0 {
        return (area_w, area_h);
    }
    match resize {
        Resize::Scale(_) | Resize::Crop(_) => (area_w, area_h),
        Resize::Fit(_) => {
            let sx = area_w as f64 / src.0 as f64;
            let sy = area_h as f64 / src.1 as f64;
            let s = sx.min(sy).min(1.0);
            let w = ((src.0 as f64) * s).round().max(1.0) as u32;
            let h = ((src.1 as f64) * s).round().max(1.0) as u32;
            (w, h)
        }
    }
}

fn stamp<W: Write>(screen: &mut Screen<W>, area: Rect, sequence: String) {
    screen.set_rect_payload(area, sequence, Style::EMPTY);
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
            (100, 100),
            area,
            (10, 20),
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
            (200, 100),
            area,
            (10, 20),
            Resize::Fit(image::imageops::FilterType::Triangle),
        );
        assert_eq!(px, (40, 20));
    }
}
