//! Shared geometry helpers for image painters.
//!
//! All raster backends size their output in pixels, then map back
//! to a cell footprint inside the placement [`Rect`]. The math is
//! identical regardless of the wire protocol — only the encoded
//! payload differs — so it lives here.

use std::io::Write;

use uncurses::Rect;
use uncurses::screen::Screen;

use crate::resize::Resize;

/// Clip `area` to `screen`'s current size. Returns a rect whose
/// origin is at most one past the screen's right/bottom edge with
/// width/height saturated at the visible remainder.
pub(crate) fn clip_area<W: Write>(area: Rect, screen: &Screen<W>) -> Rect {
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

/// Compute the pixel dimensions the source image should be resized
/// to in order to fill `area` according to `resize`, given a
/// per-cell pixel size. For [`Resize::Fit`] this preserves aspect
/// ratio and may scale the image up or down so it touches the area
/// on at least one edge.
pub(crate) fn target_pixels(
    src: (u32, u32),
    area: Rect,
    cell_px: (u16, u16),
    resize: Resize,
) -> (u32, u32) {
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
pub(crate) fn footprint(area: Rect, cell_px: (u16, u16), target: (u32, u32)) -> Rect {
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
    use image::imageops::FilterType;

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
            Resize::Scale(FilterType::Triangle),
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
            Resize::Fit(FilterType::Triangle),
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
        assert_eq!(fp.width, 2);
        assert_eq!(fp.height, 2);
        assert_eq!(fp.x, 4);
        assert_eq!(fp.y, 2);
    }
}
