//! Backend selection and shared screen-region integration.

use std::io::{self, Write};
use std::sync::Arc;

use ::image::imageops;
use uncurses::buffer::{Bounded, Surface};
use uncurses::layout::{Position, Rect, Size};
use uncurses::screen::{PreparedRegion, Region, RegionContext, RegionOp, Screen};
use uncurses::text::TextSurface;

use crate::geometry::{self, coalesce};
use crate::protocols::{iterm2, kitty, kitty_placeholders, sixel};
use crate::{Image, ImageCapabilities, ImageOptions, Protocol, SymbolSet, invalid};

/// Select a backend and stage images through a screen.
///
/// This object performs no terminal I/O. `Screen::render` owns output and
/// resource completion. The same image and options can be declared each frame.
#[derive(Clone, Debug, Default)]
pub struct ImageRenderer {
    pub protocol: Protocol,
    pub capabilities: ImageCapabilities,
}

impl ImageRenderer {
    /// Stage an image and return the selected representation.
    ///
    /// Automatic selection prefers Kitty placeholders, native Kitty, iTerm2,
    /// Sixel, then default symbols.
    /// Sixel reserves a row below its rectangle for terminal cursor behavior.
    /// Automatic selection uses symbols when that row is unavailable.
    /// iTerm2 and Sixel clear terminal glyphs within each visible fragment
    /// before drawing pixels. The screen retains the underlying desired text.
    ///
    /// Automatic placeholders require detected Kitty support, true color, and at
    /// most 297 cells per dimension. Otherwise Kitty uses native placements.
    /// This policy assumes Unicode support from basic Kitty graphics support.
    /// Select `Protocol::Kitty` for native-only terminals.
    /// Explicit `Protocol::KittyPlaceholders` rejects unsupported
    /// sizes and profiles. Like symbols, placeholder cells require explicit
    /// clearing when moved.
    ///
    /// Symbols follow the screen's color profile and text-width policy.
    /// Redraw the image after a policy change to rebuild its cells.
    pub fn draw<W: Write>(
        &self,
        screen: &mut Screen<W>,
        key: &str,
        rect: Rect,
        image: &Image,
        options: &ImageOptions,
    ) -> io::Result<Protocol> {
        if rect.is_empty() || rect.intersection(screen.bounds()) != rect {
            return Err(invalid("image rectangle must fit inside the screen"));
        }
        let size = Size::new(rect.width, rect.height);
        geometry::resolve(image.dimensions(), size, (1, 1), options)?;
        let mut protocol = self.protocol.clone();
        if protocol == Protocol::Auto {
            protocol = if self.capabilities.kitty {
                if screen.color_profile() == uncurses::color::Profile::TrueColor
                    && usize::from(size.width) <= kitty_placeholders::MAX_DIMENSION
                    && usize::from(size.height) <= kitty_placeholders::MAX_DIMENSION
                {
                    Protocol::KittyPlaceholders
                } else {
                    Protocol::Kitty
                }
            } else if self.capabilities.iterm2 && cfg!(feature = "png") {
                Protocol::Iterm2
            } else if self.capabilities.sixel && rect.bottom() < screen.height() {
                Protocol::Sixel
            } else {
                Protocol::Symbols(SymbolSet::default())
            };
        }
        if protocol == Protocol::Iterm2 && !cfg!(feature = "png") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "iTerm2 images require the png feature",
            ));
        }
        if protocol == Protocol::Sixel && rect.bottom() >= screen.height() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Sixel requires a spare row below the image; use automatic selection or symbols",
            ));
        }
        if let Protocol::Symbols(symbols) = &protocol {
            let cells = image.symbols_with_policy(
                size,
                options,
                screen.color_profile(),
                symbols.clone(),
                screen.width_mode(),
                screen.eaw_wide(),
            )?;
            screen.remove_region(key);
            screen.remove_region_resource(key);
            cells.draw(screen, Position::new(rect.x, rect.y));
        } else if matches!(protocol, Protocol::KittyPlaceholders) {
            image
                .kitty_placeholders(size, options)?
                .draw(screen, key, rect.position())?;
        } else {
            screen.set_region(
                key,
                rect,
                Placement {
                    image: image.clone(),
                    options: options.clone(),
                    protocol: protocol.clone(),
                    rect,
                },
            )?;
            screen.remove_region_resource(key);
        }
        Ok(protocol)
    }
}

struct Placement {
    image: Image,
    options: ImageOptions,
    protocol: Protocol,
    rect: Rect,
}

impl Region for Placement {
    fn same(&self, other: &dyn Region) -> bool {
        other.downcast_ref::<Self>().is_some_and(|other| {
            Arc::ptr_eq(&self.image.pixels, &other.image.pixels)
                && if self.protocol == Protocol::Sixel {
                    self.options == other.options
                } else {
                    self.options.same_raster(&other.options)
                }
                && self.protocol == other.protocol
                && (self.protocol != Protocol::Sixel || self.rect == other.rect)
        })
    }

    fn prepare(&self, size: Size, context: RegionContext) -> io::Result<PreparedRegion> {
        self.prepare_spans(
            size,
            context,
            vec![Rect::new(0, 0, size.width, size.height)],
        )
    }

    fn prepare_visible(
        &self,
        size: Size,
        context: RegionContext,
        visible: &mut dyn Iterator<Item = Rect>,
    ) -> io::Result<Option<PreparedRegion>> {
        self.prepare_spans(size, context, coalesce(visible))
            .map(Some)
    }
}

impl Placement {
    fn prepare_spans(
        &self,
        size: Size,
        context: RegionContext,
        visible: Vec<Rect>,
    ) -> io::Result<PreparedRegion> {
        if self.protocol == Protocol::Sixel && self.rect.bottom() >= context.screen_size.height {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Sixel requires a spare row after resize",
            ));
        }
        let mut prepared = PreparedRegion::default();
        if matches!(self.protocol, Protocol::Kitty) {
            prepared = kitty::prepare_placement(&self.image, size, &self.options, visible)?;
        } else {
            let pixels = self.image.rasterize(size, &self.options)?;
            let indexed = if self.protocol == Protocol::Sixel {
                Some(crate::quant::sixel(&pixels, &self.options)?)
            } else {
                None
            };
            for rect in visible {
                let (cw, ch) = self.options.cell_pixels;
                let pixel_rect = (
                    u32::from(rect.x) * u32::from(cw),
                    u32::from(rect.y) * u32::from(ch),
                    u32::from(rect.width) * u32::from(cw),
                    u32::from(rect.height) * u32::from(ch),
                );
                let bytes = if let Some(indexed) = &indexed {
                    sixel::encode(indexed, pixel_rect)?
                } else if self.protocol == Protocol::Iterm2 {
                    let fragment = imageops::crop_imm(
                        &pixels,
                        pixel_rect.0,
                        pixel_rect.1,
                        pixel_rect.2,
                        pixel_rect.3,
                    )
                    .to_image();
                    iterm2::encode(fragment, rect)?
                } else {
                    return Err(invalid("expected a native image protocol"));
                };
                prepared.draw.extend([
                    // Legacy graphics can leave existing glyphs above the pixels.
                    RegionOp::Erase(rect),
                    RegionOp::MoveTo(Position::new(rect.x, rect.y)),
                    RegionOp::Write(bytes),
                ]);
            }
        }
        prepared
            .erase
            .push(RegionOp::Erase(Rect::new(0, 0, size.width, size.height)));
        Ok(prepared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uncurses::buffer::SurfaceMut;
    use uncurses::style::Style;
    use uncurses::text::TextSurface;

    #[test]
    fn builtin_sixel_dithers_preserve_equal_clipped_frames() {
        let image = Image::from_rgb(16, 8, vec![128; 16 * 8 * 3]).unwrap();
        for algorithm in crate::dither::test_algorithms() {
            let mut options = ImageOptions {
                cell_pixels: (2, 2),
                ..Default::default()
            };
            options.dither = algorithm;
            let before = options.clone();
            let renderer = ImageRenderer {
                protocol: Protocol::Sixel,
                ..Default::default()
            };
            let mut screen = Screen::new(Vec::new(), (10, 8));
            screen.set_fullscreen(true);
            for round in 0..2 {
                screen.clear();
                renderer
                    .draw(
                        &mut screen,
                        "image",
                        Rect::new(1, 1, 6, 4),
                        &image,
                        &options,
                    )
                    .unwrap();
                screen.set_str((3, 2), "X", Style::default());
                screen.render().unwrap();
                assert_eq!(screen.writer().is_empty(), round == 1);
                screen.writer_mut().clear();
            }
            assert_eq!(options, before);
        }
    }

    #[test]
    fn full_color_backends_ignore_dithers_and_reuse_resources() {
        use crate::dither::{Dither, DitherContext};
        #[derive(Debug)]
        struct Unexpected;
        impl Dither for Unexpected {
            fn dither(&self, _: DitherContext<'_>) -> io::Result<Vec<u8>> {
                panic!("full-color output must not call a custom dither");
            }
        }
        impl crate::quant::Quantize for Unexpected {
            fn palette(&self, _: crate::quant::QuantizeContext<'_>) -> io::Result<Vec<[u8; 3]>> {
                panic!("full-color output must not call a custom quantizer");
            }
        }
        let image = Image::from_rgb(1, 1, vec![12, 34, 56]).unwrap();
        for protocol in [
            Protocol::KittyPlaceholders,
            Protocol::Kitty,
            Protocol::Iterm2,
            Protocol::Symbols(SymbolSet::UPPER_LOWER),
        ] {
            if protocol == Protocol::Iterm2 && !cfg!(feature = "png") {
                continue;
            }
            let renderer = ImageRenderer {
                protocol,
                ..Default::default()
            };
            let mut screen = Screen::new(Vec::new(), (10, 8));
            screen.set_fullscreen(true);
            screen.set_color_profile(uncurses::color::Profile::TrueColor);
            let mut options = ImageOptions {
                cell_pixels: (2, 2),
                ..Default::default()
            };
            for round in 0..2 {
                screen.clear();
                renderer
                    .draw(
                        &mut screen,
                        "image",
                        Rect::new(1, 1, 6, 4),
                        &image,
                        &options,
                    )
                    .unwrap();
                screen.render().unwrap();
                assert_eq!(screen.writer().is_empty(), round == 1);
                screen.writer_mut().clear();
                options.dither = Some(Arc::new(Unexpected));
                options.quantizer = Arc::new(Unexpected);
                options.max_colors = 2;
            }
        }
    }

    #[test]
    fn custom_dithers_receive_full_rasters_and_preserve_equal_frames() {
        use crate::dither::{Dither, DitherContext};
        #[derive(Debug)]
        struct Fixed(u8);
        impl Dither for Fixed {
            fn dither(&self, context: DitherContext<'_>) -> io::Result<Vec<u8>> {
                assert_eq!(context.pixels().dimensions(), (12, 8));
                Ok(vec![self.0; 12 * 8])
            }
        }
        let image = Image::from_rgb(2, 1, vec![0, 0, 0, 255, 255, 255]).unwrap();
        let mut options = ImageOptions {
            cell_pixels: (2, 2),
            ..Default::default()
        };
        options.dither = Some(Arc::new(Fixed(0)));
        let renderer = ImageRenderer {
            protocol: Protocol::Sixel,
            ..Default::default()
        };
        let rect = Rect::new(1, 1, 6, 4);
        let mut screen = Screen::new(Vec::new(), (10, 8));
        screen.set_fullscreen(true);
        for round in 0..2 {
            screen.clear();
            renderer
                .draw(&mut screen, "image", rect, &image, &options)
                .unwrap();
            screen.set_str((3, 2), "X", Style::default());
            screen.render().unwrap();
            if round == 1 {
                assert!(screen.writer().is_empty());
            }
            screen.writer_mut().clear();
        }
        options.dither = Some(Arc::new(Fixed(1)));
        renderer
            .draw(&mut screen, "image", rect, &image, &options)
            .unwrap();
        screen.render().unwrap();
        assert!(!screen.writer().is_empty());
        screen.writer_mut().clear();
        options.dither = Some(Arc::new(Fixed(255)));
        renderer
            .draw(&mut screen, "image", rect, &image, &options)
            .unwrap();
        assert_eq!(
            screen.render().unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(screen.writer().is_empty());
        options.dither = None;
        renderer
            .draw(&mut screen, "image", rect, &image, &options)
            .unwrap();
        screen.render().unwrap();
    }

    #[test]
    fn quantizer_configuration_and_failures_preserve_frame_state() {
        use crate::quant::{KMeans, Quantize, QuantizeContext, Wu};
        #[derive(Debug)]
        struct Invalid;
        impl Quantize for Invalid {
            fn palette(&self, _: QuantizeContext<'_>) -> io::Result<Vec<[u8; 3]>> {
                Err(io::Error::other("palette failure"))
            }
        }
        let image = Image::from_rgb(2, 1, vec![0, 0, 0, 255, 255, 255]).unwrap();
        let renderer = ImageRenderer {
            protocol: Protocol::Sixel,
            ..Default::default()
        };
        for kmeans in [false, true] {
            let mut screen = Screen::new(Vec::new(), (10, 8));
            screen.set_fullscreen(true);
            for round in 0..2 {
                let options = ImageOptions {
                    quantizer: if kmeans {
                        Arc::new(KMeans::default())
                    } else {
                        Arc::new(Wu)
                    },
                    cell_pixels: (2, 2),
                    ..Default::default()
                };
                screen.clear();
                renderer
                    .draw(
                        &mut screen,
                        "image",
                        Rect::new(1, 1, 6, 4),
                        &image,
                        &options,
                    )
                    .unwrap();
                screen.render().unwrap();
                assert_eq!(screen.writer().is_empty(), round == 1);
                screen.writer_mut().clear();
            }
            let mut options = ImageOptions {
                quantizer: Arc::new(Invalid),
                ..Default::default()
            };
            renderer
                .draw(
                    &mut screen,
                    "image",
                    Rect::new(1, 1, 6, 4),
                    &image,
                    &options,
                )
                .unwrap();
            assert_eq!(screen.render().unwrap_err().to_string(), "palette failure");
            assert!(screen.writer().is_empty());
            options.quantizer = Arc::new(Wu);
            renderer
                .draw(
                    &mut screen,
                    "image",
                    Rect::new(1, 1, 6, 4),
                    &image,
                    &options,
                )
                .unwrap();
            screen.render().unwrap();
        }
    }

    #[test]
    fn automatic_kitty_selects_unicode_within_profile_and_diacritic_limits() {
        use uncurses::color::Profile;
        let image = Image::from_rgba(1, 1, vec![255; 4]).unwrap();
        let options = ImageOptions {
            cell_pixels: (1, 1),
            ..Default::default()
        };
        for (profile, size, expected) in [
            (Profile::TrueColor, Size::new(4, 2), true),
            (Profile::Ansi256, Size::new(4, 2), false),
            (Profile::Ansi, Size::new(4, 2), false),
            (Profile::Ascii, Size::new(4, 2), false),
            (Profile::TrueColor, Size::new(297, 1), true),
            (Profile::TrueColor, Size::new(298, 1), false),
            (Profile::TrueColor, Size::new(1, 297), true),
            (Profile::TrueColor, Size::new(1, 298), false),
        ] {
            let mut screen = Screen::new(Vec::new(), (size.width, size.height + 1));
            screen.set_fullscreen(true);
            screen.set_color_profile(profile);
            let renderer = ImageRenderer {
                capabilities: ImageCapabilities {
                    kitty: true,
                    iterm2: true,
                    sixel: true,
                },
                ..Default::default()
            };
            let rect = Rect::new(0, 0, size.width, size.height);
            assert_eq!(
                renderer
                    .draw(&mut screen, "image", rect, &image, &options)
                    .unwrap(),
                if expected {
                    Protocol::KittyPlaceholders
                } else {
                    Protocol::Kitty
                },
            );
            screen.render().unwrap();
            assert_eq!(screen.writer().windows(3).any(|b| b == b"U=1"), expected);
        }
    }

    #[test]
    fn explicit_native_kitty_overrides_automatic_unicode_selection() {
        let image = Image::from_rgba(1, 1, vec![255; 4]).unwrap();
        let mut screen = Screen::new(Vec::new(), (4, 2));
        screen.set_color_profile(uncurses::color::Profile::TrueColor);
        let renderer = ImageRenderer {
            protocol: Protocol::Kitty,
            capabilities: ImageCapabilities {
                kitty: true,
                ..Default::default()
            },
        };
        assert_eq!(
            renderer
                .draw(
                    &mut screen,
                    "image",
                    Rect::new(0, 0, 4, 2),
                    &image,
                    &ImageOptions::default(),
                )
                .unwrap(),
            Protocol::Kitty
        );
        screen.render().unwrap();
        assert!(screen.writer().windows(3).any(|b| b == b"a=p"));
        assert!(!screen.writer().windows(3).any(|b| b == b"U=1"));
    }

    #[test]
    fn resources_follow_clipping_replay_and_equal_rebuilds() {
        let image = Image::from_rgba(1, 1, vec![255, 0, 0, 255]).unwrap();
        let options = ImageOptions {
            cell_pixels: (1, 2),
            ..Default::default()
        };
        let renderer = ImageRenderer {
            protocol: Protocol::Kitty,
            ..Default::default()
        };
        let mut screen = Screen::new(Vec::new(), (20, 8));
        screen.set_fullscreen(true);
        let rect = Rect::new(1, 1, 4, 3);
        renderer
            .draw(&mut screen, "image", rect, &image, &options)
            .unwrap();
        screen.render().unwrap();
        let bytes = std::mem::take(screen.writer_mut());
        let upload = bytes.windows(3).position(|b| b == b"a=t").unwrap();
        let clear = bytes.windows(4).position(|b| b == b"\x1b[2J").unwrap();
        assert!(upload > clear);
        for _ in 0..2 {
            screen.clear();
            renderer
                .draw(&mut screen, "image", rect, &image, &options)
                .unwrap();
            screen.render().unwrap();
            assert!(screen.writer().is_empty());
        }
        screen.set_str((2, 2), "X", Style::default());
        screen.render().unwrap();
        let bytes = std::mem::take(screen.writer_mut());
        assert!(!bytes.windows(3).any(|b| b == b"a=t"));
        assert_eq!(bytes.windows(3).filter(|b| *b == b"a=p").count(), 4);
        screen.insert_above("log").unwrap();
        let bytes = std::mem::take(screen.writer_mut());
        assert_eq!(bytes.windows(3).filter(|b| *b == b"a=t").count(), 1);
        screen.clear();
        screen.render().unwrap();
        assert!(screen.writer().windows(7).any(|b| b == b"a=d,d=I"));
    }

    #[test]
    fn legacy_images_clear_only_visible_fragments_before_drawing() {
        let image = Image::from_rgba(1, 1, vec![255, 0, 0, 255]).unwrap();
        let size = Size::new(4, 3);
        let context = RegionContext {
            screen_size: Size::new(20, 8),
            color_profile: uncurses::color::Profile::TrueColor,
        };
        let visible = [
            Rect::new(0, 0, 4, 1),
            Rect::new(0, 1, 1, 2),
            Rect::new(3, 1, 1, 2),
        ];
        for protocol in [Protocol::Iterm2, Protocol::Sixel] {
            if protocol == Protocol::Iterm2 && !cfg!(feature = "png") {
                continue;
            }
            let placement = Placement {
                image: image.clone(),
                options: ImageOptions::default(),
                protocol,
                rect: Rect::new(2, 2, size.width, size.height),
            };
            let prepared = placement
                .prepare_spans(size, context, visible.to_vec())
                .unwrap();
            assert_eq!(prepared.draw.len(), visible.len() * 3);
            for (ops, rect) in prepared.draw.as_chunks::<3>().0.iter().zip(visible) {
                assert_eq!(ops[0], RegionOp::Erase(rect));
                assert_eq!(ops[1], RegionOp::MoveTo(rect.position()));
                assert!(matches!(&ops[2], RegionOp::Write(bytes) if !bytes.is_empty()));
            }
        }
    }

    #[test]
    fn selection_rejects_unsafe_or_unavailable_protocols_before_mutation() {
        let image = Image::from_rgba(1, 1, vec![255; 4]).unwrap();
        let options = ImageOptions::default();
        let mut screen = Screen::new(Vec::new(), (4, 2));
        let rect = Rect::new(0, 0, 4, 2);
        let mut renderer = ImageRenderer::default();
        assert_eq!(
            renderer
                .draw(&mut screen, "image", rect, &image, &options)
                .unwrap(),
            Protocol::Symbols(SymbolSet::DEFAULT)
        );
        renderer.capabilities.sixel = true;
        assert_eq!(
            renderer
                .draw(&mut screen, "image", rect, &image, &options)
                .unwrap(),
            Protocol::Symbols(SymbolSet::DEFAULT)
        );
        renderer.protocol = Protocol::Sixel;
        assert_eq!(
            renderer
                .draw(&mut screen, "image", rect, &image, &options)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        assert!(screen.writer().is_empty());
        if !cfg!(feature = "png") {
            renderer.protocol = Protocol::Auto;
            renderer.capabilities.iterm2 = true;
            assert_eq!(
                renderer
                    .draw(&mut screen, "image", rect, &image, &options)
                    .unwrap(),
                Protocol::Symbols(SymbolSet::DEFAULT)
            );
            renderer.protocol = Protocol::Iterm2;
            assert_eq!(
                renderer
                    .draw(&mut screen, "image", rect, &image, &options)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::Unsupported
            );
        }
    }
}
