//! Source crops and destination pixel geometry.

use std::io;

use uncurses::layout::Size;

use crate::{Crop, Fit, ImageOptions, invalid};

pub(crate) struct Geometry {
    pub source: Crop,
    pub x: i64,
    pub y: i64,
    pub width: u32,
    pub height: u32,
}

pub(crate) fn resolve(
    source: (u32, u32),
    cells: Size,
    samples: (u32, u32),
    options: &ImageOptions,
) -> io::Result<Geometry> {
    resolve_with_canvas(source, cells, samples, samples, options)
}

pub(crate) fn resolve_with_canvas(
    source: (u32, u32),
    cells: Size,
    samples: (u32, u32),
    canvas: (u32, u32),
    options: &ImageOptions,
) -> io::Result<Geometry> {
    options.validate_color()?;
    if cells.width == 0
        || cells.height == 0
        || samples.0 == 0
        || samples.1 == 0
        || canvas.0 == 0
        || canvas.1 == 0
        || !options.scale.is_finite()
        || options.scale <= 0.0
        || options.aspect_ratio.is_some_and(|(w, h)| w == 0 || h == 0)
    {
        return Err(invalid(
            "image dimensions, ratio, cell dimensions, and scale must be positive",
        ));
    }
    let crop = options.crop.unwrap_or(Crop {
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
    let canvas_w = f64::from(canvas.0);
    let canvas_h = f64::from(canvas.1);
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
    let width = (width / canvas_w * f64::from(samples.0)).round().max(1.0) as u32;
    let height = (height / canvas_h * f64::from(samples.1)).round().max(1.0) as u32;
    Ok(Geometry {
        source: crop,
        x: (i64::from(samples.0) - i64::from(width)) / 2,
        y: (i64::from(samples.1) - i64::from(height)) / 2,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting_uses_physical_cells_at_both_sampling_resolutions() {
        let mut options = ImageOptions::default();
        let size = Size::new(4, 4);
        for (samples, contained) in [((40, 80), (40, 40)), ((4, 8), (4, 4))] {
            options.fit = Fit::Contain;
            let g = resolve((100, 100), size, samples, &options).unwrap();
            assert_eq!((g.width, g.height), contained);
            assert_eq!((g.source.width, g.source.height), (100, 100));
            options.fit = Fit::Cover;
            let g = resolve((100, 100), size, samples, &options).unwrap();
            assert_eq!((g.width, g.height), (samples.1, samples.1));
            assert_eq!((g.x, g.y), (-i64::from(samples.0) / 2, 0));
            assert_eq!((g.source.x, g.source.width), (0, 100));
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
            ..Default::default()
        };
        let saved = options.clone();
        let g = resolve((40, 40), Size::new(8, 4), (80, 40), &options).unwrap();
        assert_eq!((g.x, g.y, g.width, g.height), (20, 10, 40, 20));
        assert_eq!(options, saved);
    }
}
