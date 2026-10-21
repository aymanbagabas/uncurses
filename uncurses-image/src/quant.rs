//! Adaptive palettes and indexed images.
//!
//! [`Wu`] is the default. [`KMeans`] refines a palette with repeatable clustering.
//! Custom implementations supply [`Quantize::palette`] and can override
//! [`Quantize::prepare`] to retain a lookup or [`Quantize::quantize`] for
//! efficient undithered output.

use std::{any::Any, collections::HashMap, fmt, io, sync::OnceLock};

use image::RgbImage;
use quantette::{
    ImageBuf, ImageRef, IndexedImageCounts, PaletteSize,
    color_map::IndexedColorMap,
    color_space::{oklab_to_srgb8, srgb8_to_oklab},
    dedup,
    deps::palette::Oklab,
    kmeans::{Kmeans as KmeansQuantizer, KmeansOptions},
    wu::{BinnerF32x3, WuF32x3},
};

use crate::{ImageOptions, color, dither, invalid};

/// Sampled pixels and the requested palette budget.
#[derive(Clone, Copy, Debug)]
pub struct QuantizeContext<'a> {
    pub pixels: &'a RgbImage,
    /// Maximum palette length, from 1 through 256.
    pub max_colors: u16,
}

impl QuantizeContext<'_> {
    fn validate(&self) -> io::Result<()> {
        if self.pixels.width() == 0 || self.pixels.height() == 0 {
            return Err(invalid("quantization requires a nonempty image"));
        }
        if !(1..=256).contains(&self.max_colors) {
            return Err(invalid("max_colors must be in 1..=256"));
        }
        Ok(())
    }
}

/// A source image, destination palette, and reusable color mapping.
///
/// Obtain this through [`Quantize::prepare`], then pass [`Self::context`] to a
/// [`dither::Dither`]. Converted pixels are shared across dither calls.
/// Custom quantizers can use [`Self::new`] for nearest-color lookup.
#[derive(Debug)]
pub struct PreparedImage<'a> {
    pixels: &'a RgbImage,
    palette: Vec<[u8; 3]>,
    pub(crate) mapping: color::Mapping,
    colors: OnceLock<Vec<Oklab>>,
    exact_indices: Option<Vec<u8>>,
}

impl<'a> PreparedImage<'a> {
    /// Prepare nearest-color lookup for an sRGB palette within the given budget.
    pub fn new(context: QuantizeContext<'a>, palette: Vec<[u8; 3]>) -> io::Result<Self> {
        context.validate()?;
        color::validate_palette(&palette, context.max_colors)?;
        Ok(Self {
            pixels: context.pixels,
            mapping: color::Mapping::new(&palette)?,
            palette,
            colors: OnceLock::new(),
            exact_indices: None,
        })
    }

    /// The original RGB pixels.
    pub fn pixels(&self) -> &RgbImage {
        self.pixels
    }

    /// The destination colors in palette index order.
    pub fn palette(&self) -> &[[u8; 3]] {
        &self.palette
    }

    /// Borrow the prepared data for a built-in or custom dither.
    pub fn context(&self) -> dither::DitherContext<'_> {
        dither::DitherContext::prepared(self)
    }

    pub(crate) fn colors(&self) -> &[Oklab] {
        self.colors.get_or_init(|| color::colors(self.pixels))
    }

    pub(crate) fn exact_indices(&self) -> Option<&[u8]> {
        self.exact_indices.as_deref()
    }

    fn validate(&self, context: QuantizeContext<'_>) -> io::Result<()> {
        if !std::ptr::eq(self.pixels, context.pixels) {
            return Err(invalid("prepared image must use the requested source"));
        }
        color::validate_palette(&self.palette, context.max_colors)
    }

    fn round_sixel_palette(&mut self) -> io::Result<()> {
        if round_sixel_palette(&mut self.palette) {
            self.exact_indices = None;
        }
        self.mapping.replace_palette(&self.palette)
    }
}

/// An sRGB palette and one palette index per pixel, in row order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedImage {
    pub width: u32,
    pub height: u32,
    pub palette: Vec<[u8; 3]>,
    pub indices: Vec<u8>,
}

impl IndexedImage {
    fn validate(&self, context: QuantizeContext<'_>) -> io::Result<()> {
        if (self.width, self.height) != context.pixels.dimensions() {
            return Err(invalid("quantized dimensions must match the source"));
        }
        color::validate_palette(&self.palette, context.max_colors)?;
        color::validate_indices(
            &self.indices,
            context.pixels.as_raw().len() / 3,
            self.palette.len(),
        )
    }
}

/// A repeatable adaptive palette generator.
///
/// Return sRGB colors within the requested budget. The pipeline validates
/// custom results before terminal output. Keep configuration immutable while
/// its shared handle is in use; replace the handle when configuration changes.
pub trait Quantize: Any + fmt::Debug + Send + Sync {
    fn palette(&self, context: QuantizeContext<'_>) -> io::Result<Vec<[u8; 3]>>;

    /// Prepare a palette and lookup for dithering.
    ///
    /// The default uses [`Self::palette`] and nearest-color lookup in Oklab.
    /// Overrides must retain the requested source and honor its palette budget.
    fn prepare<'a>(&self, context: QuantizeContext<'a>) -> io::Result<PreparedImage<'a>> {
        context.validate()?;
        self.validate()?;
        PreparedImage::new(context, self.palette(context)?)
    }

    /// Generate undithered indices. The default uses nearest colors in Oklab.
    fn quantize(&self, context: QuantizeContext<'_>) -> io::Result<IndexedImage> {
        context.validate()?;
        self.validate()?;
        let palette = self.palette(context)?;
        color::validate_palette(&palette, context.max_colors)?;
        let indices = color::nearest(context.pixels, &palette)?;
        Ok(IndexedImage {
            width: context.pixels.width(),
            height: context.pixels.height(),
            palette,
            indices,
        })
    }

    /// Validate algorithm settings before image processing.
    fn validate(&self) -> io::Result<()> {
        Ok(())
    }

    /// Whether another implementation has equivalent configuration.
    ///
    /// Override for value-based frame reuse. The pipeline also recognizes
    /// shared-handle identity. An override must define an equivalence relation.
    fn same(&self, _other: &dyn Quantize) -> bool {
        false
    }
}

/// Wu palette generation in Oklab, with an exact-color fast path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Wu;

/// K-means palette refinement in Oklab.
#[derive(Clone, Debug, PartialEq)]
pub struct KMeans {
    /// Number of training samples relative to the image size. Default 1.
    /// Must be finite and nonnegative.
    pub sampling_factor: f32,
    /// Repeatable random seed. Default 0.
    pub seed: u64,
}

impl Default for KMeans {
    fn default() -> Self {
        Self {
            sampling_factor: 1.0,
            seed: 0,
        }
    }
}

impl KMeans {
    fn settings(&self) -> io::Result<quantette::kmeans::KmeansOptions> {
        self.validate()?;
        Ok(quantette::kmeans::KmeansOptions::new()
            .sampling_factor(self.sampling_factor)
            .seed(self.seed))
    }
}

impl Quantize for Wu {
    fn prepare<'a>(&self, context: QuantizeContext<'a>) -> io::Result<PreparedImage<'a>> {
        builtin(context, None, Output::Prepared)?.prepare(context)
    }
    fn palette(&self, context: QuantizeContext<'_>) -> io::Result<Vec<[u8; 3]>> {
        Ok(builtin(context, None, Output::Palette)?.palette)
    }
    fn quantize(&self, context: QuantizeContext<'_>) -> io::Result<IndexedImage> {
        Ok(builtin(context, None, Output::Indexed)?.indexed(context))
    }
    fn same(&self, other: &dyn Quantize) -> bool {
        color::same(self, other)
    }
}

impl Quantize for KMeans {
    fn prepare<'a>(&self, context: QuantizeContext<'a>) -> io::Result<PreparedImage<'a>> {
        builtin(context, Some(self.settings()?), Output::Prepared)?.prepare(context)
    }
    fn palette(&self, context: QuantizeContext<'_>) -> io::Result<Vec<[u8; 3]>> {
        Ok(builtin(context, Some(self.settings()?), Output::Palette)?.palette)
    }
    fn quantize(&self, context: QuantizeContext<'_>) -> io::Result<IndexedImage> {
        Ok(builtin(context, Some(self.settings()?), Output::Indexed)?.indexed(context))
    }
    fn validate(&self) -> io::Result<()> {
        if !self.sampling_factor.is_finite() || self.sampling_factor < 0.0 {
            return Err(invalid(
                "K-means sampling factor must be finite and nonnegative",
            ));
        }
        Ok(())
    }
    fn same(&self, other: &dyn Quantize) -> bool {
        color::same(self, other)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Output {
    Palette,
    Indexed,
    Prepared,
}

#[derive(Default)]
struct BuiltinOutput {
    palette: Vec<[u8; 3]>,
    indices: Vec<u8>,
    lookup: Option<color::Lookup>,
    colors: Vec<Oklab>,
}

impl BuiltinOutput {
    fn palette(colors: &[Oklab]) -> Self {
        Self {
            palette: oklab_to_srgb8(colors)
                .iter()
                .map(|c| [c.red, c.green, c.blue])
                .collect(),
            ..Default::default()
        }
    }

    fn indexed(self, context: QuantizeContext<'_>) -> IndexedImage {
        IndexedImage {
            width: context.pixels.width(),
            height: context.pixels.height(),
            palette: self.palette,
            indices: self.indices,
        }
    }

    fn prepare(self, context: QuantizeContext<'_>) -> io::Result<PreparedImage<'_>> {
        if let Some(lookup) = self.lookup {
            Ok(PreparedImage {
                pixels: context.pixels,
                mapping: color::Mapping::with_lookup(lookup, &self.palette)?,
                palette: self.palette,
                colors: OnceLock::from(self.colors),
                exact_indices: None,
            })
        } else {
            let mut prepared = PreparedImage::new(context, self.palette)?;
            prepared.exact_indices = Some(self.indices);
            Ok(prepared)
        }
    }
}

enum Samples {
    Pixels(ImageBuf<Oklab>),
    Indexed(IndexedImageCounts<Oklab, u32>),
}

impl Samples {
    fn wu(&self) -> io::Result<WuF32x3<Oklab, 32, 16, 16>> {
        let binner = BinnerF32x3::oklab_from_srgb8();
        match self {
            Self::Pixels(image) => WuF32x3::run_image(image.as_ref(), binner),
            Self::Indexed(image) => WuF32x3::run_indexed_image_counts(image, binner),
        }
        .ok_or_else(|| invalid("cannot quantize an empty image"))
    }

    fn kmeans(
        &self,
        centroids: quantette::PaletteBuf<Oklab>,
        settings: KmeansOptions,
    ) -> KmeansQuantizer<Oklab, f32, 3> {
        match self {
            Self::Pixels(image) => KmeansQuantizer::run_image(image.as_ref(), centroids, settings),
            Self::Indexed(image) => {
                KmeansQuantizer::run_indexed_image(image.as_ref(), centroids, settings)
            }
        }
    }

    fn indices(&self, map: impl IndexedColorMap<Oklab>) -> Vec<u8> {
        match self {
            Self::Pixels(image) => image.as_ref().map_to_indexed(map).into_parts().1,
            Self::Indexed(image) => image.as_ref().map_to_indexed(map).into_parts().1,
        }
    }
}

fn dedup_threshold(output: Output, kmeans: bool, max_colors: u16) -> usize {
    // Match Quantette's palette and undithered image policies.
    match output {
        Output::Prepared => usize::MAX,
        Output::Indexed if kmeans => {
            let pixels = if cfg!(target_feature = "avx2") {
                4096 * 4096
            } else {
                1024 * 1024 * 2
            };
            pixels / usize::from(max_colors / 8).max(1)
        }
        _ => 2048 * 2048,
    }
}

fn builtin(
    context: QuantizeContext<'_>,
    kmeans: Option<KmeansOptions>,
    output: Output,
) -> io::Result<BuiltinOutput> {
    context.validate()?;
    if let Some((palette, indices)) = exact(context, output != Output::Palette) {
        return Ok(BuiltinOutput {
            palette,
            indices,
            ..Default::default()
        });
    }
    let k: PaletteSize = context
        .max_colors
        .try_into()
        .map_err(|e: quantette::PaletteSizeFromIntError| invalid(&e.to_string()))?;
    let samples = if context.pixels.pixels().len()
        >= dedup_threshold(output, kmeans.is_some(), context.max_colors)
    {
        let rgb = color::rgb_colors(context.pixels);
        if output == Output::Palette && kmeans.is_none() {
            // Palette-only Wu needs color counts, not a full array of indices.
            let counts = dedup::dedup_colors_u8_3_counts(&rgb)
                .map_err(|e| invalid(&e.to_string()))?
                .map(|palette| srgb8_to_oklab(&palette));
            let wu = WuF32x3::run_palette_counts(&counts, BinnerF32x3::oklab_from_srgb8())
                .ok_or_else(|| invalid("cannot quantize an empty image"))?;
            return Ok(BuiltinOutput::palette(&wu.palette(k)));
        }
        let image = ImageRef::new(context.pixels.width(), context.pixels.height(), &rgb)
            .map_err(|e| invalid(&e.to_string()))?;
        Samples::Indexed(
            dedup::dedup_image_u8_3_counts(image).map(|palette| srgb8_to_oklab(&palette)),
        )
    } else {
        Samples::Pixels(
            ImageBuf::new(
                context.pixels.width(),
                context.pixels.height(),
                color::colors(context.pixels),
            )
            .map_err(|e| invalid(&e.to_string()))?,
        )
    };
    let wu = samples.wu()?;
    let lookup = if let Some(settings) = kmeans {
        let result = samples.kmeans(wu.palette(k), settings);
        if output == Output::Palette {
            return Ok(BuiltinOutput::palette(&result.into_palette()));
        }
        color::Lookup::Nearest(result.into_color_map())
    } else {
        if output == Output::Palette {
            return Ok(BuiltinOutput::palette(&wu.palette(k)));
        }
        color::Lookup::Wu(wu.color_map(k))
    };
    let mut result = BuiltinOutput::palette(lookup.palette());
    if output == Output::Indexed {
        result.indices = match lookup {
            color::Lookup::Nearest(map) => samples.indices(map),
            color::Lookup::Wu(map) => samples.indices(map),
        };
    } else {
        result.lookup = Some(lookup);
        if let Samples::Pixels(image) = samples {
            result.colors = image.into_inner();
        }
    }
    Ok(result)
}

fn exact(context: QuantizeContext<'_>, indexed: bool) -> Option<(Vec<[u8; 3]>, Vec<u8>)> {
    let mut colors = HashMap::new();
    let mut palette = Vec::new();
    let mut indices = if indexed {
        Vec::with_capacity(context.pixels.as_raw().len() / 3)
    } else {
        Vec::new()
    };
    let mut last = None;
    for pixel in context.pixels.pixels() {
        let index = if let Some((previous, index)) = last
            && previous == pixel.0
        {
            index
        } else {
            let next = colors.len();
            if next == usize::from(context.max_colors) && !colors.contains_key(&pixel.0) {
                return None;
            }
            let index = *colors.entry(pixel.0).or_insert(next as u8);
            if usize::from(index) == palette.len() {
                palette.push(pixel.0);
            }
            last = Some((pixel.0, index));
            index
        };
        if indexed {
            indices.push(index);
        }
    }
    Some((palette, indices))
}

pub(crate) fn sixel(pixels: &RgbImage, options: &ImageOptions) -> io::Result<IndexedImage> {
    options.validate_color()?;
    let context = QuantizeContext {
        pixels,
        max_colors: options.max_colors,
    };
    context.validate()?;
    if let Some(algorithm) = &options.dither {
        let mut prepared = options.quantizer.prepare(context)?;
        prepared.validate(context)?;
        prepared.round_sixel_palette()?;
        let indices = dither::map(algorithm.as_ref(), prepared.context())?;
        return Ok(IndexedImage {
            width: pixels.width(),
            height: pixels.height(),
            palette: prepared.palette,
            indices,
        });
    }
    let mut image = options.quantizer.quantize(context)?;
    image.validate(context)?;
    round_sixel_palette(&mut image.palette);
    Ok(image)
}

fn round_sixel_palette(palette: &mut [[u8; 3]]) -> bool {
    let mut changed = false;
    for color in palette {
        for channel in color {
            let percent = (u32::from(*channel) * 100 + 127) / 255;
            let rounded = ((percent * 255 + 50) / 100) as u8;
            changed |= *channel != rounded;
            *channel = rounded;
        }
    }
    changed
}

#[cfg(test)]
mod tests;
