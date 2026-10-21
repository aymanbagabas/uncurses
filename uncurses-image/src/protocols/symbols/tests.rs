use super::{
    SymbolSet,
    catalog::{self, Glyph},
};
use crate::{FilterType, Fit, ImageRenderer, Protocol};
use crate::{Image, ImageOptions};
use std::{collections::BTreeSet, io};
use uncurses::buffer::{Surface, SurfaceMut};
use uncurses::cell::Cell;
use uncurses::color::{Color, Profile};
use uncurses::layout::{Position, Rect, Size};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::{TextSurface, WidthMode};

#[test]
fn screen_symbols_follow_ambiguous_width_policy() {
    let image = Image::from_rgb(8, 8, vec![255; 8 * 8 * 3]).unwrap();
    let renderer = ImageRenderer {
        protocol: Protocol::Symbols("u2580".parse().unwrap()),
        ..Default::default()
    };
    let mut screen = Screen::new(Vec::new(), (5, 1));
    for eaw_wide in [false, true, false] {
        screen.set_eaw_wide(eaw_wide);
        screen.clear();
        renderer
            .draw(
                &mut screen,
                "image",
                Rect::new(0, 0, 5, 1),
                &image,
                &options(),
            )
            .unwrap();
        let cell = screen.cell(Position::ORIGIN).unwrap();
        assert_eq!(cell.content(), "\u{2580}");
        assert_eq!(u16::from(cell.width()), screen.str_width(cell.content()));
        screen.render().unwrap();
    }
}

#[test]
fn screen_symbols_follow_grapheme_measurement() {
    let image = glyph_image(glyph('\u{25b6}'));
    let renderer = ImageRenderer {
        protocol: Protocol::Symbols("u25b6".parse().unwrap()),
        ..Default::default()
    };
    let mut screen = Screen::new(Vec::new(), (2, 1));
    screen.set_eaw_wide(true);
    for grapheme in [false, true, false] {
        screen.set_grapheme_clusters(grapheme);
        screen.clear();
        renderer
            .draw(
                &mut screen,
                "image",
                Rect::new(0, 0, 2, 1),
                &image,
                &options(),
            )
            .unwrap();
        let cell = screen.cell(Position::ORIGIN).unwrap();
        assert_eq!(cell.content(), "\u{25b6}");
        assert_eq!(cell.width(), if grapheme { 1 } else { 2 });
        assert_eq!(u16::from(cell.width()), screen.str_width(cell.content()));
        screen.render().unwrap();
    }
}

#[test]
fn catalogs_measure_classify_and_stretch_glyphs_for_each_policy() {
    for (mode, eaw_wide) in catalog::POLICIES {
        let glyphs = catalog::glyphs(mode, eaw_wide);
        assert_eq!(
            glyphs.iter().map(|g| g.character).collect::<Vec<_>>(),
            catalog::GLYPHS
                .iter()
                .map(|g| g.character)
                .collect::<Vec<_>>()
        );
        for g in glyphs {
            let text = g.character.to_string();
            assert_eq!(g.width, mode.grapheme_width(&text, eaw_wide), "{text:?}");
            assert_eq!(
                SymbolSet::NARROW.contains_with_policy(g.character, mode, eaw_wide),
                g.width == 1
            );
            assert_eq!(
                SymbolSet::WIDE.contains_with_policy(g.character, mode, eaw_wide),
                g.width == 2
            );
            let original = glyph(g.character);
            if g.width == original.width {
                assert_eq!(g.mask, original.mask, "{text:?}");
            } else {
                assert_eq!((original.width, g.width), (1, 2), "{text:?}");
                for y in 0..8 {
                    for x in 0..16 {
                        let actual = (g.mask[x / 8] >> (y * 8 + x % 8)) & 1;
                        let expected = (original.mask[0] >> (y * 8 + x / 2)) & 1;
                        assert_eq!(actual, expected, "{text:?} ({x}, {y})");
                    }
                }
            }
        }
    }
    let left = catalog::glyphs(WidthMode::Wc, true)
        .iter()
        .find(|g| g.character == '\u{258c}')
        .unwrap();
    assert_eq!(left.mask, [u64::MAX, 0]);
}

#[test]
fn composed_selectors_keep_width_policy_until_rendering() {
    let block_without_wide: SymbolSet = "block-wide".parse().unwrap();
    let upper_without_wide: SymbolSet = "u2580-wide".parse().unwrap();
    let mixed: SymbolSet = "narrow+[\u{2580}]-wide".parse().unwrap();
    assert_eq!(SymbolSet::ALL - SymbolSet::WIDE, SymbolSet::NARROW);
    assert_eq!(mixed, SymbolSet::NARROW);
    assert_ne!(
        SymbolSet::NARROW,
        SymbolSet::ALL - "u3040..u309f".parse().unwrap()
    );
    for (mode, eaw_wide) in catalog::POLICIES {
        assert_eq!(
            block_without_wide.contains_with_policy('\u{2580}', mode, eaw_wide),
            !eaw_wide
        );
        assert!(block_without_wide.contains_with_policy(' ', mode, eaw_wide));
        assert_eq!(
            upper_without_wide.is_empty_with_policy(mode, eaw_wide),
            eaw_wide
        );
        for glyph in catalog::glyphs(mode, eaw_wide) {
            assert_eq!(
                mixed.contains_with_policy(glyph.character, mode, eaw_wide),
                glyph.width == 1
            );
        }
    }
}

#[test]
fn direct_and_screen_symbols_agree_across_policies_profiles_and_odd_sizes() {
    let image = glyph_image(glyph('\u{2580}'));
    for (mode, eaw_wide) in catalog::POLICIES {
        for profile in [
            Profile::TrueColor,
            Profile::Ansi256,
            Profile::Ansi,
            Profile::Ascii,
        ] {
            for protocol in [
                Protocol::Auto,
                Protocol::Symbols(SymbolSet::UPPER_LOWER),
                Protocol::Symbols(SymbolSet::LEFT_RIGHT),
                Protocol::Symbols(SymbolSet::NARROW),
                Protocol::Symbols(SymbolSet::WIDE),
                Protocol::Symbols(SymbolSet::ALL - SymbolSet::WIDE),
            ] {
                for width in [1, 2, 3, 5] {
                    for dither in [None, options().dither] {
                        let opts = ImageOptions {
                            dither,
                            ..options()
                        };
                        let saved = opts.clone();
                        let size = Size::new(width, 2);
                        let mut screen = Screen::new(Vec::new(), (width + 2, 2));
                        screen.set_eaw_wide(eaw_wide);
                        screen.set_grapheme_clusters(mode == WidthMode::Grapheme);
                        screen.set_color_profile(profile);
                        screen.set_str((width + 1, 0), "!", Style::default());
                        let renderer = ImageRenderer {
                            protocol: protocol.clone(),
                            ..Default::default()
                        };
                        let selected = renderer
                            .draw(
                                &mut screen,
                                "image",
                                Rect::new(1, 0, width, 2),
                                &image,
                                &opts,
                            )
                            .unwrap();
                        let Protocol::Symbols(symbols) = selected else {
                            panic!("expected symbol output");
                        };
                        let direct = image
                            .symbols_with_policy(
                                size,
                                &opts,
                                profile,
                                symbols.clone(),
                                mode,
                                eaw_wide,
                            )
                            .unwrap();
                        for y in 0..size.height {
                            let mut x = 0;
                            while x < width {
                                let cell = direct.cell((x, y).into()).unwrap();
                                assert_eq!(Some(cell), screen.cell((x + 1, y).into()));
                                assert!(!cell.is_continuation());
                                assert!(x + u16::from(cell.width()) <= width);
                                if !cell.is_blank() {
                                    assert_eq!(
                                        cell.width(),
                                        mode.grapheme_width(cell.content(), eaw_wide)
                                    );
                                    assert!(symbols.contains_with_policy(
                                        cell.content().chars().next().unwrap(),
                                        mode,
                                        eaw_wide
                                    ));
                                }
                                if cell.width() == 2 {
                                    assert!(
                                        direct.cell((x + 1, y).into()).unwrap().is_continuation()
                                    );
                                    assert!(
                                        screen.cell((x + 2, y).into()).unwrap().is_continuation()
                                    );
                                }
                                x += u16::from(cell.width());
                            }
                            assert_eq!(x, width);
                        }
                        assert_eq!(screen.cell((width + 1, 0).into()).unwrap().content(), "!");
                        assert_eq!(opts, saved);
                        screen.render().unwrap();
                    }
                }
            }
        }
    }
}

#[test]
fn ambiguous_half_blocks_use_two_cells_and_keep_their_shape() {
    let expected = catalog::glyphs(WidthMode::Wc, true)
        .iter()
        .find(|g| g.character == '\u{2580}')
        .unwrap();
    let image = glyph_image(expected);
    let opts = ImageOptions {
        dither: None,
        ..options()
    };
    for (mode, eaw_wide) in catalog::POLICIES {
        for profile in [Profile::TrueColor, Profile::Ascii] {
            let cells = image
                .symbols_with_policy(
                    Size::new(2, 1),
                    &opts,
                    profile,
                    SymbolSet::UPPER_LOWER,
                    mode,
                    eaw_wide,
                )
                .unwrap();
            let first = cells.cell(Position::ORIGIN).unwrap();
            assert_eq!(first.content(), "\u{2580}");
            assert_eq!(first.width(), if eaw_wide { 2 } else { 1 });
            assert_eq!(
                cells.cell((1, 0).into()).unwrap().is_continuation(),
                eaw_wide
            );
            if profile == Profile::TrueColor {
                assert_eq!(first.style.fg, Some(Color::Rgb(255, 255, 255)));
                assert_eq!(first.style.bg, Some(Color::Rgb(0, 0, 0)));
            }
        }
    }
}

#[test]
fn a_policy_dependent_empty_selection_preserves_the_screen() {
    let image = glyph_image(glyph('\u{2580}'));
    let renderer = ImageRenderer {
        protocol: Protocol::Symbols("u2580-wide".parse().unwrap()),
        ..Default::default()
    };
    let mut screen = Screen::new(Vec::new(), (3, 1));
    screen.set_eaw_wide(true);
    screen.set_str((0, 0), "old", Style::default());
    let err = renderer
        .draw(
            &mut screen,
            "image",
            Rect::new(0, 0, 3, 1),
            &image,
            &options(),
        )
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(screen.cell(Position::ORIGIN).unwrap().content(), "o");
    assert!(screen.writer().is_empty());
}

fn options() -> ImageOptions {
    ImageOptions {
        fit: Fit::Stretch,
        filter: FilterType::Nearest,
        ..Default::default()
    }
}

#[test]
fn dithering_changes_monochrome_symbols_but_preserves_truecolor_and_options() {
    use uncurses::text::Encode;
    let encoded = |buffer: uncurses::buffer::Buffer| {
        let mut bytes = Vec::new();
        buffer.encode_with(&mut bytes, Profile::TrueColor).unwrap();
        bytes
    };
    let image = Image::from_rgb(16, 8, vec![128; 16 * 8 * 3]).unwrap();
    for symbols in [SymbolSet::BRAILLE, SymbolSet::UPPER_LOWER] {
        let size = Size::new(8, 4);
        let mut options = options();
        options.dither = None;
        let plain = image
            .symbols(size, &options, Profile::Ascii, symbols.clone())
            .map(encoded)
            .unwrap();
        let color = image
            .symbols(size, &options, Profile::TrueColor, symbols.clone())
            .map(encoded)
            .unwrap();
        let algorithms: [std::sync::Arc<dyn crate::dither::Dither>; 3] = [
            std::sync::Arc::new(crate::dither::Bayer8x8 {
                grain: (4, 4),
                ..Default::default()
            }),
            std::sync::Arc::new(crate::dither::FloydSteinberg {
                grain: (4, 4),
                ..Default::default()
            }),
            std::sync::Arc::new(crate::dither::Noise {
                grain: (4, 4),
                ..Default::default()
            }),
        ];
        for algorithm in algorithms {
            options.dither = Some(algorithm);
            let before = options.clone();
            let cells = image
                .symbols(size, &options, Profile::Ascii, symbols.clone())
                .map(encoded)
                .unwrap();
            assert_ne!(cells, plain);
            assert_eq!(
                cells,
                image
                    .symbols(size, &options, Profile::Ascii, symbols.clone())
                    .map(encoded)
                    .unwrap()
            );
            assert_eq!(
                color,
                image
                    .symbols(size, &options, Profile::TrueColor, symbols.clone())
                    .map(encoded)
                    .unwrap()
            );
            assert_eq!(options, before);
        }
    }
}

fn glyph_image(glyph: &Glyph) -> Image {
    let pixels = (0..8)
        .flat_map(|y| {
            (0..usize::from(glyph.width) * 8).flat_map(move |x| {
                if glyph.mask[x / 8] & (1 << (y * 8 + x % 8)) != 0 {
                    [255, 255, 255, 255]
                } else {
                    [0, 0, 0, 255]
                }
            })
        })
        .collect();
    Image::from_rgba(u32::from(glyph.width) * 8, 8, pixels).unwrap()
}

fn glyph(character: char) -> &'static Glyph {
    catalog::GLYPHS
        .iter()
        .find(|g| g.character == character)
        .unwrap()
}

#[test]
fn every_manual_class_selects_real_glyphs() {
    for name in [
        "all",
        "space",
        "solid",
        "stipple",
        "block",
        "border",
        "diagonal",
        "dot",
        "quad",
        "half",
        "left_right",
        "upper_lower",
        "inverted",
        "braille",
        "technical",
        "geometric",
        "ascii",
        "legacy",
        "sextant",
        "wedge",
        "wide",
        "narrow",
        "octant",
        "alpha",
        "digit",
        "alnum",
        "latin",
        "default",
    ] {
        let selection: SymbolSet = name.parse().unwrap();
        assert!(!selection.is_empty(), "{name}");
        let selected = catalog::GLYPHS
            .iter()
            .find(|g| selection.contains(g.character) && g.mask[0] != 0)
            .or_else(|| {
                catalog::GLYPHS
                    .iter()
                    .find(|g| selection.contains(g.character))
            })
            .unwrap();
        for profile in [
            Profile::TrueColor,
            Profile::Ansi256,
            Profile::Ansi,
            Profile::Ascii,
            Profile::Disabled,
        ] {
            let image = glyph_image(selected);
            let cells = image
                .symbols(
                    Size::new(u16::from(selected.width), 1),
                    &options(),
                    profile,
                    selection.clone(),
                )
                .unwrap();
            let cell = cells.cell(Position::ORIGIN).unwrap();
            assert!(
                selection.contains(cell.content().chars().next().unwrap()),
                "{name}: {cell:?}"
            );
            if profile < Profile::Ansi {
                assert_eq!(cell.style, Style::default());
            } else {
                assert_eq!(profile.convert(cell.style.fg.unwrap()), cell.style.fg);
                assert_eq!(profile.convert(cell.style.bg.unwrap()), cell.style.bg);
            }
        }
    }
    assert!("none".parse::<SymbolSet>().unwrap().is_empty());
}

#[test]
fn selectors_apply_ordered_glyph_set_operations() {
    for (name, selection, glyphs) in [
        (
            "upper_lower",
            SymbolSet::UPPER_LOWER,
            ['\u{2580}', '\u{2584}'],
        ),
        (
            "left_right",
            SymbolSet::LEFT_RIGHT,
            ['\u{258c}', '\u{2590}'],
        ),
    ] {
        assert_eq!(name.parse::<SymbolSet>().unwrap(), selection);
        assert!(glyphs.into_iter().all(|glyph| selection.contains(glyph)));
    }
    assert_eq!(
        "upper_lower+left_right".parse::<SymbolSet>().unwrap(),
        SymbolSet::HALF
    );
    assert_eq!(
        "half-left_right".parse::<SymbolSet>().unwrap(),
        SymbolSet::UPPER_LOWER - SymbolSet::LEFT_RIGHT
    );
    let borders = SymbolSet::BORDER - SymbolSet::DIAGONAL;
    assert!(!borders.contains('\u{2571}'));
    assert!(borders.contains('\u{2500}'));
    assert_eq!(borders, "border-diagonal".parse().unwrap());
    assert_eq!(borders | SymbolSet::DIAGONAL, SymbolSet::BORDER);
    assert_eq!(
        (SymbolSet::BLOCK | SymbolSet::BORDER) - SymbolSet::SOLID,
        "block+border-solid".parse().unwrap()
    );
    assert_eq!(SymbolSet::DEFAULT - SymbolSet::DOT, "-dot".parse().unwrap());
    assert_eq!(
        SymbolSet::ASCII | SymbolSet::BRAILLE,
        "ascii,braille".parse().unwrap()
    );
    assert_eq!(
        "u2580..u259f".parse::<SymbolSet>().unwrap(),
        SymbolSet::BLOCK - SymbolSet::SPACE
    );
    assert_eq!(
        "[ab]".parse::<SymbolSet>().unwrap(),
        "u61..u62".parse().unwrap()
    );
    let escaped: SymbolSet = r"[\]\\]".parse().unwrap();
    assert!(escaped.contains(']') && escaped.contains('\\'));
    assert_eq!(SymbolSet::QUADRANT, "quad".parse().unwrap());
    for bad in [
        "",
        "vhalf",
        "hhalf",
        "missing",
        "block+",
        "block,,dot",
        "[abc",
        "[a]x",
        "u110000",
        "ud800",
        "u10000",
        "[\n]",
        "u62..u61",
    ] {
        assert!(bad.parse::<SymbolSet>().is_err(), "{bad:?}");
    }
}

#[test]
fn mosaic_families_have_complete_distinct_patterns() {
    for (symbols, count) in [
        (SymbolSet::QUADRANT, 16),
        (SymbolSet::SEXTANT, 64),
        (SymbolSet::OCTANT, 256),
        (SymbolSet::BRAILLE, 256),
    ] {
        let masks: BTreeSet<_> = catalog::GLYPHS
            .iter()
            .filter(|g| symbols.contains(g.character))
            .map(|g| g.mask[0])
            .collect();
        assert_eq!(masks.len(), count, "{symbols:?}");
    }
    assert_eq!(glyph('\u{1cd00}').mask[0], 0x0000_0000_0f0f_0000);
    assert_eq!(glyph('\u{1cde5}').mask[0], 0xffff_ffff_ffff_f0f0);
    assert_eq!(glyph('\u{1fb00}').mask[0], 0x0000_0000_000f_0f0f);
    assert_eq!(glyph('\u{2801}').mask[0], 0x0606);
    assert_eq!(glyph('\u{2880}').mask[0], 0x6060_0000_0000_0000);
    assert_eq!(glyph('\u{1fb3c}').mask[0], !glyph('\u{1fb52}').mask[0]);
}

#[test]
fn exact_shapes_survive_monochrome_matching() {
    for (symbols, character) in [
        (SymbolSet::QUADRANT, '\u{259a}'),
        (SymbolSet::SEXTANT, '\u{1fb19}'),
        (SymbolSet::OCTANT, '\u{1cd61}'),
        (SymbolSet::BRAILLE, '\u{2847}'),
        (SymbolSet::WEDGE, '\u{1fb41}'),
        (SymbolSet::ASCII, 'R'),
        (SymbolSet::WIDE, '\u{3042}'),
    ] {
        let expected = glyph(character);
        let cells = glyph_image(expected)
            .symbols(
                Size::new(u16::from(expected.width), 1),
                &options(),
                Profile::Ascii,
                symbols,
            )
            .unwrap();
        let actual = glyph(
            cells
                .cell(Position::ORIGIN)
                .unwrap()
                .content()
                .chars()
                .next()
                .unwrap(),
        );
        assert_eq!(actual.mask, expected.mask, "{character}");
    }
}

#[test]
fn mixed_wide_and_narrow_choices_preserve_grid_geometry() {
    let expected = glyph('\u{3042}');
    let image = glyph_image(expected);
    let cells = image
        .symbols(Size::new(3, 1), &options(), Profile::Ascii, SymbolSet::WIDE)
        .unwrap();
    assert_eq!(cells.cell((0, 0).into()).unwrap().width(), 2);
    assert!(cells.cell((1, 0).into()).unwrap().is_continuation());
    assert!(cells.cell((2, 0).into()).unwrap().is_blank());
    let cells = image
        .symbols(
            Size::new(2, 1),
            &options(),
            Profile::Ascii,
            SymbolSet::WIDE | SymbolSet::SPACE,
        )
        .unwrap();
    let actual = glyph(
        cells
            .cell((0, 0).into())
            .unwrap()
            .content()
            .chars()
            .next()
            .unwrap(),
    );
    assert_eq!(actual.width, 2);
    assert_eq!(actual.mask, expected.mask);
    for g in catalog::GLYPHS.iter() {
        assert!(!g.character.is_control());
        assert_eq!(
            uncurses::text::char_width(g.character, false),
            g.width,
            "{:?}",
            g.character
        );
    }
    assert!(!SymbolSet::ALL.contains('\u{03a2}'));
}

#[test]
fn palette_matching_and_geometry_keep_caller_options() {
    let image = glyph_image(glyph('\u{259a}'));
    let mut opts = options();
    opts.background = [12, 34, 56];
    let saved = opts.clone();
    let cells = image
        .symbols(
            Size::new(1, 1),
            &opts,
            Profile::TrueColor,
            SymbolSet::DEFAULT,
        )
        .unwrap();
    let cell = cells.cell(Position::ORIGIN).unwrap();
    let emitted = glyph(cell.content().chars().next().unwrap());
    for i in 0..64 {
        let color = if emitted.mask[0] & (1 << i) != 0 {
            cell.style.fg
        } else {
            cell.style.bg
        };
        let expected = if glyph('\u{259a}').mask[0] & (1 << i) != 0 {
            Color::Rgb(255, 255, 255)
        } else {
            Color::Rgb(0, 0, 0)
        };
        assert_eq!(color, Some(expected));
    }
    assert_eq!(opts, saved);
}

#[test]
fn invalid_selection_leaves_the_screen_unchanged() {
    let image = Image::from_rgba(1, 1, vec![255; 4]).unwrap();
    let mut screen = Screen::new(Vec::new(), (4, 2));
    screen.set_cell((0, 0), &Cell::new("X", 1));
    let renderer = ImageRenderer {
        protocol: Protocol::Symbols(SymbolSet::NONE),
        ..Default::default()
    };
    assert_eq!(
        renderer
            .draw(
                &mut screen,
                "picture",
                Rect::new(0, 0, 4, 2),
                &image,
                &options()
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert!(screen.writer().is_empty());
    assert_eq!(screen.cell(Position::ORIGIN).unwrap().content(), "X");
    for size in [Size::new(0, 1), Size::new(1, 0)] {
        assert!(
            image
                .symbols(size, &options(), Profile::TrueColor, SymbolSet::DEFAULT)
                .is_err()
        );
    }
}
