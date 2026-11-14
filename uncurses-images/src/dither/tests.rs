use super::*;
use crate::{ImageOptions, quant};
use std::sync::Arc;

const PALETTE: [[u8; 3]; 2] = [[0; 3], [255; 3]];

#[test]
fn defaults_and_value_equality() {
    let options = ImageOptions::default();
    assert_eq!(options.max_colors, 256);
    assert_eq!(options, ImageOptions::default());
    assert!(options.quantizer.same(&quant::Wu));
    assert!(options.dither.as_ref().unwrap().same(&FloydSteinberg {
        strength: 0.875,
        grain: (1, 1),
    }));
    let other = ImageOptions {
        dither: None,
        ..options.clone()
    };
    assert_ne!(options, other);
}

#[test]
fn algorithms_cover_partial_grains_and_preserve_input() {
    let pixels = RgbImage::from_fn(19, 13, |x, y| {
        image::Rgb([(x * 11) as u8, (y * 17) as u8, 128])
    });
    let before = pixels.clone();
    for w in [1, 2, 4, 8] {
        for h in [1, 2, 4, 8] {
            let algorithms: [Arc<dyn Dither>; 3] = [
                Arc::new(Bayer8x8 {
                    grain: (w, h),
                    ..Default::default()
                }),
                Arc::new(Noise {
                    grain: (w, h),
                    ..Default::default()
                }),
                Arc::new(FloydSteinberg {
                    grain: (w, h),
                    ..Default::default()
                }),
            ];
            for algorithm in algorithms {
                let context = DitherContext::new(&pixels, &PALETTE);
                let result = map(algorithm.as_ref(), context).unwrap();
                assert_eq!(result.len(), 19 * 13);
                assert!(result.iter().all(|&i| i < 2));
                assert_eq!(result, map(algorithm.as_ref(), context).unwrap());
                assert_eq!(pixels, before);
            }
        }
    }
}

#[test]
fn zero_strength_maps_nearest_and_extreme_settings_are_checked() {
    let pixels = RgbImage::from_pixel(19, 13, image::Rgb([128; 3]));
    let context = DitherContext::new(&pixels, &PALETTE);
    for algorithm in [
        &Bayer8x8 {
            intensity: 0.0,
            grain: (2, 4),
        } as &dyn Dither,
        &Noise {
            intensity: 0.0,
            grain: (2, 4),
        },
        &FloydSteinberg {
            strength: 0.0,
            grain: (2, 4),
        },
    ] {
        assert_eq!(map(algorithm, context).unwrap(), context.nearest().unwrap());
    }
    for intensity in [-1.0, f32::NAN, f32::INFINITY] {
        assert!(
            Bayer8x8 {
                intensity,
                ..Default::default()
            }
            .dither(context)
            .is_err()
        );
        assert!(
            Noise {
                intensity,
                ..Default::default()
            }
            .dither(context)
            .is_err()
        );
        assert!(
            FloydSteinberg {
                strength: intensity,
                ..Default::default()
            }
            .dither(context)
            .is_err()
        );
    }
    assert!(
        FloydSteinberg {
            strength: 2.0,
            ..Default::default()
        }
        .dither(context)
        .is_err()
    );
    for grain in [(0, 1), (1, 3), (16, 8)] {
        assert!(
            Bayer8x8 {
                grain,
                ..Default::default()
            }
            .dither(context)
            .is_err()
        );
        assert!(
            FloydSteinberg {
                grain,
                ..Default::default()
            }
            .dither(context)
            .is_err()
        );
    }
    assert!(
        Noise {
            intensity: f32::MAX,
            ..Default::default()
        }
        .dither(context)
        .is_ok()
    );
}

#[test]
fn custom_handles_share_context_validation_and_errors() {
    #[derive(Debug)]
    struct Custom(Vec<u8>);
    impl Dither for Custom {
        fn dither(&self, context: DitherContext<'_>) -> io::Result<Vec<u8>> {
            assert_eq!(context.pixels.dimensions(), (2, 2));
            assert_eq!(context.palette, PALETTE);
            Ok(self.0.clone())
        }
    }
    let pixels = RgbImage::from_pixel(2, 2, image::Rgb([128; 3]));
    let context = DitherContext::new(&pixels, &PALETTE);
    for output in [vec![0; 3], vec![0; 5], vec![2; 4]] {
        assert_eq!(
            map(&Custom(output), context).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
    let options = ImageOptions {
        dither: Some(Arc::new(Custom(vec![0; 4]))),
        ..Default::default()
    };
    assert_eq!(options, options.clone());
    assert_ne!(
        options,
        ImageOptions {
            dither: Some(Arc::new(Custom(vec![0; 4]))),
            ..Default::default()
        }
    );
    #[derive(Debug)]
    struct Fails;
    impl Dither for Fails {
        fn dither(&self, _: DitherContext<'_>) -> io::Result<Vec<u8>> {
            Err(io::Error::other("custom failure"))
        }
    }
    assert_eq!(
        map(&Fails, context).unwrap_err().to_string(),
        "custom failure"
    );
    for palette in [vec![], vec![[0; 3]; 257]] {
        assert_eq!(
            map(&Fails, DitherContext::new(&pixels, &palette))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }
}

#[test]
fn floyd_steinberg_matches_quantette_in_oklab() {
    let pixels = RgbImage::from_fn(19, 13, |x, y| {
        image::Rgb([(x * 11) as u8, (y * 17) as u8, 128])
    });
    let colors = color::colors(&pixels);
    let input = quantette::ImageRef::new(19, 13, &colors).unwrap();
    let (_, expected) = quantette::dither::FloydSteinberg::default()
        .dither_image_to_indexed(input, color::color_map(&PALETTE).unwrap())
        .into_parts();
    assert_eq!(
        FloydSteinberg::default()
            .dither(DitherContext::new(&pixels, &PALETTE))
            .unwrap(),
        expected
    );
}
