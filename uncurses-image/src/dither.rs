//! Built-in and custom palette dithering.
//!
//! Put an implementation in [`crate::ImageOptions::dither`] through an `Arc`.
//! `None` selects undithered output. The default is [`FloydSteinberg`].
//! Built-ins use Oklab for color matching and diffusion.

use std::{any::Any, borrow::Cow, fmt, io};

use image::RgbImage;
use quantette::{color_map::IndexedColorMap, deps::palette::Oklab};

use crate::{color, quant::PreparedImage};

mod builtins;
pub use builtins::{Bayer8x8, FloydSteinberg, Noise};

/// Sampled image data and destination colors.
///
/// Sixel supplies the complete raster before clipping. Symbols supply an
/// 8-by-8 sample grid per cell. Palette entries are sRGB bytes.
/// Algorithm settings belong to the implementation.
#[derive(Clone, Copy, Debug)]
pub struct DitherContext<'a> {
    pixels: &'a RgbImage,
    palette: &'a [[u8; 3]],
    prepared: Option<&'a PreparedImage<'a>>,
}

impl<'a> DitherContext<'a> {
    /// Use a fixed sRGB palette with nearest-color lookup.
    pub fn new(pixels: &'a RgbImage, palette: &'a [[u8; 3]]) -> Self {
        Self {
            pixels,
            palette,
            prepared: None,
        }
    }

    pub(crate) fn prepared(image: &'a PreparedImage<'a>) -> Self {
        Self {
            pixels: image.pixels(),
            palette: image.palette(),
            prepared: Some(image),
        }
    }

    /// The original RGB pixels.
    pub fn pixels(&self) -> &'a RgbImage {
        self.pixels
    }

    /// The destination colors in palette index order.
    pub fn palette(&self) -> &'a [[u8; 3]] {
        self.palette
    }

    /// Map pixels to the nearest palette colors in Oklab.
    ///
    /// This performs nearest-color matching even when preparation retains a
    /// quantizer-specific lookup.
    pub fn nearest(&self) -> io::Result<Vec<u8>> {
        if let Some(indices) = self.exact_indices() {
            return Ok(indices.to_vec());
        }
        let map = match self.prepared {
            Some(image) => image.mapping.nearest(),
            None => Cow::Owned(color::color_map(self.palette)?),
        };
        Ok(map.map_to_indices(&self.colors()))
    }

    pub(crate) fn exact_indices(&self) -> Option<&[u8]> {
        self.prepared.and_then(PreparedImage::exact_indices)
    }

    pub(crate) fn colors(&self) -> Cow<'_, [Oklab]> {
        match self.prepared {
            Some(image) => Cow::Borrowed(image.colors()),
            None => Cow::Owned(color::colors(self.pixels)),
        }
    }

    pub(crate) fn mapping(&self) -> io::Result<Cow<'_, color::Mapping>> {
        match self.prepared {
            Some(image) => Ok(Cow::Borrowed(&image.mapping)),
            None => color::Mapping::new(self.palette).map(Cow::Owned),
        }
    }
}

/// A repeatable mapping from pixels to palette indices.
///
/// Return one valid index per pixel in row order. The image pipeline validates
/// results before terminal output. Keep configuration immutable while its
/// shared handle is in use. Replace the handle when configuration changes.
pub trait Dither: Any + fmt::Debug + Send + Sync {
    fn dither(&self, context: DitherContext<'_>) -> io::Result<Vec<u8>>;

    /// Validate algorithm settings before image processing.
    fn validate(&self) -> io::Result<()> {
        Ok(())
    }

    /// Whether another implementation has equivalent configuration.
    ///
    /// Override for value-based frame reuse. The pipeline also recognizes
    /// shared-handle identity. An override must define an equivalence relation.
    fn same(&self, _other: &dyn Dither) -> bool {
        false
    }
}

pub(crate) fn map(dither: &dyn Dither, context: DitherContext<'_>) -> io::Result<Vec<u8>> {
    color::validate_palette(context.palette, 256)?;
    dither.validate()?;
    let indices = dither.dither(context)?;
    color::validate_indices(
        &indices,
        context.pixels.as_raw().len() / 3,
        context.palette.len(),
    )?;
    Ok(indices)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) fn test_algorithms() -> [Option<std::sync::Arc<dyn Dither>>; 4] {
    use std::sync::Arc;
    [
        None,
        Some(Arc::new(Bayer8x8::default())),
        Some(Arc::new(FloydSteinberg::default())),
        Some(Arc::new(Noise::default())),
    ]
}
