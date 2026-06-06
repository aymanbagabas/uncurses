//! Sixel backend.
//!
//! Encodes an image to a DCS sixel sequence and stamps
//! [`uncurses::cell::Cell::skip`] placeholders over its cell
//! footprint. The renderer emits the placeholders as blank spaces
//! and refuses cell-shifting optimizations on rows that contain
//! them, so the painted region stays anchored to the columns the
//! painter chose.
//!
//! ## Cache identity
//!
//! [`Self::paint`] hashes the source pixel data to recognize "same
//! image as last paint" — the host does not supply an identity.
//! The cache is keyed on `(pixel_hash, cell_rect, cell_px,
//! resize)`, so a paint whose inputs would re-encode to the same
//! bytes reuses the cached encoding.
//!
//! ## Per-cell pixel size
//!
//! Sixel images are sized in pixels. The host passes the
//! terminal's cell pixel size to [`Self::paint`]; with `cell_px ==
//! (0, 0)` the painter stamps no footprint and queues no bytes.

use std::io::{self, Write};

use icy_sixel::SixelImage;
use image::{DynamicImage, GenericImageView};
use rustc_hash::FxHashMap;
use uncurses::Rect;
use uncurses::buffer::{Surface, SurfaceMut};
use uncurses::cell::Cell;
use uncurses::screen::Screen;

use crate::hash::pixel_hash;
use crate::painter::{ImageId, Painter};
use crate::resize::Resize;

/// Sixel painter.
///
/// Caches the encoded sixel sequence per
/// `(pixel_hash, cell_rect, cell_px, resize)` so repeated paints
/// whose encoded bytes would be identical reuse the cached
/// sequence. Tracks the most recent cell footprint per image so
/// the next paint can release the cells it previously owned —
/// only cells that are still the placeholder we stamped are
/// cleared, so any glyph the host wrote on top of the image is
/// preserved across paints.
#[derive(Debug, Default)]
pub struct Sixel {
    cache: FxHashMap<CacheKey, String>,
    /// Most recent cell footprint per image hash. The painter
    /// clears the placeholder cells inside this rectangle that are
    /// not covered by the new footprint at the next [`Self::paint`].
    last_footprint: FxHashMap<u64, Rect>,
    /// Sequences queued by [`Self::paint`] and drained by
    /// [`Self::draw`]. Each entry is a self-contained sequence:
    /// save cursor, move to footprint origin, emit DCS, restore
    /// cursor — so the renderer's tracked cursor stays aligned
    /// with the terminal's after the bytes are emitted.
    pending: Vec<u8>,
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
        self.last_footprint.clear();
    }
}

impl Painter for Sixel {
    /// Stamp Skip cells over the image footprint and queue the
    /// encoded sixel sequence for emission.
    ///
    /// On first paint of a given `(image pixels, cell footprint,
    /// cell pixel size, resize strategy)` combination the painter
    /// encodes; subsequent paints with the same combination reuse
    /// the cached encoding. A change to any of those inputs forces
    /// a re-encode because the resulting DCS bytes differ.
    ///
    /// Returns the pixel-content id, which the caller can later
    /// pass to [`Painter::forget`] to drop every cached encoding
    /// for these pixels. When `cell_px == (0, 0)` the painter
    /// stamps nothing and queues nothing — sixel has no meaningful
    /// fallback in cell space.
    fn paint<W: Write>(
        &mut self,
        screen: &mut Screen<W>,
        area: Rect,
        image: &DynamicImage,
        resize: Resize,
        cell_px: (u16, u16),
    ) -> io::Result<ImageId> {
        let id = pixel_hash(image);
        let area = clip_area(area, screen);
        if area.width == 0 || area.height == 0 || cell_px.0 == 0 || cell_px.1 == 0 {
            return Ok(ImageId(id));
        }

        let (target_w, target_h) = target_pixels(image.dimensions(), area, cell_px, resize);
        if target_w == 0 || target_h == 0 {
            return Ok(ImageId(id));
        }

        // Cell footprint of the encoded image, centered inside the
        // requested `area`.
        let footprint = footprint(area, cell_px, (target_w, target_h));
        if footprint.width == 0 || footprint.height == 0 {
            return Ok(ImageId(id));
        }

        let key = CacheKey {
            pixel_hash: id,
            cell_rect: (footprint.width, footprint.height),
            cell_px,
            resize,
        };
        let dcs = match self.cache.get(&key) {
            Some(s) => s.clone(),
            None => {
                let encoded = encode(image, (target_w, target_h), resize)?;
                self.cache.entry(key).or_insert(encoded).clone()
            }
        };

        // Release the previous footprint's placeholder cells that
        // aren't covered by the new one. Cells the host overwrote
        // are no longer placeholders, so they stay as-is — only the
        // cells we still own get blanked.
        if let Some(prev) = self.last_footprint.get(&id).copied() {
            release_unused_placeholders(screen, prev, footprint);
        }

        // Stamp placeholders over the new footprint. The image owns
        // these cells: any prior glyph at one of these columns is
        // cleared by the renderer's diff on the next render(), and
        // the sixel bytes redraw the pixels in the same frame.
        screen.fill_rect(footprint, &Cell::skip());
        self.last_footprint.insert(id, footprint);

        // Save cursor, move to footprint origin, emit DCS, restore
        // cursor. Terminal rows/columns are 1-based.
        let row = footprint.y.saturating_add(1);
        let col = footprint.x.saturating_add(1);
        write!(self.pending, "\x1b7\x1b[{row};{col}H{dcs}\x1b8")?;
        Ok(ImageId(id))
    }

    /// Drain queued sixel bytes into the screen's output buffer.
    fn draw<W: Write>(&mut self, screen: &mut Screen<W>) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        screen.write_all(&self.pending)?;
        self.pending.clear();
        Ok(())
    }

    /// Drop every cached entry whose pixel-content id matches `id`.
    /// `id` is the value returned by a prior [`Painter::paint`].
    /// Sixel has no terminal-side state to release, so `screen` is
    /// unused.
    fn forget<W: Write>(&mut self, _screen: &mut Screen<W>, id: ImageId) -> io::Result<()> {
        let id = id.0;
        self.cache.retain(|key, _| key.pixel_hash != id);
        self.last_footprint.remove(&id);
        Ok(())
    }
}

/// Blank every cell inside `prev` that isn't inside `keep` and is
/// still the placeholder we stamped. Cells the host overwrote with
/// real glyphs are no longer placeholders and stay untouched.
fn release_unused_placeholders<W: Write>(screen: &mut Screen<W>, prev: Rect, keep: Rect) {
    let bx0 = prev.left();
    let by0 = prev.top();
    let bx1 = prev.right();
    let by1 = prev.bottom();
    for y in by0..by1 {
        for x in bx0..bx1 {
            if x >= keep.left() && x < keep.right() && y >= keep.top() && y < keep.bottom() {
                continue;
            }
            let pos = (x, y).into();
            if matches!(screen.cell(pos), Some(c) if c.is_skip()) {
                screen.set_cell(pos, &Cell::BLANK);
            }
        }
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

/// Compute the pixel dimensions the source image should be resized
/// to in order to fill `area` according to `resize`, given a per-cell
/// pixel size. For `Fit` this preserves aspect ratio and may scale
/// the image up or down so it touches the area on at least one edge.
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
            let s = sx.min(sy);
            let w = ((src.0 as f64) * s).round().max(1.0) as u32;
            let h = ((src.1 as f64) * s).round().max(1.0) as u32;
            (w, h)
        }
    }
}

/// Cell footprint of an encoded image of `(target_w, target_h)`
/// pixels, centered inside `area`. Width/height round up to whole
/// cells; the position shifts so the footprint sits in the middle
/// of `area`.
fn footprint(area: Rect, cell_px: (u16, u16), target: (u32, u32)) -> Rect {
    let cw = cell_px.0.max(1) as u32;
    let ch = cell_px.1.max(1) as u32;
    let cells_w = target.0.div_ceil(cw).min(area.width as u32) as u16;
    let cells_h = target.1.div_ceil(ch).min(area.height as u32) as u16;
    let dx = (area.width - cells_w) / 2;
    let dy = (area.height - cells_h) / 2;
    Rect {
        x: area.x + dx,
        y: area.y + dy,
        width: cells_w,
        height: cells_h,
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
            (100, 100),
            area,
            (10, 20),
            Resize::Scale(image::imageops::FilterType::Triangle),
        );
        assert_eq!(px, (40, 40));
    }

    #[test]
    fn target_pixels_fit_preserves_aspect() {
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

    #[test]
    fn footprint_centers_smaller_image_in_area() {
        let area = Rect {
            x: 2,
            y: 1,
            width: 6,
            height: 4,
        };
        let fp = footprint(area, (10, 20), (20, 40));
        // 20px / 10 = 2 cells wide, 40 / 20 = 2 cells tall.
        // Centered in (6, 4): dx = 2, dy = 1.
        assert_eq!(fp.width, 2);
        assert_eq!(fp.height, 2);
        assert_eq!(fp.x, 4);
        assert_eq!(fp.y, 2);
    }
}
