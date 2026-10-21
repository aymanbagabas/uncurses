//! Image geometry and sampling options.

use crate::{
    dither::{Dither, FloydSteinberg},
    quant::{Quantize, Wu},
};
use ::image::imageops::FilterType;
use std::{io, sync::Arc};

/// How the source occupies the destination rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fit {
    /// Preserve the aspect ratio and fit the whole image inside the rectangle.
    #[default]
    Contain,
    /// Preserve the aspect ratio and crop the excess to fill the rectangle.
    Cover,
    /// Resize independently along both axes to fill the rectangle.
    Stretch,
    /// Use source pixel dimensions before applying the scale factor.
    Original,
}

/// A source rectangle in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Image geometry and sampling settings.
///
/// Options remain unchanged when passed to an image method. Images are centered
/// in the destination. Dimensions refer to physical pixels, not square cells.
#[derive(Clone, Debug)]
pub struct ImageOptions {
    /// The fit mode. Defaults to containment.
    pub fit: Fit,
    /// Uniform scale after fitting. Must be finite and greater than zero.
    /// Values above one zoom in; content outside the destination is cropped.
    pub scale: f64,
    /// Override the source width:height ratio. Both components must be nonzero.
    /// Stretch uses the destination ratio instead.
    pub aspect_ratio: Option<(u32, u32)>,
    /// Select part of the source before fitting.
    pub crop: Option<Crop>,
    /// Cell width and height in pixels. Both must be nonzero.
    /// Defaults to an explicit 8x16 estimate. Use reported terminal dimensions
    /// for pixel-accurate native graphics.
    pub cell_pixels: (u16, u16),
    /// Color for transparent source pixels and unused destination space.
    pub background: [u8; 3],
    /// Resampling filter. Defaults to triangle filtering.
    pub filter: FilterType,
    /// Palette dithering for Sixel and indexed or monochrome symbols.
    /// Full-color Kitty and iTerm2 output uses the original raster.
    pub dither: Option<Arc<dyn Dither>>,
    /// Adaptive palette generator for Sixel. Defaults to [`Wu`].
    pub quantizer: Arc<dyn Quantize>,
    /// Maximum adaptive palette length, in 1..=256. Defaults to 256.
    pub max_colors: u16,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            fit: Fit::Contain,
            scale: 1.0,
            aspect_ratio: None,
            crop: None,
            cell_pixels: (8, 16),
            background: [0, 0, 0],
            filter: FilterType::Triangle,
            dither: Some(Arc::new(FloydSteinberg::default())),
            quantizer: Arc::new(Wu),
            max_colors: 256,
        }
    }
}

impl ImageOptions {
    pub(crate) fn validate_color(&self) -> io::Result<()> {
        if !(1..=256).contains(&self.max_colors) {
            return Err(crate::invalid("max_colors must be in 1..=256"));
        }
        self.quantizer.validate()?;
        if let Some(dither) = &self.dither {
            dither.validate()?;
        }
        Ok(())
    }
    pub(crate) fn same_raster(&self, other: &Self) -> bool {
        self.fit == other.fit
            && self.scale == other.scale
            && self.aspect_ratio == other.aspect_ratio
            && self.crop == other.crop
            && self.cell_pixels == other.cell_pixels
            && self.background == other.background
            && self.filter == other.filter
    }
}

impl PartialEq for ImageOptions {
    fn eq(&self, other: &Self) -> bool {
        self.same_raster(other)
            && self.max_colors == other.max_colors
            && (Arc::ptr_eq(&self.quantizer, &other.quantizer)
                || self.quantizer.same(other.quantizer.as_ref()))
            && match (&self.dither, &other.dither) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a.same(b.as_ref()),
                _ => false,
            }
    }
}
