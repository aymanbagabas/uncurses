//! Source crops, destination pixel geometry, and visible cell spans.

use std::io;

use uncurses::layout::{Rect, Size};

use crate::{Crop, Fit, ImageOptions, invalid};

pub(crate) struct Geometry {
    pub source: Crop,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

pub(crate) fn resolve(
    source: (u32, u32),
    cells: Size,
    samples: (u32, u32),
    options: &ImageOptions,
) -> io::Result<Geometry> {
    options.validate_color()?;
    if cells.width == 0
        || cells.height == 0
        || samples.0 == 0
        || samples.1 == 0
        || options.cell_pixels.0 == 0
        || options.cell_pixels.1 == 0
        || !options.scale.is_finite()
        || options.scale <= 0.0
        || options.aspect_ratio.is_some_and(|(w, h)| w == 0 || h == 0)
    {
        return Err(invalid(
            "image dimensions, ratio, cell dimensions, and scale must be positive",
        ));
    }
    let mut crop = options.crop.unwrap_or(Crop {
        x: 0,
        y: 0,
        width: source.0,
        height: source.1,
    });
    if crop.width == 0
        || crop.height == 0
        || crop.x.checked_add(crop.width).is_none_or(|x| x > source.0)
        || crop.y.checked_add(crop.height).is_none_or(|y| y > source.1)
    {
        return Err(invalid("source crop must fit inside the image"));
    }
    let canvas_w = f64::from(cells.width) * f64::from(options.cell_pixels.0);
    let canvas_h = f64::from(cells.height) * f64::from(options.cell_pixels.1);
    let ratio = options
        .aspect_ratio
        .map_or(f64::from(crop.width) / f64::from(crop.height), |(w, h)| {
            f64::from(w) / f64::from(h)
        });
    let (width, height) = match options.fit {
        Fit::Stretch => (canvas_w, canvas_h),
        Fit::Original => (f64::from(crop.height) * ratio, f64::from(crop.height)),
        Fit::Contain | Fit::Cover => {
            let height = if options.fit == Fit::Contain {
                canvas_h.min(canvas_w / ratio)
            } else {
                canvas_h.max(canvas_w / ratio)
            };
            (height * ratio, height)
        }
    };
    let width = width * options.scale;
    let height = height * options.scale;
    if !width.is_finite() || !height.is_finite() {
        return Err(invalid(
            "scaled image dimensions exceed the supported range",
        ));
    }
    if width > canvas_w {
        let kept = (f64::from(crop.width) * canvas_w / width)
            .round()
            .clamp(1.0, f64::from(crop.width)) as u32;
        crop.x += (crop.width - kept) / 2;
        crop.width = kept;
    }
    if height > canvas_h {
        let kept = (f64::from(crop.height) * canvas_h / height)
            .round()
            .clamp(1.0, f64::from(crop.height)) as u32;
        crop.y += (crop.height - kept) / 2;
        crop.height = kept;
    }
    let width = (width.min(canvas_w) / canvas_w * f64::from(samples.0))
        .round()
        .clamp(1.0, f64::from(samples.0)) as u32;
    let height = (height.min(canvas_h) / canvas_h * f64::from(samples.1))
        .round()
        .clamp(1.0, f64::from(samples.1)) as u32;
    Ok(Geometry {
        source: crop,
        x: (samples.0 - width) / 2,
        y: (samples.1 - height) / 2,
        width,
        height,
    })
}

pub(crate) fn coalesce(spans: impl Iterator<Item = Rect>) -> Vec<Rect> {
    let mut rectangles: Vec<Rect> = Vec::new();
    for rect in spans {
        if let Some(previous) = rectangles.iter_mut().rev().find(|previous| {
            previous.x == rect.x && previous.width == rect.width && previous.bottom() == rect.y
        }) {
            previous.height += rect.height;
        } else {
            rectangles.push(rect);
        }
    }
    rectangles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting_uses_physical_cells_at_both_sampling_resolutions() {
        let mut options = ImageOptions {
            cell_pixels: (10, 20),
            ..Default::default()
        };
        let size = Size::new(4, 4);
        for (samples, contained) in [((40, 80), (40, 40)), ((4, 8), (4, 4))] {
            options.fit = Fit::Contain;
            let g = resolve((100, 100), size, samples, &options).unwrap();
            assert_eq!((g.width, g.height), contained);
            assert_eq!((g.source.width, g.source.height), (100, 100));
            options.fit = Fit::Cover;
            let g = resolve((100, 100), size, samples, &options).unwrap();
            assert_eq!((g.width, g.height), samples);
            assert_eq!((g.source.x, g.source.width), (25, 50));
            options.fit = Fit::Stretch;
            let g = resolve((100, 100), size, samples, &options).unwrap();
            assert_eq!((g.width, g.height), samples);
            assert_eq!((g.source.width, g.source.height), (100, 100));
        }
    }

    #[test]
    fn scale_ratio_and_crop_leave_options_unchanged() {
        let options = ImageOptions {
            fit: Fit::Original,
            scale: 2.0,
            aspect_ratio: Some((2, 1)),
            crop: Some(Crop {
                x: 10,
                y: 20,
                width: 10,
                height: 10,
            }),
            cell_pixels: (10, 10),
            ..Default::default()
        };
        let saved = options.clone();
        let g = resolve((40, 40), Size::new(8, 4), (80, 40), &options).unwrap();
        assert_eq!((g.x, g.y, g.width, g.height), (20, 10, 40, 20));
        assert_eq!(options, saved);
    }
}
