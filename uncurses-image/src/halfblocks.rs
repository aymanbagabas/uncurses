//! Half-block ANSI cell painter.
//!
//! Renders an image into actual terminal cells using the U+2580
//! UPPER HALF BLOCK (▀) glyph. Each cell encodes two vertical
//! source pixels: the top pixel as the cell's foreground color
//! and the bottom pixel as the cell's background. The result is a
//! coarse but graphics-protocol-free fallback that works on any
//! color-capable terminal.
//!
//! Unlike the raster backends, this painter does not paint pixels
//! outside the cell grid. Cells are written directly via
//! [`Screen::set_cell`] and travel through the renderer's cell
//! diff like any other glyph. The host's `cell_px` argument is
//! ignored: the painter sizes the source to one image pixel per
//! column and two image pixels per row.
//!
//! The painter remembers the footprint of every active paint so
//! [`Painter::paint`] can blank cells the previous footprint owned
//! that aren't covered by the new one, and so [`Painter::forget`]
//! can clean up cells when the host stops painting an id.

use std::io::{self, Write};

use image::{DynamicImage, GenericImageView, Pixel, Rgba};
use rustc_hash::FxHashMap;
use uncurses::Rect;
use uncurses::cell::Cell;
use uncurses::color::Color;
use uncurses::screen::{RegionId, Screen};
use uncurses::style::Style;

use crate::layout::clip_area;
use crate::painter::Painter;
use crate::resize::{CropAnchor, Resize};

/// U+2580 UPPER HALF BLOCK.
const UPPER_HALF: &str = "\u{2580}";

/// Half-block painter.
///
/// Tracks the footprint last painted under each id so subsequent
/// [`Painter::paint`] / [`Painter::forget`] calls can release
/// cells the new footprint no longer covers.
#[derive(Debug, Default)]
pub struct HalfBlocks {
    footprints: FxHashMap<RegionId, Rect>,
}

impl HalfBlocks {
    /// Construct a fresh painter with no tracked footprints.
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop every tracked footprint without touching the screen.
    /// Use after a screen resize when the host has already cleared
    /// the affected cells through other means.
    pub fn clear(&mut self) {
        self.footprints.clear();
    }
}

impl Painter for HalfBlocks {
    /// Stamp half-block cells into `area` of `screen`. Cells the
    /// previous paint of `id` covered that aren't covered by the
    /// new footprint are released back to blank.
    ///
    /// The `cell_px` argument is ignored: half-blocks always work
    /// at one image pixel per column and two per row.
    fn paint<W: Write>(
        &mut self,
        screen: &mut Screen<W>,
        id: RegionId,
        area: Rect,
        image: &DynamicImage,
        resize: Resize,
        _cell_px: (u16, u16),
    ) -> io::Result<()> {
        let area = clip_area(area, screen);
        if area.width == 0 || area.height == 0 {
            self.forget(screen, id)?;
            return Ok(());
        }

        let resized = resize_to_cells(image, area, resize);
        for cy in 0..area.height {
            let py = (cy as u32) * 2;
            for cx in 0..area.width {
                let upper = sample(&resized, cx as u32, py);
                let lower = sample(&resized, cx as u32, py + 1);
                screen.set_cell((area.x + cx, area.y + cy), &make_cell(upper, lower));
            }
        }

        if let Some(prev) = self.footprints.insert(id, area) {
            blank_difference(screen, prev, area);
        }
        Ok(())
    }

    /// Blank every cell painted under `id` and drop the tracked
    /// footprint. Idempotent and a no-op for unknown ids.
    fn forget<W: Write>(&mut self, screen: &mut Screen<W>, id: RegionId) -> io::Result<()> {
        if let Some(prev) = self.footprints.remove(&id) {
            blank_rect(screen, prev);
        }
        Ok(())
    }
}

/// Resize `src` to exactly `area.width × (area.height * 2)` pixels
/// according to `resize`. One image pixel per column, two per row.
fn resize_to_cells(src: &DynamicImage, area: Rect, resize: Resize) -> image::RgbaImage {
    let target_w = area.width as u32;
    let target_h = (area.height as u32) * 2;
    if target_w == 0 || target_h == 0 {
        return image::RgbaImage::new(target_w.max(1), target_h.max(1));
    }

    match resize {
        Resize::Scale(filter) => src.resize_exact(target_w, target_h, filter).to_rgba8(),
        Resize::Fit(filter) => {
            let fit = src.resize(target_w, target_h, filter).to_rgba8();
            let mut canvas = image::RgbaImage::new(target_w, target_h);
            let dx = (target_w.saturating_sub(fit.width())) / 2;
            let dy = (target_h.saturating_sub(fit.height())) / 2;
            image::imageops::overlay(&mut canvas, &fit, dx as i64, dy as i64);
            canvas
        }
        Resize::Crop(anchor) => crop_with_anchor(src, target_w, target_h, anchor),
    }
}

fn crop_with_anchor(
    src: &DynamicImage,
    target_w: u32,
    target_h: u32,
    anchor: CropAnchor,
) -> image::RgbaImage {
    let (sw, sh) = src.dimensions();
    if sw == 0 || sh == 0 {
        return image::RgbaImage::new(target_w, target_h);
    }
    let scale = (target_w as f64 / sw as f64).max(target_h as f64 / sh as f64);
    let scaled_w = ((sw as f64) * scale).round().max(target_w as f64) as u32;
    let scaled_h = ((sh as f64) * scale).round().max(target_h as f64) as u32;
    let scaled = src
        .resize_exact(scaled_w, scaled_h, image::imageops::FilterType::Triangle)
        .to_rgba8();

    let (dx, dy) = match anchor {
        CropAnchor::TopLeft => (0, 0),
        CropAnchor::TopRight => (scaled_w.saturating_sub(target_w), 0),
        CropAnchor::BottomLeft => (0, scaled_h.saturating_sub(target_h)),
        CropAnchor::BottomRight => (
            scaled_w.saturating_sub(target_w),
            scaled_h.saturating_sub(target_h),
        ),
        CropAnchor::Center => (
            scaled_w.saturating_sub(target_w) / 2,
            scaled_h.saturating_sub(target_h) / 2,
        ),
    };

    image::imageops::crop_imm(&scaled, dx, dy, target_w, target_h).to_image()
}

fn sample(img: &image::RgbaImage, x: u32, y: u32) -> Rgba<u8> {
    if x >= img.width() || y >= img.height() {
        return Rgba([0, 0, 0, 0]);
    }
    *img.get_pixel(x, y)
}

/// Build the styled cell that paints `upper` as foreground and
/// `lower` as background of the upper-half-block glyph. Fully
/// transparent pixels are encoded as "no color" so the cell
/// composites with whatever was painted there before.
fn make_cell(upper: Rgba<u8>, lower: Rgba<u8>) -> Cell {
    let mut style = Style::EMPTY;
    if upper.0[3] > 0 {
        let [r, g, b, _] = upper.channels().try_into().unwrap_or([0, 0, 0, 0]);
        style = style.with_fg(Color::Rgb(r, g, b));
    }
    if lower.0[3] > 0 {
        let [r, g, b, _] = lower.channels().try_into().unwrap_or([0, 0, 0, 0]);
        style = style.with_bg(Color::Rgb(r, g, b));
    }
    Cell::narrow(UPPER_HALF).with_style(style)
}

/// Blank every cell of `rect` in `screen`'s front buffer.
fn blank_rect<W: Write>(screen: &mut Screen<W>, rect: Rect) {
    for cy in 0..rect.height {
        for cx in 0..rect.width {
            screen.set_cell((rect.x + cx, rect.y + cy), &Cell::BLANK);
        }
    }
}

/// Blank every cell that was inside `prev` but isn't inside `next`.
fn blank_difference<W: Write>(screen: &mut Screen<W>, prev: Rect, next: Rect) {
    let prev_x1 = prev.x.saturating_add(prev.width);
    let prev_y1 = prev.y.saturating_add(prev.height);
    let next_x1 = next.x.saturating_add(next.width);
    let next_y1 = next.y.saturating_add(next.height);

    for cy in prev.y..prev_y1 {
        for cx in prev.x..prev_x1 {
            let inside_next = cx >= next.x && cx < next_x1 && cy >= next.y && cy < next_y1;
            if !inside_next {
                screen.set_cell((cx, cy), &Cell::BLANK);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> DynamicImage {
        let mut buf = RgbaImage::new(w, h);
        for px in buf.pixels_mut() {
            *px = Rgba(rgba);
        }
        DynamicImage::ImageRgba8(buf)
    }

    fn collect_cells<W: Write>(screen: &mut Screen<W>, area: Rect) -> Vec<(u16, u16, Cell)> {
        let mut out = Vec::new();
        for cy in 0..area.height {
            for cx in 0..area.width {
                let cell = screen
                    .cell_mut((area.x + cx, area.y + cy))
                    .cloned()
                    .unwrap_or(Cell::BLANK);
                out.push((area.x + cx, area.y + cy, cell));
            }
        }
        out
    }

    #[test]
    fn paint_stamps_half_block_glyph_into_every_cell() {
        let img = solid(8, 8, [10, 20, 30, 255]);
        let mut painter = HalfBlocks::new();
        let mut screen = Screen::new(Vec::<u8>::new()).with_size(20, 4);
        let area = Rect {
            x: 1,
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
                (0, 0),
            )
            .unwrap();

        for (_, _, cell) in collect_cells(&mut screen, area) {
            assert_eq!(cell.content(), UPPER_HALF, "cell content: {cell:?}");
        }
    }

    #[test]
    fn paint_releases_cells_outside_new_footprint() {
        let img = solid(8, 8, [10, 20, 30, 255]);
        let mut painter = HalfBlocks::new();
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
        painter
            .paint(
                &mut screen,
                RegionId(1),
                Rect {
                    x: 1,
                    y: 0,
                    width: 4,
                    height: 2,
                },
                &img,
                Resize::default(),
                (0, 0),
            )
            .unwrap();

        assert!(screen.cell_mut((0u16, 0u16)).unwrap().is_blank());
        assert!(screen.cell_mut((0u16, 1u16)).unwrap().is_blank());
        assert_eq!(screen.cell_mut((1u16, 0u16)).unwrap().content(), UPPER_HALF);
    }

    #[test]
    fn forget_blanks_painted_cells() {
        let img = solid(8, 8, [10, 20, 30, 255]);
        let mut painter = HalfBlocks::new();
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
                (0, 0),
            )
            .unwrap();
        painter.forget(&mut screen, RegionId(1)).unwrap();

        for (_, _, cell) in collect_cells(&mut screen, area) {
            assert!(cell.is_blank(), "cell after forget: {cell:?}");
        }
        assert!(painter.footprints.is_empty());
    }

    #[test]
    fn forget_unknown_id_is_no_op() {
        let mut painter = HalfBlocks::new();
        let mut screen = Screen::new(Vec::<u8>::new()).with_size(20, 4);
        painter.forget(&mut screen, RegionId(42)).unwrap();
    }

    #[test]
    fn empty_area_clears_tracked_footprint() {
        let img = solid(8, 8, [10, 20, 30, 255]);
        let mut painter = HalfBlocks::new();
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
        painter
            .paint(
                &mut screen,
                RegionId(1),
                Rect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
                &img,
                Resize::default(),
                (0, 0),
            )
            .unwrap();
        assert!(painter.footprints.is_empty());
    }

    #[test]
    fn transparent_pixels_yield_no_color() {
        let img = solid(2, 2, [10, 20, 30, 0]);
        let mut painter = HalfBlocks::new();
        let mut screen = Screen::new(Vec::<u8>::new()).with_size(4, 2);
        painter
            .paint(
                &mut screen,
                RegionId(1),
                Rect {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 1,
                },
                &img,
                Resize::default(),
                (0, 0),
            )
            .unwrap();
        let cell = screen.cell_mut((0u16, 0u16)).unwrap();
        assert!(cell.style().fg().is_none());
        assert!(cell.style().bg().is_none());
    }
}
