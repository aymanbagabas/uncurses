use super::*;
use std::sync::Arc;

#[test]
fn output_requests_retain_only_the_required_data() {
    for exact_colors in [true, false] {
        let pixels = RgbImage::from_fn(37, 19, |x, y| {
            image::Rgb(if exact_colors {
                if x % 2 == 0 { [0; 3] } else { [255; 3] }
            } else {
                [(x * 7) as u8, (y * 13) as u8, ((x + y) * 4) as u8]
            })
        });
        let context = QuantizeContext {
            pixels: &pixels,
            max_colors: 2,
        };
        for settings in [None, Some(KMeans::default().settings().unwrap())] {
            let palette = builtin(context, settings, Output::Palette).unwrap();
            assert!(palette.indices.is_empty());
            assert!(palette.lookup.is_none());
            assert!(palette.colors.is_empty());
            let indexed = builtin(context, settings, Output::Indexed).unwrap();
            assert_eq!(indexed.indices.len(), 37 * 19);
            assert!(indexed.lookup.is_none());
            assert!(indexed.colors.is_empty());
            let prepared = builtin(context, settings, Output::Prepared).unwrap();
            if exact_colors {
                assert_eq!(prepared.indices.len(), 37 * 19);
                assert!(prepared.lookup.is_none());
                assert!(prepared.colors.is_empty());
            } else {
                assert!(prepared.indices.is_empty());
                assert!(prepared.lookup.is_some());
                assert_eq!(prepared.colors.len(), 37 * 19);
            }
        }
    }
}

#[test]
fn shared_core_preserves_quantette_deduplication_and_native_indices() {
    use quantette::{Pipeline, QuantizeMethod};
    let large = 2048 * 2048;
    let kmeans_threshold = dedup_threshold(Output::Indexed, true, 256);
    for (algorithm, sizes) in [
        (&Wu as &dyn Quantize, vec![large - 1, large]),
        (
            &KMeans {
                sampling_factor: 0.0,
                seed: 17,
            },
            vec![kmeans_threshold - 1, kmeans_threshold, large],
        ),
    ] {
        let method = if algorithm.same(&Wu) {
            QuantizeMethod::Wu
        } else {
            KMeans {
                sampling_factor: 0.0,
                seed: 17,
            }
            .settings()
            .unwrap()
            .into()
        };
        for count in sizes {
            let pixels = RgbImage::from_fn(count as u32, 1, |x, _| {
                let i = x % 1021;
                image::Rgb([i as u8, (i >> 2) as u8, (i * 73) as u8])
            });
            let context = QuantizeContext {
                pixels: &pixels,
                max_colors: 256,
            };
            let rgb = color::rgb_colors(&pixels);
            let input = ImageRef::new(count as u32, 1, &rgb).unwrap();
            let reference = || {
                Pipeline::new()
                    .ditherer(None)
                    .quantize_method(method.clone())
                    .input_image(input)
            };
            let expected_palette = reference().output_srgb8_palette().unwrap();
            assert_eq!(
                algorithm.palette(context).unwrap(),
                expected_palette
                    .iter()
                    .map(|c| [c.red, c.green, c.blue])
                    .collect::<Vec<_>>(),
                "palette: {algorithm:?}, {count} pixels"
            );
            let expected = reference().output_srgb8_indexed_image();
            let actual = algorithm.quantize(context).unwrap();
            assert_eq!(
                actual.palette,
                expected
                    .palette()
                    .iter()
                    .map(|c| [c.red, c.green, c.blue])
                    .collect::<Vec<_>>(),
                "indexed palette: {algorithm:?}, {count} pixels"
            );
            assert_eq!(
                actual.indices,
                expected.indices(),
                "{algorithm:?}, {count} pixels"
            );
        }
    }
}

#[test]
fn prepared_output_keeps_full_pixels_above_the_indexed_threshold() {
    let count = dedup_threshold(Output::Indexed, true, 256);
    let pixels = RgbImage::from_fn(count as u32, 1, |x, _| {
        image::Rgb([x as u8, (x >> 8) as u8, (x * 19) as u8])
    });
    for settings in [
        None,
        Some(
            KMeans {
                sampling_factor: 0.0,
                seed: 9,
            }
            .settings()
            .unwrap(),
        ),
    ] {
        let result = builtin(
            QuantizeContext {
                pixels: &pixels,
                max_colors: 256,
            },
            settings,
            Output::Prepared,
        )
        .unwrap();
        assert_eq!(result.colors, color::colors(&pixels));
        assert!(result.indices.is_empty());
        assert!(result.lookup.is_some());
    }
}

#[test]
fn quantizers_honor_budgets_and_repeatable_indices() {
    let pixels = RgbImage::from_fn(37, 19, |x, y| {
        image::Rgb([(x * 7) as u8, (y * 13) as u8, ((x + y) * 4) as u8])
    });
    for quantizer in [&Wu as &dyn Quantize, &KMeans::default()] {
        for max_colors in [1, 2, 16, 256] {
            let context = QuantizeContext {
                pixels: &pixels,
                max_colors,
            };
            let indexed = quantizer.quantize(context).unwrap();
            indexed.validate(context).unwrap();
            assert_eq!(indexed, quantizer.quantize(context).unwrap());
            assert_eq!(indexed.palette, quantizer.palette(context).unwrap());
        }
    }
}

#[test]
fn exact_runs_and_color_257_preserve_the_full_image() {
    let pixels = RgbImage::from_fn(512, 1, |x, _| image::Rgb([(x / 2) as u8; 3]));
    let options = ImageOptions {
        dither: None,
        ..Default::default()
    };
    let indexed = sixel(&pixels, &options).unwrap();
    assert_eq!(indexed.palette.len(), 256);
    assert_eq!(
        indexed.indices,
        (0..512).map(|x| (x / 2) as u8).collect::<Vec<_>>()
    );
    let pixels = RgbImage::from_fn(514, 1, |x, _| {
        image::Rgb(if x < 512 {
            [(x / 2) as u8; 3]
        } else {
            [0, 0, 1]
        })
    });
    let result = sixel(&pixels, &options).unwrap();
    result
        .validate(QuantizeContext {
            pixels: &pixels,
            max_colors: 256,
        })
        .unwrap();
}

#[test]
fn custom_palette_and_indexed_paths_are_selected_and_validated() {
    #[derive(Debug)]
    struct Custom {
        invalid: bool,
    }
    impl Quantize for Custom {
        fn palette(&self, context: QuantizeContext<'_>) -> io::Result<Vec<[u8; 3]>> {
            assert_eq!(context.max_colors, 2);
            Ok(if self.invalid {
                vec![[0; 3]; 3]
            } else {
                vec![[0; 3], [255; 3]]
            })
        }
        fn quantize(&self, context: QuantizeContext<'_>) -> io::Result<IndexedImage> {
            Ok(IndexedImage {
                width: context.pixels.width(),
                height: context.pixels.height(),
                palette: vec![[0; 3]],
                indices: vec![u8::from(self.invalid); context.pixels.as_raw().len() / 3],
            })
        }
    }
    let pixels = RgbImage::from_pixel(3, 2, image::Rgb([255; 3]));
    let mut options = ImageOptions {
        quantizer: Arc::new(Custom { invalid: false }),
        max_colors: 2,
        ..Default::default()
    };
    assert_eq!(sixel(&pixels, &options).unwrap().palette.len(), 2);
    options.dither = None;
    assert_eq!(sixel(&pixels, &options).unwrap().indices, vec![0; 6]);
    options.quantizer = Arc::new(Custom { invalid: true });
    assert!(sixel(&pixels, &options).is_err());
    options.dither = ImageOptions::default().dither;
    assert!(sixel(&pixels, &options).is_err());
    assert_eq!(options, options.clone());
}

#[test]
fn invalid_requests_and_settings_fail_explicitly() {
    let pixels = RgbImage::from_pixel(1, 1, image::Rgb([0; 3]));
    for max_colors in [0, 257] {
        assert!(
            Wu.quantize(QuantizeContext {
                pixels: &pixels,
                max_colors
            })
            .is_err()
        );
    }
    assert!(
        KMeans {
            sampling_factor: f32::NAN,
            ..Default::default()
        }
        .palette(QuantizeContext {
            pixels: &pixels,
            max_colors: 2
        })
        .is_err()
    );
    let pixels = RgbImage::new(0, 0);
    assert!(
        Wu.palette(QuantizeContext {
            pixels: &pixels,
            max_colors: 2
        })
        .is_err()
    );
}

#[test]
fn preparation_reuses_pixels_and_matches_builtin_palettes() {
    let pixels = RgbImage::from_fn(37, 19, |x, y| {
        image::Rgb([(x * 7) as u8, (y * 13) as u8, ((x + y) * 4) as u8])
    });
    for quantizer in [&Wu as &dyn Quantize, &KMeans::default()] {
        for max_colors in [1, 2, 16, 256] {
            let context = QuantizeContext {
                pixels: &pixels,
                max_colors,
            };
            let prepared = quantizer.prepare(context).unwrap();
            assert_eq!(prepared.palette(), quantizer.palette(context).unwrap());
            assert!(std::ptr::eq(prepared.pixels(), &pixels));
            assert!(prepared.colors.get().is_some());
            let converted = prepared.colors().as_ptr();
            for algorithm in dither::test_algorithms().into_iter().flatten() {
                let indices = dither::map(algorithm.as_ref(), prepared.context()).unwrap();
                color::validate_indices(&indices, 37 * 19, prepared.palette().len()).unwrap();
                assert_eq!(
                    indices,
                    dither::map(algorithm.as_ref(), prepared.context()).unwrap()
                );
                assert_eq!(converted, prepared.colors().as_ptr());
                if quantizer.same(&KMeans::default()) {
                    assert_eq!(
                        indices,
                        dither::map(
                            algorithm.as_ref(),
                            dither::DitherContext::new(&pixels, prepared.palette())
                        )
                        .unwrap()
                    );
                }
            }
            assert_eq!(
                prepared.context().nearest().unwrap(),
                color::nearest(&pixels, prepared.palette()).unwrap()
            );
        }
    }
}

#[test]
fn prepared_wu_retains_bin_indices_with_rounded_error_feedback() {
    use quantette::{Palette, PaletteBuf, color_map::IndexedColorMap, wu::WuF32x3ColorMap};

    struct Expected {
        bins: WuF32x3ColorMap<Oklab, 32, 16, 16>,
        destination: color::ColorMap,
    }
    impl IndexedColorMap<Oklab> for Expected {
        type Output = Oklab;
        fn into_palette(self) -> PaletteBuf<Oklab> {
            self.destination.into_palette()
        }
        fn palette(&self) -> &Palette<Oklab> {
            self.destination.palette()
        }
        fn base_palette(&self) -> &Palette<Oklab> {
            self.destination.palette()
        }
        fn palette_index(&self, color: &Oklab) -> u8 {
            self.bins.palette_index(color)
        }
    }

    let pixels = RgbImage::from_fn(64, 32, |x, y| {
        image::Rgb([(x * 4) as u8, (y * 8) as u8, ((x + y) * 2) as u8])
    });
    let colors = color::colors(&pixels);
    let input = ImageRef::new(64, 32, &colors).unwrap();
    let bins = WuF32x3::run_image(input, BinnerF32x3::oklab_from_srgb8())
        .unwrap()
        .color_map(16u16.try_into().unwrap());
    let mut prepared = Wu
        .prepare(QuantizeContext {
            pixels: &pixels,
            max_colors: 16,
        })
        .unwrap();
    let mut palette = prepared.palette().to_vec();
    round_sixel_palette(&mut palette);
    prepared.round_sixel_palette().unwrap();
    assert_eq!(prepared.palette(), palette);
    let expected = Expected {
        bins,
        destination: color::color_map(&palette).unwrap(),
    };
    assert_eq!(
        prepared.mapping.map_to_indices(&colors),
        expected.bins.map_to_indices(&colors)
    );
    assert_eq!(
        prepared.mapping.base_palette().as_slice(),
        expected.base_palette().as_slice()
    );
    let fs = quantette::dither::FloydSteinberg::default();
    let wrong = fs
        .dither_image_to_indexed(input, &expected.bins)
        .into_parts()
        .1;
    let correct = fs.dither_image_to_indexed(input, expected).into_parts().1;
    assert_ne!(correct, wrong);
    let options = ImageOptions {
        max_colors: 16,
        ..Default::default()
    };
    let output = sixel(&pixels, &options).unwrap();
    assert_eq!(output.palette, palette);
    assert_eq!(output.indices, correct);
}

#[test]
fn custom_preparation_and_exact_images_keep_callbacks() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Debug)]
    struct Custom;
    impl Quantize for Custom {
        fn palette(&self, _: QuantizeContext<'_>) -> io::Result<Vec<[u8; 3]>> {
            CALLS.fetch_add(1, Ordering::Relaxed);
            Ok(vec![[0; 3], [255; 3]])
        }
    }
    impl dither::Dither for Custom {
        fn dither(&self, context: dither::DitherContext<'_>) -> io::Result<Vec<u8>> {
            CALLS.fetch_add(1, Ordering::Relaxed);
            assert_eq!(context.palette(), [[0; 3], [255; 3]]);
            Ok(vec![0; context.pixels().as_raw().len() / 3])
        }
    }
    let pixels = RgbImage::from_pixel(3, 2, image::Rgb([255; 3]));
    let options = ImageOptions {
        quantizer: Arc::new(Custom),
        dither: Some(Arc::new(Custom)),
        ..Default::default()
    };
    let before = options.clone();
    assert_eq!(sixel(&pixels, &options).unwrap().indices, vec![0; 6]);
    assert_eq!(CALLS.load(Ordering::Relaxed), 2);
    assert_eq!(options, before);

    let prepared = Custom
        .prepare(QuantizeContext {
            pixels: &pixels,
            max_colors: 2,
        })
        .unwrap();
    assert!(prepared.colors.get().is_none());
    assert_eq!(prepared.context().nearest().unwrap(), vec![1; 6]);
    let converted = prepared.colors().as_ptr();
    assert_eq!(prepared.context().nearest().unwrap(), vec![1; 6]);
    assert_eq!(converted, prepared.colors().as_ptr());
}

#[test]
fn prepared_overrides_validate_source_budget_and_errors() {
    #[derive(Debug)]
    struct Override {
        other_source: bool,
        colors: usize,
        fails: bool,
    }
    impl Quantize for Override {
        fn palette(&self, _: QuantizeContext<'_>) -> io::Result<Vec<[u8; 3]>> {
            panic!("dithered output must use the preparation override");
        }
        fn prepare<'a>(&self, context: QuantizeContext<'a>) -> io::Result<PreparedImage<'a>> {
            if self.fails {
                return Err(io::Error::other("preparation failed"));
            }
            // A static source lets an override return a different, valid image.
            static OTHER: OnceLock<RgbImage> = OnceLock::new();
            let pixels = if self.other_source {
                OTHER.get_or_init(|| RgbImage::from_pixel(2, 2, image::Rgb([0; 3])))
            } else {
                context.pixels
            };
            PreparedImage::new(
                QuantizeContext {
                    pixels,
                    max_colors: 256,
                },
                vec![[0; 3]; self.colors],
            )
        }
    }
    let pixels = RgbImage::from_pixel(2, 2, image::Rgb([255; 3]));
    for (other_source, colors, fails, error) in [
        (false, 3, false, io::ErrorKind::InvalidInput),
        (true, 2, false, io::ErrorKind::InvalidInput),
        (false, 2, true, io::ErrorKind::Other),
    ] {
        let options = ImageOptions {
            quantizer: Arc::new(Override {
                other_source,
                colors,
                fails,
            }),
            max_colors: 2,
            ..Default::default()
        };
        assert_eq!(sixel(&pixels, &options).unwrap_err().kind(), error);
    }
}
#[test]
fn exact_preparation_shortcut_respects_rounding_grain_and_callbacks() {
    use dither::Dither;
    let pixels = RgbImage::from_fn(19, 13, |x, y| {
        image::Rgb(if (x + y) % 2 == 0 { [0; 3] } else { [255; 3] })
    });
    let context = QuantizeContext {
        pixels: &pixels,
        max_colors: 2,
    };
    for quantizer in [&Wu as &dyn Quantize, &KMeans::default()] {
        let mut prepared = quantizer.prepare(context).unwrap();
        prepared.round_sixel_palette().unwrap();
        assert!(prepared.exact_indices().is_some());
        let indices = dither::FloydSteinberg::default()
            .dither(prepared.context())
            .unwrap();
        assert!(prepared.colors.get().is_none());
        assert_eq!(
            indices,
            dither::FloydSteinberg::default()
                .dither(dither::DitherContext::new(&pixels, prepared.palette()))
                .unwrap()
        );
        for grain in [(1, 1), (2, 4), (8, 8)] {
            for algorithm in [
                &dither::FloydSteinberg {
                    grain,
                    ..Default::default()
                } as &dyn Dither,
                &dither::Bayer8x8 {
                    grain,
                    ..Default::default()
                },
                &dither::Noise {
                    grain,
                    ..Default::default()
                },
            ] {
                assert_eq!(
                    algorithm.dither(prepared.context()).unwrap(),
                    algorithm
                        .dither(dither::DitherContext::new(&pixels, prepared.palette()))
                        .unwrap()
                );
            }
        }
    }
    #[derive(Debug)]
    struct Custom;
    impl Dither for Custom {
        fn dither(&self, context: dither::DitherContext<'_>) -> io::Result<Vec<u8>> {
            Ok(vec![1; context.pixels().as_raw().len() / 3])
        }
    }
    let options = ImageOptions {
        dither: Some(Arc::new(Custom)),
        ..Default::default()
    };
    assert_eq!(sixel(&pixels, &options).unwrap().indices, vec![1; 19 * 13]);

    let pixels = RgbImage::from_pixel(19, 13, image::Rgb([1, 127, 254]));
    let mut prepared = Wu
        .prepare(QuantizeContext {
            pixels: &pixels,
            max_colors: 2,
        })
        .unwrap();
    assert!(prepared.exact_indices().is_some());
    prepared.round_sixel_palette().unwrap();
    assert!(prepared.exact_indices().is_none());
    assert_eq!(
        dither::FloydSteinberg::default()
            .dither(prepared.context())
            .unwrap(),
        dither::FloydSteinberg::default()
            .dither(dither::DitherContext::new(&pixels, prepared.palette()))
            .unwrap()
    );
    assert!(prepared.colors.get().is_some());
}

#[test]
fn custom_indexed_results_are_fully_validated() {
    #[derive(Debug)]
    struct Output(IndexedImage);
    impl Quantize for Output {
        fn palette(&self, _: QuantizeContext<'_>) -> io::Result<Vec<[u8; 3]>> {
            panic!("undithered output should use the indexed override");
        }
        fn quantize(&self, _: QuantizeContext<'_>) -> io::Result<IndexedImage> {
            Ok(self.0.clone())
        }
    }
    let pixels = RgbImage::from_pixel(2, 2, image::Rgb([0; 3]));
    let valid = IndexedImage {
        width: 2,
        height: 2,
        palette: vec![[0; 3]],
        indices: vec![0; 4],
    };
    for result in [
        IndexedImage {
            width: 1,
            ..valid.clone()
        },
        IndexedImage {
            indices: vec![0; 3],
            ..valid.clone()
        },
        IndexedImage {
            indices: vec![0; 5],
            ..valid.clone()
        },
        IndexedImage {
            indices: vec![1; 4],
            ..valid.clone()
        },
        IndexedImage {
            palette: vec![],
            ..valid.clone()
        },
        IndexedImage {
            palette: vec![[0; 3]; 3],
            ..valid.clone()
        },
    ] {
        let options = ImageOptions {
            quantizer: Arc::new(Output(result)),
            dither: None,
            max_colors: 2,
            ..Default::default()
        };
        assert_eq!(
            sixel(&pixels, &options).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
