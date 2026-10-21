//! Image sampling, glyph matching, and cell construction.

use std::io;

use uncurses::buffer::{Buffer, SurfaceMut};
use uncurses::cell::Cell;
use uncurses::color::{Color, Profile};
use uncurses::layout::{Position, Size};
use uncurses::style::Style;
use uncurses::text::WidthMode;

use super::{
    SymbolSet,
    catalog::{self, Glyph},
};
use crate::{Image, ImageOptions, invalid};

impl Image {
    /// Approximate an image with ordinary character cells.
    ///
    /// Families select candidate glyphs, independently of physical cell geometry
    /// in `options`. The matcher samples an 8-by-8 grid per cell, chooses nearby
    /// shapes, then fits foreground and background colors to the output profile.
    /// Wide glyphs compete with two adjacent narrow cells.
    ///
    /// `UPPER_LOWER` alone uses the faster two-sample path for undithered
    /// or true-color output. Active dithering uses the shared 8-by-8 sample grid.
    /// Dithering maps that grid to the output palette before glyph matching.
    /// Colorless output assumes light ink on a dark background and emits
    /// unstyled glyphs. Other profiles compare the actual quantized colors.
    ///
    /// The returned buffer uses normal text composition and diffing. Clear its
    /// old cells explicitly when moving it. Caller options remain unchanged.
    ///
    /// This method uses [`WidthMode::Wc`] and one-cell ambiguous characters.
    /// Use [`symbols_with_policy`](Self::symbols_with_policy) for an explicit policy.
    /// [`ImageRenderer`](crate::ImageRenderer) uses the screen's policy automatically.
    pub fn symbols(
        &self,
        size: Size,
        options: &ImageOptions,
        profile: Profile,
        symbols: SymbolSet,
    ) -> io::Result<Buffer> {
        self.symbols_with_policy(size, options, profile, symbols, WidthMode::Wc, false)
    }

    /// Approximate an image with the supplied text-width policy.
    ///
    /// Obtain `mode` and `eaw_wide` from the destination's
    /// [`TextSurface`](uncurses::text::TextSurface) implementation.
    /// Glyph widths, masks, and width-based selections follow that policy.
    /// Two-cell glyphs occupy adjacent columns. An unmatched final column stays blank.
    ///
    /// The one-cell half-block fast path applies when every half-block glyph
    /// occupies one cell. Other policies use the shared glyph matcher.
    /// Sampling, colors, and dithering follow [`symbols`](Self::symbols).
    /// An empty selection for the supplied policy returns an input error.
    pub fn symbols_with_policy(
        &self,
        size: Size,
        options: &ImageOptions,
        profile: Profile,
        symbols: SymbolSet,
        mode: WidthMode,
        eaw_wide: bool,
    ) -> io::Result<Buffer> {
        if symbols.is_empty_with_policy(mode, eaw_wide) {
            return Err(invalid("select at least one supported symbol family"));
        }
        if symbols == SymbolSet::UPPER_LOWER
            && (options.dither.is_none() || profile == Profile::TrueColor)
            && [" ", "\u{2580}", "\u{2584}", "\u{2588}"]
                .iter()
                .all(|glyph| mode.grapheme_width(glyph, eaw_wide) == 1)
        {
            return self.vertical_halves(size, options, profile);
        }
        let mut pixels = self.sample(
            size,
            options,
            (u32::from(size.width) * 8, u32::from(size.height) * 8),
        )?;
        if let Some(dither) = &options.dither
            && profile != Profile::TrueColor
        {
            let palette = if profile < Profile::Ansi {
                vec![[0, 0, 0], [255, 255, 255]]
            } else {
                let count = if profile == Profile::Ansi { 16 } else { 256 };
                (0..count)
                    .map(|i| {
                        let (r, g, b) = Color::Indexed(i as u8).to_rgb();
                        [r, g, b]
                    })
                    .collect()
            };
            let indices = crate::dither::map(
                dither.as_ref(),
                crate::dither::DitherContext::new(&pixels, &palette),
            )?;
            for (pixel, index) in pixels.pixels_mut().zip(indices) {
                pixel.0 = palette[usize::from(index)];
            }
        }
        let selected: Vec<_> = catalog::glyphs(mode, eaw_wide)
            .iter()
            .filter(|glyph| symbols.matches(glyph))
            .collect();
        let narrow: Vec<_> = selected.iter().copied().filter(|g| g.width == 1).collect();
        let wide: Vec<_> = selected.iter().copied().filter(|g| g.width == 2).collect();
        let mut cells = Buffer::new(size.width, size.height);
        let width = usize::from(size.width);
        let mut tiles = vec![[[0; 3]; 64]; width];
        for y in 0..size.height {
            for (x, tile) in tiles.iter_mut().enumerate() {
                for (i, pixel) in tile.iter_mut().enumerate() {
                    *pixel = pixels
                        .get_pixel(
                            x as u32 * 8 + (i % 8) as u32,
                            u32::from(y) * 8 + (i / 8) as u32,
                        )
                        .0;
                }
            }
            let mut costs = vec![0u64; width + 1];
            let mut choices = vec![None; width];
            for x in (0..width).rev() {
                let mut best = match_glyph(&narrow, &tiles[x..x + 1], profile);
                let mut cost = best.as_ref().map_or(u64::MAX, |m| m.error + costs[x + 1]);
                if x + 1 < width
                    && let Some(pair) = match_glyph(&wide, &tiles[x..x + 2], profile)
                {
                    let pair_cost = pair.error + costs[x + 2];
                    if pair_cost < cost {
                        cost = pair_cost;
                        best = Some(pair);
                    }
                }
                // A wide-only selection leaves an odd trailing column blank.
                costs[x] = if best.is_some() { cost } else { 0 };
                choices[x] = best;
            }
            let mut x = 0;
            while x < width {
                if let Some(choice) = &choices[x] {
                    cells.set_cell(
                        Position::new(x as u16, y),
                        &Cell::new(choice.glyph.character.to_string(), choice.glyph.width)
                            .style(choice.style.clone()),
                    );
                    x += usize::from(choice.glyph.width);
                } else {
                    x += 1;
                }
            }
        }
        Ok(cells)
    }
}

#[derive(Clone)]
struct Match<'a> {
    glyph: &'a Glyph,
    style: Style,
    error: u64,
}

fn match_glyph<'a>(
    glyphs: &[&'a Glyph],
    tiles: &[[[u8; 3]; 64]],
    profile: Profile,
) -> Option<Match<'a>> {
    if glyphs.is_empty() {
        return None;
    }
    let colored = profile >= Profile::Ansi;
    let mut lo = [255; 3];
    let mut hi = [0; 3];
    for pixel in tiles.iter().flatten() {
        for c in 0..3 {
            lo[c] = lo[c].min(pixel[c]);
            hi[c] = hi[c].max(pixel[c]);
        }
    }
    let axis = (0..3).max_by_key(|&c| hi[c] - lo[c]).unwrap();
    let threshold = (u16::from(lo[axis]) + u16::from(hi[axis])) / 2;
    let mut target = [0u64; 2];
    for (j, tile) in tiles.iter().enumerate() {
        for (i, pixel) in tile.iter().enumerate() {
            let lit = if colored {
                u16::from(pixel[axis]) > threshold
            } else {
                299 * u32::from(pixel[0]) + 587 * u32::from(pixel[1]) + 114 * u32::from(pixel[2])
                    >= 128_000
            };
            if lit {
                target[j] |= 1 << i;
            }
        }
    }
    // A bounded shape shortlist keeps large alphabets practical. Color fitting
    // scores its candidates against the quantized palette, not just the mask.
    let mut shortlist = [(u32::MAX, 0usize); 16];
    for (index, glyph) in glyphs.iter().enumerate() {
        let distance: u32 = (0..tiles.len())
            .map(|j| (target[j] ^ glyph.mask[j]).count_ones())
            .sum();
        let distance = if colored {
            distance.min(tiles.len() as u32 * 64 - distance)
        } else {
            distance
        };
        if distance < shortlist[15].0 {
            let at = shortlist.partition_point(|entry| entry.0 <= distance);
            shortlist.copy_within(at..15, at + 1);
            shortlist[at] = (distance, index);
        }
    }
    shortlist
        .iter()
        .take_while(|(d, _)| *d != u32::MAX)
        .map(|&(_, index)| fit(glyphs[index], tiles, profile))
        .min_by_key(|m| m.error)
}

fn fit<'a>(glyph: &'a Glyph, tiles: &[[[u8; 3]; 64]], profile: Profile) -> Match<'a> {
    let mut sums = [[0u32; 3]; 2];
    let mut counts = [0u32; 2];
    for (j, tile) in tiles.iter().enumerate() {
        for (i, pixel) in tile.iter().enumerate() {
            let side = ((glyph.mask[j] >> i) & 1) as usize;
            counts[side] += 1;
            for c in 0..3 {
                sums[side][c] += u32::from(pixel[c]);
            }
        }
    }
    let mut rgb = [[0u8; 3], [255u8; 3]];
    let mut style = Style::default();
    if profile >= Profile::Ansi {
        let colors: [Color; 2] = std::array::from_fn(|side| {
            let mean: [u8; 3] = std::array::from_fn(|c| {
                ((sums[side][c] + counts[side] / 2) / counts[side].max(1)) as u8
            });
            let color = profile
                .convert(Color::Rgb(mean[0], mean[1], mean[2]))
                .unwrap();
            let (r, g, b) = color.to_rgb();
            rgb[side] = [r, g, b];
            color
        });
        style = style.fg(colors[1]).bg(colors[0]);
    }
    let mut error = 0;
    for (j, tile) in tiles.iter().enumerate() {
        for (i, pixel) in tile.iter().enumerate() {
            let side = ((glyph.mask[j] >> i) & 1) as usize;
            for c in 0..3 {
                let d = i64::from(pixel[c]) - i64::from(rgb[side][c]);
                error += (d * d) as u64;
            }
        }
    }
    Match {
        glyph,
        style,
        error,
    }
}

impl Image {
    fn vertical_halves(
        &self,
        size: Size,
        options: &ImageOptions,
        profile: Profile,
    ) -> io::Result<Buffer> {
        let pixels = self.sample(
            size,
            options,
            (u32::from(size.width), u32::from(size.height) * 2),
        )?;
        let mut cells = Buffer::new(size.width, size.height);
        for y in 0..size.height {
            for x in 0..size.width {
                let top = pixels.get_pixel(u32::from(x), u32::from(y) * 2).0;
                let bottom = pixels.get_pixel(u32::from(x), u32::from(y) * 2 + 1).0;
                let cell = if profile >= Profile::Ansi {
                    Cell::new("\u{2580}", 1).style(
                        Style::default()
                            .fg(profile.convert(Color::Rgb(top[0], top[1], top[2])))
                            .bg(profile.convert(Color::Rgb(bottom[0], bottom[1], bottom[2]))),
                    )
                } else {
                    let light = |rgb: [u8; 3]| {
                        299 * u32::from(rgb[0]) + 587 * u32::from(rgb[1]) + 114 * u32::from(rgb[2])
                            >= 128_000
                    };
                    let glyph = match (light(top), light(bottom)) {
                        (false, false) => " ",
                        (true, false) => "\u{2580}",
                        (false, true) => "\u{2584}",
                        (true, true) => "\u{2588}",
                    };
                    Cell::new(glyph, 1)
                };
                cells.set_cell(Position::new(x, y), &cell);
            }
        }
        Ok(cells)
    }
}
