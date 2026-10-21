//! Stateless algorithms with configuration owned by each implementation.

use std::io;

use quantette::{ImageRef, color_map::IndexedColorMap, deps::palette::Oklab};

use super::{Dither, DitherContext};
use crate::{color, invalid};

/// Serpentine Floyd-Steinberg diffusion in Oklab.
#[derive(Clone, Debug, PartialEq)]
pub struct FloydSteinberg {
    /// Fraction of error propagated, in 0..=1. Defaults to 0.875.
    pub strength: f32,
    /// Grain dimensions in samples. Each axis accepts 1, 2, 4, or 8.
    /// Defaults to (1, 1). Each grain uses its mean Oklab color.
    pub grain: (u8, u8),
}

impl Default for FloydSteinberg {
    fn default() -> Self {
        Self {
            strength: 0.875,
            grain: (1, 1),
        }
    }
}

/// An 8-by-8 Bayer threshold pattern.
#[derive(Clone, Debug, PartialEq)]
pub struct Bayer8x8 {
    /// Finite, nonnegative perturbation strength. Defaults to 1.
    pub intensity: f32,
    /// Grain dimensions in samples. Axes accept 1, 2, 4, or 8; default (1, 1).
    pub grain: (u8, u8),
}

impl Default for Bayer8x8 {
    fn default() -> Self {
        Self {
            intensity: 1.0,
            grain: (1, 1),
        }
    }
}

/// Repeatable coordinate-based noise.
#[derive(Clone, Debug, PartialEq)]
pub struct Noise {
    /// Finite, nonnegative perturbation strength. Defaults to 1.
    pub intensity: f32,
    /// Grain dimensions in samples. Axes accept 1, 2, 4, or 8; default (1, 1).
    pub grain: (u8, u8),
}

impl Default for Noise {
    fn default() -> Self {
        Self {
            intensity: 1.0,
            grain: (1, 1),
        }
    }
}

fn validate(intensity: f32, grain: (u8, u8)) -> io::Result<()> {
    if !intensity.is_finite() || intensity < 0.0 {
        return Err(invalid("dither strength must be finite and nonnegative"));
    }
    if !matches!(grain.0, 1 | 2 | 4 | 8) || !matches!(grain.1, 1 | 2 | 4 | 8) {
        return Err(invalid("dither grain axes must be 1, 2, 4, or 8"));
    }
    Ok(())
}

impl Dither for Bayer8x8 {
    fn dither(&self, context: DitherContext<'_>) -> io::Result<Vec<u8>> {
        self.validate()?;
        perturb(context, self.grain, self.intensity, |x, y, _| {
            (f64::from(bayer(x, y)) + 0.5) / 64.0 - 0.5
        })
    }
    fn validate(&self) -> io::Result<()> {
        validate(self.intensity, self.grain)
    }
    fn same(&self, other: &dyn Dither) -> bool {
        color::same(self, other)
    }
}

impl Dither for Noise {
    fn dither(&self, context: DitherContext<'_>) -> io::Result<Vec<u8>> {
        self.validate()?;
        perturb(context, self.grain, self.intensity, noise)
    }
    fn validate(&self) -> io::Result<()> {
        validate(self.intensity, self.grain)
    }
    fn same(&self, other: &dyn Dither) -> bool {
        color::same(self, other)
    }
}

impl Dither for FloydSteinberg {
    fn validate(&self) -> io::Result<()> {
        validate(self.strength, self.grain)?;
        if self.strength > 1.0 {
            return Err(invalid("Floyd-Steinberg strength must be in 0..=1"));
        }
        Ok(())
    }
    fn same(&self, other: &dyn Dither) -> bool {
        color::same(self, other)
    }

    fn dither(&self, context: DitherContext<'_>) -> io::Result<Vec<u8>> {
        self.validate()?;
        if self.strength == 0.0 {
            return context.nearest();
        }
        if self.grain == (1, 1)
            && let Some(indices) = context.exact_indices()
        {
            // Exact destination colors produce zero diffusion error.
            return Ok(indices.to_vec());
        }
        let map = context.mapping()?;
        let colors = context.colors();
        let (width, height) = context.pixels.dimensions();
        let (gw, gh) = (u32::from(self.grain.0), u32::from(self.grain.1));
        let (columns, rows) = (width.div_ceil(gw), height.div_ceil(gh));
        let grains;
        let samples: &[Oklab] = if self.grain == (1, 1) {
            &colors
        } else {
            grains = (0..rows)
                .flat_map(|y| {
                    let colors = &colors;
                    (0..columns).map(move |x| {
                        let mut sum = Oklab::new(0.0, 0.0, 0.0);
                        let mut count = 0;
                        for yy in y * gh..((y + 1) * gh).min(height) {
                            for xx in x * gw..((x + 1) * gw).min(width) {
                                let c = colors[(yy * width + xx) as usize];
                                sum.l += c.l;
                                sum.a += c.a;
                                sum.b += c.b;
                                count += 1;
                            }
                        }
                        Oklab::new(
                            sum.l / count as f32,
                            sum.a / count as f32,
                            sum.b / count as f32,
                        )
                    })
                })
                .collect::<Vec<_>>();
            &grains
        };
        let input = ImageRef::new(columns, rows, samples).map_err(|e| invalid(&e.to_string()))?;
        let algorithm = quantette::dither::FloydSteinberg::with_error_diffusion(self.strength)
            .ok_or_else(|| invalid("invalid Floyd-Steinberg strength"))?;
        let indices = map.diffuse(input, algorithm);
        if self.grain == (1, 1) {
            return Ok(indices);
        }
        Ok((0..height)
            .flat_map(|y| {
                let indices = &indices;
                (0..width).map(move |x| indices[((y / gh) * columns + x / gw) as usize])
            })
            .collect())
    }
}

fn perturb(
    context: DitherContext<'_>,
    grain: (u8, u8),
    intensity: f32,
    offset: impl Fn(u32, u32, u32) -> f64,
) -> io::Result<Vec<u8>> {
    if intensity == 0.0 {
        return context.nearest();
    }
    let map = context.mapping()?;
    let amplitude =
        1.0 / ((context.palette.len() as f64).cbrt().max(2.0) - 1.0) * f64::from(intensity);
    Ok(context
        .colors()
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let x = i as u32 % context.pixels.width() / u32::from(grain.0);
            let y = i as u32 / context.pixels.width() / u32::from(grain.1);
            let adjusted = |value: f32, channel, low, high| {
                (f64::from(value) + offset(x, y, channel) * amplitude).clamp(low, high) as f32
            };
            // Oklab lightness spans 0..1; chromatic axes use signed coordinates.
            let c = Oklab::new(
                adjusted(c.l, 0, 0.0, 1.0),
                adjusted(c.a, 1, -0.5, 0.5),
                adjusted(c.b, 2, -0.5, 0.5),
            );
            map.palette_index(&c)
        })
        .collect())
}

fn bayer(x: u32, y: u32) -> u8 {
    let mut value = 0;
    for bit in 0..3 {
        value = value * 4 + ((((x >> bit) ^ (y >> bit)) & 1) * 2 + ((y >> bit) & 1)) as u8;
    }
    value
}

fn noise(x: u32, y: u32, channel: u32) -> f64 {
    let mut value = x.wrapping_mul(0x9e3779b9)
        ^ y.wrapping_mul(0x85ebca6b)
        ^ channel.wrapping_add(1).wrapping_mul(0xc2b2ae35);
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846ca68b);
    value ^= value >> 16;
    f64::from(value) / f64::from(u32::MAX) - 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bayer_matrix_has_all_thresholds() {
        let values: std::collections::BTreeSet<_> = (0..8)
            .flat_map(|y| (0..8).map(move |x| bayer(x, y)))
            .collect();
        assert_eq!(values, (0..64).collect());
        assert_eq!(
            [bayer(0, 0), bayer(1, 0), bayer(0, 1), bayer(1, 1)],
            [0, 32, 48, 16]
        );
    }
}
