//! Half-block backend.
//!
//! Renders an image using the `▀` (UPPER HALF BLOCK) glyph, where
//! the glyph's foreground is the upper pixel of a 2-pixel-tall row
//! pair and its background is the lower pixel. This packs two image
//! rows into one terminal row and works on any color-capable
//! terminal — no probing, no per-cell pixel size.

use std::io::{self, Write};

use image::{DynamicImage, GenericImageView, Pixel, Rgba, imageops};

use uncurses::Rect;
use uncurses::cell::Cell;
use uncurses::color::Color;
use uncurses::screen::Screen;
use uncurses::style::Style;

use crate::painter::{ImageId, Painter};
use crate::resize::{CropAnchor, Resize};

/// `▀` — Unicode U+2580 UPPER HALF BLOCK.
const UPPER_HALF: &str = "\u{2580}";

/// Stateless half-block image painter.
///
/// Each call to [`Painter::paint`] stamps the cells in `area` from
/// `image`, scaling per `resize`. Cells outside `area` are
/// untouched. The rendered cells are pure narrow text cells, so the
/// renderer's diff handles them like any other content.
#[derive(Debug, Default, Clone, Copy)]
pub struct Halfblocks;

impl Halfblocks {
    /// Construct a fresh painter. Equivalent to `Halfblocks::default()`.
    pub fn new() -> Self {
        Self
    }
}

impl Painter for Halfblocks {
    /// Stamp `image` into `area` of `screen` using half-block cells.
    ///
    /// `area` is clipped to the screen surface. If the clipped area
    /// is empty, this is a no-op. Halfblocks holds no cached state,
    /// so the returned id is always [`ImageId::NONE`].
    fn paint<W: Write>(
        &mut self,
        screen: &mut Screen<W>,
        area: Rect,
        image: &DynamicImage,
        resize: Resize,
    ) -> io::Result<ImageId> {
        let area = clip_area(area, screen);
        if area.width == 0 || area.height == 0 {
            return Ok(ImageId::NONE);
        }

        let resized = resize_to_cells(image, area, resize);
        let img_w = resized.width();
        let img_h = resized.height();

        for cy in 0..area.height {
            let py0 = (cy as u32) * 2;
            for cx in 0..area.width {
                let px = cx as u32;
                let upper = sample(&resized, px, py0, img_w, img_h);
                let lower = sample(&resized, px, py0 + 1, img_w, img_h);
                let cell = make_cell(upper, lower);
                screen.set_cell((area.x + cx, area.y + cy), &cell);
            }
        }
        Ok(ImageId::NONE)
    }

    /// No-op. Halfblocks holds no cached state.
    fn forget<W: Write>(&mut self, _screen: &mut Screen<W>, _id: ImageId) -> io::Result<()> {
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

fn resize_to_cells(src: &DynamicImage, area: Rect, resize: Resize) -> image::RgbaImage {
    let target_w = area.width as u32;
    let target_h = (area.height as u32) * 2;
    if target_w == 0 || target_h == 0 {
        return image::RgbaImage::new(target_w.max(1), target_h.max(1));
    }

    match resize {
        Resize::Scale(filter) => src.resize_exact(target_w, target_h, filter).to_rgba8(),
        Resize::Fit(filter) => {
            // Preserve aspect; pad to (target_w, target_h) with zero
            // alpha so the unfilled cells render as transparent /
            // default-bg blanks.
            let fit = src.resize(target_w, target_h, filter).to_rgba8();
            let mut canvas = image::RgbaImage::new(target_w, target_h);
            let dx = (target_w.saturating_sub(fit.width())) / 2;
            let dy = (target_h.saturating_sub(fit.height())) / 2;
            imageops::overlay(&mut canvas, &fit, dx as i64, dy as i64);
            canvas
        }
        Resize::Crop(anchor) => {
            if matches!(anchor, CropAnchor::Center) {
                src.resize_to_fill(target_w, target_h, image::imageops::FilterType::Triangle)
                    .to_rgba8()
            } else {
                crop_with_anchor(src, target_w, target_h, anchor)
            }
        }
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

fn sample(img: &image::RgbaImage, x: u32, y: u32, w: u32, h: u32) -> Rgba<u8> {
    if x >= w || y >= h {
        return Rgba([0, 0, 0, 0]);
    }
    *img.get_pixel(x, y)
}

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
