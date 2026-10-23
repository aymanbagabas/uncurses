//! Bounded image decoding and immutable source pixels.

use std::{
    fs::File,
    io::{self, Read},
    path::Path,
    sync::Arc,
};

use image::{DynamicImage, ImageDecoder, Rgb, RgbImage, RgbaImage, imageops, metadata::Cicp};
use uncurses::layout::Size;

use crate::{Backend, ImageOptions, PreparedImage, geometry, invalid};

/// Immutable RGBA pixels shared by prepared image blocks.
///
/// Source images accept at most 16,777,216 pixels and 16,384 pixels per axis.
/// Decoding reserves space for decoded pixels, RGBA conversion, and orientation
/// within a 256 MiB budget. Decoder-specific working memory remains subject to
/// the image library's best-effort limits.
/// Rasterization counts the complete retained source, even with a smaller crop.
/// CICP-tagged decoded pixels use the image library's sRGB conversion.
#[derive(Clone, Debug)]
pub struct Image {
    pixels: Arc<RgbaImage>,
}

impl Image {
    pub(crate) fn same_source(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.pixels, &other.pixels)
    }

    /// Maximum encoded file length: 32 MiB.
    pub const MAX_ENCODED_BYTES: u64 = 32 * 1024 * 1024;
    /// Maximum source pixel count.
    pub const MAX_PIXELS: u64 = 16 * 1024 * 1024;
    /// Maximum raster pixel count.
    pub const MAX_RASTER_PIXELS: u64 = 4 * 1024 * 1024;
    /// Budget for explicit decode and raster buffers: 256 MiB.
    pub const MAX_BUFFER_BYTES: u64 = 256 * 1024 * 1024;
    /// Maximum source or raster dimension in pixels.
    pub const MAX_DIMENSION: u32 = 16_384;

    /// Adopt decoded pixels and convert them to sRGB RGBA.
    ///
    /// Zero dimensions, oversized dimensions, and excessive conversion buffers
    /// return `InvalidInput` before conversion.
    /// Supported CICP color spaces use color conversion before alpha compositing.
    /// Unsupported color spaces return `Unsupported`.
    /// The budget includes the source and the converted RGBA buffer.
    pub fn new(image: DynamicImage) -> io::Result<Self> {
        Ok(Self {
            pixels: Arc::new(Self::srgb_pixels(image)?),
        })
    }

    fn srgb_pixels(mut image: DynamicImage) -> io::Result<RgbaImage> {
        let count = pixel_count(image.width(), image.height(), Self::MAX_PIXELS)?;
        check_budget(image.as_bytes().len() as u64 + count * 4)?;
        if image.color_space() != Cicp::SRGB {
            image
                .convert_color_space(Cicp::SRGB, Default::default(), image::ColorType::Rgba8)
                .map_err(|error| io::Error::new(io::ErrorKind::Unsupported, error))?;
        }
        Ok(image.into_rgba8())
    }

    /// Convert tightly packed sRGB bytes in RGB row order.
    ///
    /// The byte count must equal `width * height * 3`.
    pub fn from_rgb(width: u32, height: u32, pixels: Vec<u8>) -> io::Result<Self> {
        let count = pixel_count(width, height, Self::MAX_PIXELS)?;
        if pixels.len() as u64 != count * 3 {
            return Err(invalid("RGB data must match the image dimensions"));
        }
        let pixels = RgbImage::from_raw(width, height, pixels)
            .ok_or_else(|| invalid("invalid RGB buffer"))?;
        Self::new(DynamicImage::ImageRgb8(pixels))
    }

    /// Adopt tightly packed sRGB bytes in RGBA row order.
    ///
    /// The byte count must equal `width * height * 4`.
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> io::Result<Self> {
        let count = pixel_count(width, height, Self::MAX_PIXELS)?;
        if pixels.len() as u64 != count * 4 {
            return Err(invalid("RGBA data must match the image dimensions"));
        }
        let pixels = RgbaImage::from_raw(width, height, pixels)
            .ok_or_else(|| invalid("invalid RGBA buffer"))?;
        Ok(Self {
            pixels: Arc::new(pixels),
        })
    }

    /// Decode a file and apply its EXIF orientation.
    ///
    /// Reads at most [`Self::MAX_ENCODED_BYTES`] plus one byte before rejection.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        if file.metadata()?.len() > Self::MAX_ENCODED_BYTES {
            return Err(invalid("encoded image exceeds the input limit"));
        }
        let mut bytes = Vec::new();
        file.take(Self::MAX_ENCODED_BYTES + 1)
            .read_to_end(&mut bytes)?;
        Self::decode(&bytes)
    }

    /// Decode bytes with the enabled `image` formats and apply EXIF orientation.
    ///
    /// PNG CICP tags select the source color space before conversion to sRGB.
    /// Conversion preserves the decoded sample precision until the final RGBA8 output.
    /// Unsupported CICP values return `Unsupported`; corrupt tags return an input error.
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        Self::decode_format(bytes, None)
    }

    /// Decode bytes with an explicit image format and apply EXIF orientation.
    ///
    /// The enabled format features and all [`decode`](Self::decode) limits apply.
    ///
    /// ```
    /// use uncurses_images::{Image, ImageFormat};
    ///
    /// let mut png = std::io::Cursor::new(Vec::new());
    /// image::DynamicImage::new_rgb8(2, 3).write_to(&mut png, ImageFormat::Png)?;
    /// let decoded = Image::decode_with_format(png.get_ref(), ImageFormat::Png)?;
    /// assert_eq!(decoded.dimensions(), (2, 3));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn decode_with_format(bytes: &[u8], format: image::ImageFormat) -> io::Result<Self> {
        Self::decode_format(bytes, Some(format))
    }

    fn decode_format(bytes: &[u8], format: Option<image::ImageFormat>) -> io::Result<Self> {
        if bytes.len() as u64 > Self::MAX_ENCODED_BYTES {
            return Err(invalid("encoded image exceeds the input limit"));
        }
        let color_space = png_color_space(bytes)?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(Self::MAX_DIMENSION);
        limits.max_image_height = Some(Self::MAX_DIMENSION);
        limits.max_alloc = Some(Self::MAX_BUFFER_BYTES);
        let mut reader = match format {
            Some(format) => image::ImageReader::with_format(io::Cursor::new(bytes), format),
            None => image::ImageReader::new(io::Cursor::new(bytes)).with_guessed_format()?,
        };
        reader.limits(limits.clone());
        let decoder = reader.into_decoder().map_err(io::Error::other)?;
        Self::decode_pixels(decoder, limits, bytes.len() as u64, color_space)
    }

    fn decode_pixels(
        mut decoder: impl ImageDecoder,
        mut limits: image::Limits,
        encoded_bytes: u64,
        color_space: Option<Cicp>,
    ) -> io::Result<Self> {
        let (width, height) = decoder.dimensions();
        let count = pixel_count(width, height, Self::MAX_PIXELS)?;
        // Reserve the output explicitly: into_decoder does not reserve it.
        // RGBA conversion and rotated output can each require another buffer.
        limits
            .reserve(decoder.total_bytes())
            .and_then(|()| limits.reserve(count * 8))
            .and_then(|()| limits.reserve(encoded_bytes * 2))
            .map_err(io::Error::other)?;
        decoder.set_limits(limits).map_err(io::Error::other)?;
        let orientation = decoder.orientation().map_err(io::Error::other)?;
        let mut decoded = DynamicImage::from_decoder(decoder).map_err(io::Error::other)?;
        if let Some(color_space) = color_space {
            decoded
                .set_color_space(color_space)
                .map_err(|error| io::Error::new(io::ErrorKind::Unsupported, error))?;
        }
        let mut rgba = DynamicImage::ImageRgba8(Self::srgb_pixels(decoded)?);
        rgba.apply_orientation(orientation);
        Self::new(rgba)
    }

    /// Source dimensions in pixels, after orientation.
    pub fn dimensions(&self) -> (u32, u32) {
        self.pixels.dimensions()
    }

    /// Borrow the immutable source pixels.
    pub fn pixels(&self) -> &RgbaImage {
        &self.pixels
    }

    /// Rasterize at the supplied physical cell resolution.
    ///
    /// Transparent pixels and unused space use `options.background`.
    /// The raster accepts at most [`Self::MAX_RASTER_PIXELS`] pixels.
    /// The buffer budget includes retained source pixels, the opaque crop,
    /// the destination, the resize result, the float intermediate, and filter weights.
    /// Options remain unchanged.
    pub fn rasterize(
        &self,
        size: Size,
        cell_pixels: Size,
        options: &ImageOptions,
    ) -> io::Result<RgbImage> {
        let width = u32::from(size.width) * u32::from(cell_pixels.width);
        let height = u32::from(size.height) * u32::from(cell_pixels.height);
        let count = pixel_count(width, height, Self::MAX_RASTER_PIXELS)?;
        let geometry = geometry::resolve(self.dimensions(), size, (width, height), options)?;
        self.sample_geometry((width, height), count, geometry, options)
    }

    pub(crate) fn sample(
        &self,
        size: Size,
        options: &ImageOptions,
        samples: (u32, u32),
    ) -> io::Result<RgbImage> {
        let count = pixel_count(samples.0, samples.1, Self::MAX_RASTER_PIXELS)?;
        let canvas = (
            u32::from(size.width) * u32::from(options.cell_pixels.0),
            u32::from(size.height) * u32::from(options.cell_pixels.1),
        );
        let geometry =
            geometry::resolve_with_canvas(self.dimensions(), size, samples, canvas, options)?;
        self.sample_geometry(samples, count, geometry, options)
    }

    fn sample_geometry(
        &self,
        (width, height): (u32, u32),
        count: u64,
        geometry: geometry::Geometry,
        options: &ImageOptions,
    ) -> io::Result<RgbImage> {
        let source = geometry.source;
        check_raster_budget(self.pixels.as_raw().len() as u64, count, &geometry)?;
        let mut result = RgbImage::from_pixel(width, height, Rgb(options.background));
        // Composite before filtering so transparent RGB does not create fringes.
        let opaque = RgbImage::from_fn(source.width, source.height, |x, y| {
            let rgba = self.pixels.get_pixel(source.x + x, source.y + y).0;
            let alpha = u32::from(rgba[3]);
            Rgb(std::array::from_fn(|i| {
                ((u32::from(rgba[i]) * alpha
                    + u32::from(options.background[i]) * (255 - alpha)
                    + 127)
                    / 255) as u8
            }))
        });
        let pixels = imageops::resize(&opaque, geometry.width, geometry.height, options.filter);
        imageops::replace(
            &mut result,
            &pixels,
            i64::from(geometry.x),
            i64::from(geometry.y),
        );
        Ok(result)
    }

    /// Prepare a native image payload for an explicit terminal protocol.
    ///
    /// `size` is the cell footprint. `cell_pixels` is the physical cell size.
    /// Use the prepared placement options with `Screen::set_placement`.
    /// The screen manages shared uploads and deletion.
    /// iTerm2 and Sixel require a trailing viewport row below the block.
    ///
    /// ```
    /// use uncurses_images::{Backend, Image, ImageOptions};
    ///
    /// let image = Image::from_rgb(1, 1, vec![255, 0, 0])?;
    /// for backend in [Backend::Kitty, Backend::Iterm2, Backend::Sixel] {
    ///     let block = image.prepare(
    ///         backend, (8, 4).into(), (8, 16).into(), &ImageOptions::default(),
    ///     )?;
    ///     assert_eq!(block.size(), (8, 4).into());
    /// }
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn prepare(
        &self,
        backend: Backend,
        size: Size,
        cell_pixels: Size,
        options: &ImageOptions,
    ) -> io::Result<PreparedImage> {
        let pixels = self.rasterize(size, cell_pixels, options)?;
        crate::native::prepare(backend, pixels, size, options)
    }
}

fn pixel_count(width: u32, height: u32, maximum: u64) -> io::Result<u64> {
    let count = u64::from(width) * u64::from(height);
    if width == 0
        || height == 0
        || width > Image::MAX_DIMENSION
        || height > Image::MAX_DIMENSION
        || count > maximum
    {
        return Err(invalid("image dimensions exceed the supported range"));
    }
    Ok(count)
}

fn check_budget(bytes: u64) -> io::Result<()> {
    if bytes > Image::MAX_BUFFER_BYTES {
        return Err(invalid("image buffers exceed the allocation limit"));
    }
    Ok(())
}

fn check_raster_budget(
    retained_bytes: u64,
    canvas_pixels: u64,
    geometry: &geometry::Geometry,
) -> io::Result<()> {
    let source = geometry.source;
    let source_count = u64::from(source.width) * u64::from(source.height);
    let filter_bytes = u64::from(source.width.max(source.height).next_power_of_two().max(4)) * 4;
    // Resize uses a float intermediate with source width and target height.
    check_budget(
        retained_bytes
            + source_count * 3
            + canvas_pixels * 6
            + u64::from(source.width) * u64::from(geometry.height) * 16
            + filter_bytes,
    )
}

fn png_color_space(bytes: &[u8]) -> io::Result<Option<Cicp>> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(None);
    }
    let mut color_space = None;
    let mut pixels_started = false;
    let mut chunks = &bytes[8..];
    while chunks.len() >= 12 {
        let length = u32::from_be_bytes(chunks[..4].try_into().unwrap()) as usize;
        let Some(chunk_length) = length.checked_add(12).filter(|&size| size <= chunks.len()) else {
            break;
        };
        if &chunks[4..8] == b"cICP" {
            if length != 4 || color_space.is_some() || pixels_started {
                return Err(invalid(
                    "PNG cICP must occur once before image data with four bytes",
                ));
            }
            if png_crc32(&chunks[4..12]) != u32::from_be_bytes(chunks[12..16].try_into().unwrap()) {
                return Err(invalid("PNG cICP checksum mismatch"));
            }
            color_space = Some(cicp_from_bytes(chunks[8..12].try_into().unwrap())?);
        }
        pixels_started |= &chunks[4..8] == b"IDAT";
        if &chunks[4..8] == b"IEND" {
            break;
        }
        chunks = &chunks[chunk_length..];
    }
    Ok(color_space)
}

fn png_crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

fn cicp_from_bytes([primaries, transfer, matrix, full_range]: [u8; 4]) -> io::Result<Cicp> {
    use image::metadata::{CicpColorPrimaries as P, CicpTransferCharacteristics as T};
    let unsupported = || {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported PNG CICP color space",
        )
    };
    if matrix != 0 || full_range != 1 {
        return Err(unsupported());
    }
    // image 0.25 exposes neither decoder CICP metadata nor byte-to-enum conversion.
    // Preserve PNG's tag explicitly until the decoder provides this information.
    let primaries = [
        P::SRgb,
        P::Unspecified,
        P::RgbM,
        P::RgbB,
        P::Bt601,
        P::Rgb240m,
        P::GenericFilm,
        P::Rgb2020,
        P::Xyz,
        P::SmpteRp431,
        P::SmpteRp432,
        P::Industry22,
    ]
    .into_iter()
    .find(|&value| value as u8 == primaries)
    .ok_or_else(unsupported)?;
    let transfer = [
        T::Bt709,
        T::Unspecified,
        T::Bt470M,
        T::Bt470BG,
        T::Bt601,
        T::Smpte240m,
        T::Linear,
        T::Log100,
        T::LogSqrt,
        T::Iec61966_2_4,
        T::Bt1361,
        T::SRgb,
        T::Bt2020_10bit,
        T::Bt2020_12bit,
        T::Smpte2084,
        T::Smpte428,
        T::Bt2100Hlg,
    ]
    .into_iter()
    .find(|&value| value as u8 == transfer)
    .ok_or_else(unsupported)?;
    Ok(Cicp {
        primaries,
        transfer,
        ..Cicp::SRGB
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Crop, Fit};

    #[test]
    fn raster_budget_includes_retained_source_pixels() {
        for crop in [
            None,
            Some(Crop {
                x: 0,
                y: 0,
                width: 4096,
                height: 2048,
            }),
        ] {
            let geometry = geometry::resolve(
                (4096, 4096),
                (1024, 3000).into(),
                (1024, 3000),
                &ImageOptions {
                    fit: Fit::Stretch,
                    crop,
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(check_raster_budget(4096 * 4096 * 4, 1024 * 3000, &geometry).is_err());
        }
        for (height, fits) in [(2106, true), (2107, false)] {
            let geometry = geometry::resolve(
                (4096, 4096),
                Size::new(1024, height),
                (1024, u32::from(height)),
                &ImageOptions {
                    fit: Fit::Stretch,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(
                check_raster_budget(4096 * 4096 * 4, 1024 * u64::from(height), &geometry).is_ok(),
                fits,
            );
        }
    }

    #[test]
    fn linear_rgb_is_converted_to_srgb_before_compositing() {
        let mut source = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([128, 128, 128, 128]),
        ));
        source
            .set_color_space(image::metadata::Cicp::SRGB_LINEAR)
            .unwrap();
        let image = Image::new(source).unwrap();
        assert_eq!(image.pixels().get_pixel(0, 0).0, [188, 188, 188, 128]);
        assert_eq!(image.pixels().color_space(), image::metadata::Cicp::SRGB);
        let raster = image
            .rasterize((1, 1).into(), (1, 1).into(), &ImageOptions::default())
            .unwrap();
        assert_eq!(raster.get_pixel(0, 0).0, [94, 94, 94]);
    }

    #[test]
    fn srgb_pixels_preserve_channels_and_linear_floats_preserve_precision() {
        let source =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, image::Rgba([128, 64, 32, 17])));
        assert_eq!(
            Image::new(source).unwrap().pixels().get_pixel(0, 0).0,
            [128, 64, 32, 17]
        );
        let mut linear = DynamicImage::ImageRgba32F(image::Rgba32FImage::from_pixel(
            1,
            1,
            image::Rgba([0.003, 0.003, 0.003, 0.5]),
        ));
        linear.set_color_space(Cicp::SRGB_LINEAR).unwrap();
        assert_eq!(
            Image::new(linear).unwrap().pixels().get_pixel(0, 0).0,
            [10, 10, 10, 128]
        );
    }

    #[test]
    fn png_srgb_tag_remains_supported() {
        let mut png = io::Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(RgbImage::from_pixel(1, 1, Rgb([128; 3])))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let mut png = png.into_inner();
        png.splice(33..33, b"\0\0\0\x01sRGB\0\xae\xce\x1c\xe9".iter().copied());
        assert_eq!(
            Image::decode(&png).unwrap().pixels().get_pixel(0, 0).0,
            [128, 128, 128, 255]
        );
    }

    #[test]
    fn encoded_png_linear_cicp_converts_before_compositing() {
        let mut png = io::Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([128, 128, 128, 128]),
        ))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
        let mut png = png.into_inner();
        png.splice(
            33..33,
            [
                0, 0, 0, 4, b'c', b'I', b'C', b'P', 1, 8, 0, 1, 154, 162, 249, 217,
            ],
        );
        for decoded in [
            Image::decode(&png).unwrap(),
            Image::decode_with_format(&png, image::ImageFormat::Png).unwrap(),
        ] {
            assert_eq!(decoded.pixels().get_pixel(0, 0).0, [188, 188, 188, 128]);
            assert_eq!(
                decoded
                    .rasterize((1, 1).into(), (1, 1).into(), &ImageOptions::default())
                    .unwrap()
                    .get_pixel(0, 0)
                    .0,
                [94; 3],
            );
        }
        let path = format!("uncurses-cicp-{}.png", std::process::id());
        std::fs::write(&path, &png).unwrap();
        let decoded = Image::open(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            decoded.unwrap().pixels().get_pixel(0, 0).0,
            [188, 188, 188, 128]
        );
    }

    fn png_with_cicp(source: DynamicImage, tag: [u8; 4]) -> Vec<u8> {
        let mut png = io::Cursor::new(Vec::new());
        source.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let mut chunk = vec![0, 0, 0, 4, b'c', b'I', b'C', b'P'];
        chunk.extend(tag);
        chunk.extend(png_crc32(&chunk[4..]).to_be_bytes());
        let mut png = png.into_inner();
        png.splice(33..33, chunk);
        png
    }

    #[test]
    fn encoded_png_wide_gamut_and_sixteen_bit_precision_are_preserved() {
        let source = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([200, 100, 50, 128]),
        ));
        let png = png_with_cicp(source.clone(), [12, 13, 0, 1]);
        let mut expected = source;
        expected.set_color_space(Cicp::DISPLAY_P3).unwrap();
        let expected = Image::new(expected).unwrap();
        assert_ne!(expected.pixels().get_pixel(0, 0).0, [200, 100, 50, 128]);
        for decoded in [
            Image::decode(&png).unwrap(),
            Image::decode_with_format(&png, image::ImageFormat::Png).unwrap(),
        ] {
            assert_eq!(decoded.pixels(), expected.pixels());
            assert_eq!(decoded.pixels().color_space(), Cicp::SRGB);
        }
        let png = png_with_cicp(
            DynamicImage::ImageRgba16(image::ImageBuffer::from_pixel(
                1,
                1,
                image::Rgba([197, 197, 197, 32768]),
            )),
            [1, 8, 0, 1],
        );
        assert_eq!(
            Image::decode(&png).unwrap().pixels().get_pixel(0, 0).0,
            [10, 10, 10, 128],
        );
    }

    #[test]
    fn encoded_png_unsupported_cicp_and_invalid_crc_are_rejected() {
        for tag in [
            [2, 13, 0, 1],
            [1, 2, 0, 1],
            [255, 13, 0, 1],
            [1, 255, 0, 1],
            [1, 13, 1, 1],
            [1, 13, 0, 0],
            [1, 13, 0, 255],
        ] {
            let png = png_with_cicp(DynamicImage::new_rgb8(1, 1), tag);
            for result in [
                Image::decode(&png),
                Image::decode_with_format(&png, image::ImageFormat::Png),
            ] {
                assert_eq!(
                    result.unwrap_err().kind(),
                    io::ErrorKind::Unsupported,
                    "{tag:?}"
                );
            }
        }
        let mut png = png_with_cicp(DynamicImage::new_rgb8(1, 1), [1, 8, 0, 1]);
        png[48] ^= 1;
        assert!(Image::decode(&png).is_err());
    }

    #[test]
    fn unsupported_cicp_returns_an_explicit_error() {
        let mut source = DynamicImage::new_rgb8(1, 1);
        source.set_rgb_primaries(image::metadata::CicpColorPrimaries::Unspecified);
        assert_eq!(
            Image::new(source).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
    }

    #[test]
    fn png_icc_profiles_use_image_decoding() {
        use image::ImageEncoder;
        let profile = b"sRGB is only a name, not a verified color profile".to_vec();
        let mut png = Vec::new();
        let mut encoder = image::codecs::png::PngEncoder::new(&mut png);
        encoder.set_icc_profile(profile.clone()).unwrap();
        encoder
            .write_image(&[128; 3], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        let mut decoder = image::ImageReader::new(io::Cursor::new(&png))
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(decoder.icc_profile().unwrap(), Some(profile));
        let expected = DynamicImage::from_decoder(decoder).unwrap().into_rgba8();
        for decoded in [
            Image::decode(&png).unwrap(),
            Image::decode_with_format(&png, image::ImageFormat::Png).unwrap(),
        ] {
            assert_eq!(decoded.pixels(), &expected);
        }
        let path = format!("uncurses-icc-{}.png", std::process::id());
        std::fs::write(&path, &png).unwrap();
        let decoded = Image::open(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(decoded.unwrap().pixels(), &expected);
    }

    #[cfg(feature = "jpeg")]
    #[test]
    fn jpeg_icc_profiles_use_image_decoding() {
        use image::ImageEncoder;
        let profile = b"sRGB is only a name, not a verified color profile".to_vec();
        let mut jpeg = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut jpeg);
        encoder.set_icc_profile(profile.clone()).unwrap();
        encoder
            .write_image(&[128; 3], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        let mut decoder = image::ImageReader::new(io::Cursor::new(&jpeg))
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(decoder.icc_profile().unwrap(), Some(profile));
        let expected = DynamicImage::from_decoder(decoder).unwrap().into_rgba8();
        for decoded in [
            Image::decode(&jpeg).unwrap(),
            Image::decode_with_format(&jpeg, image::ImageFormat::Jpeg).unwrap(),
        ] {
            assert_eq!(decoded.pixels(), &expected);
        }
        let path = format!("uncurses-icc-{}.jpg", std::process::id());
        std::fs::write(&path, &jpeg).unwrap();
        let decoded = Image::open(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(decoded.unwrap().pixels(), &expected);
    }

    #[test]
    fn raw_pixels_validate_dimensions_and_length() {
        let image = Image::from_rgb(2, 1, vec![1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!(image.pixels().as_raw(), &[1, 2, 3, 255, 4, 5, 6, 255]);
        for (width, height) in [(0, 1), (1, 0), (u32::MAX, u32::MAX), (5000, 5000)] {
            assert!(Image::from_rgb(width, height, vec![]).is_err());
            assert!(Image::from_rgba(width, height, vec![]).is_err());
        }
        assert!(Image::from_rgb(1, 1, vec![0; 4]).is_err());
        assert!(Image::from_rgba(1, 1, vec![0; 3]).is_err());
        assert!(Image::new(DynamicImage::new_rgb8(0, 1)).is_err());
    }

    #[test]
    fn png_decode_and_malformed_input() {
        let source = RgbaImage::from_pixel(3, 5, image::Rgba([7, 8, 9, 128]));
        let mut png = io::Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(source.clone())
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        assert_eq!(Image::decode(png.get_ref()).unwrap().pixels(), &source);
        assert_eq!(
            Image::decode_with_format(png.get_ref(), image::ImageFormat::Png)
                .unwrap()
                .pixels(),
            &source,
        );
        for bytes in [&[][..], b"not an image", &png.get_ref()[..20]] {
            assert!(Image::decode(bytes).is_err());
            assert!(Image::decode_with_format(bytes, image::ImageFormat::Png).is_err());
        }
        let oversized = vec![0; Image::MAX_ENCODED_BYTES as usize + 1];
        assert_eq!(
            Image::decode(&oversized).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn files_validate_size_before_reading() {
        use std::io::Write;
        struct TestFile(String);
        impl Drop for TestFile {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let path = format!(".compositor-image-test-{}", std::process::id());
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        let file_guard = TestFile(path);
        let mut png = io::Cursor::new(Vec::new());
        DynamicImage::new_rgba8(2, 3)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        file.write_all(png.get_ref()).unwrap();
        file.flush().unwrap();
        assert_eq!(Image::open(&file_guard.0).unwrap().dimensions(), (2, 3));
        file.set_len(Image::MAX_ENCODED_BYTES + 1).unwrap();
        assert_eq!(
            Image::open(&file_guard.0).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        drop(file);
    }

    #[test]
    fn decoded_rgba_and_orientation_buffers_share_the_budget() {
        struct LargeDecoder;
        impl ImageDecoder for LargeDecoder {
            fn dimensions(&self) -> (u32, u32) {
                (4096, 4096)
            }
            fn color_type(&self) -> image::ColorType {
                image::ColorType::Rgba32F
            }
            fn read_image(self, _: &mut [u8]) -> image::ImageResult<()> {
                panic!("pixel decoding must not start above the allocation budget");
            }
            fn read_image_boxed(self: Box<Self>, _: &mut [u8]) -> image::ImageResult<()> {
                panic!("pixel decoding must not start above the allocation budget");
            }
        }
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(Image::MAX_BUFFER_BYTES);
        let error = Image::decode_pixels(LargeDecoder, limits, 0, None).unwrap_err();
        let source = error
            .get_ref()
            .unwrap()
            .downcast_ref::<image::ImageError>()
            .unwrap();
        assert!(matches!(source, image::ImageError::Limits(_)));
    }

    #[test]
    fn every_backend_has_exact_pixels_and_immutable_options() {
        let image = Image::from_rgba(1, 1, vec![255, 0, 0, 128]).unwrap();
        let options = ImageOptions {
            fit: Fit::Stretch,
            background: [0, 0, 255],
            ..Default::default()
        };
        let saved = options.clone();
        for backend in [Backend::Kitty, Backend::Iterm2, Backend::Sixel] {
            let block = image
                .prepare(backend, (2, 3).into(), (3, 4).into(), &options)
                .unwrap();
            assert_eq!(block.size(), Size::new(2, 3));
            let mut screen = uncurses::screen::Screen::new(Vec::new(), (4, 5));
            screen
                .set_placement(
                    1,
                    uncurses::layout::Rect::new(0, 0, 2, 3),
                    &block.placement(1).unwrap(),
                )
                .unwrap();
            screen.render().unwrap();
            let output = String::from_utf8_lossy(screen.writer());
            assert!(output.contains(match backend {
                Backend::Kitty => "\x1b_Ga=t",
                Backend::Iterm2 => "\x1b]1337;",
                Backend::Sixel => "\x1bP",
            }));
            assert_eq!(options, saved);
        }
        let raster = image
            .rasterize((2, 3).into(), (3, 4).into(), &options)
            .unwrap();
        assert_eq!(raster.dimensions(), (6, 12));
        assert!(raster.pixels().all(|p| p.0 == [128, 0, 127]));
    }

    #[test]
    fn crop_fit_and_background_have_exact_pixels() {
        let image = Image::from_rgb(2, 1, vec![255, 0, 0, 0, 0, 255]).unwrap();
        let options = ImageOptions {
            fit: Fit::Stretch,
            filter: imageops::FilterType::Nearest,
            crop: Some(Crop {
                x: 1,
                y: 0,
                width: 1,
                height: 1,
            }),
            ..Default::default()
        };
        let raster = image
            .rasterize((2, 2).into(), (1, 1).into(), &options)
            .unwrap();
        assert!(raster.pixels().all(|p| p.0 == [0, 0, 255]));
        let contained = image
            .rasterize(
                (2, 3).into(),
                (1, 1).into(),
                &ImageOptions {
                    filter: imageops::FilterType::Nearest,
                    background: [0, 255, 0],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(contained.get_pixel(0, 0).0, [0, 255, 0]);
        assert_eq!(contained.get_pixel(0, 1).0, [255, 0, 0]);
        assert_eq!(contained.get_pixel(1, 1).0, [0, 0, 255]);
        assert_eq!(contained.get_pixel(0, 2).0, [0, 255, 0]);
    }

    #[test]
    fn invalid_raster_requests_return_errors() {
        let image = Image::from_rgba(1, 1, vec![255; 4]).unwrap();
        for size in [
            Size::ZERO,
            Size::new(0, 1),
            Size::new(1, 0),
            Size::new(u16::MAX, u16::MAX),
        ] {
            assert!(
                image
                    .rasterize(size, (8, 16).into(), &ImageOptions::default())
                    .is_err()
            );
        }
        assert!(
            image
                .rasterize((1, 1).into(), Size::ZERO, &ImageOptions::default())
                .is_err()
        );
        assert!(
            image
                .prepare(
                    Backend::Kitty,
                    (1, 1).into(),
                    Size::ZERO,
                    &ImageOptions::default()
                )
                .is_err()
        );
        for options in [
            ImageOptions {
                scale: 0.0,
                ..Default::default()
            },
            ImageOptions {
                scale: f64::NAN,
                ..Default::default()
            },
            ImageOptions {
                scale: f64::INFINITY,
                ..Default::default()
            },
            ImageOptions {
                aspect_ratio: Some((0, 1)),
                ..Default::default()
            },
            ImageOptions {
                crop: Some(Crop {
                    x: u32::MAX,
                    y: 0,
                    width: 2,
                    height: 1,
                }),
                ..Default::default()
            },
            ImageOptions {
                max_colors: 0,
                ..Default::default()
            },
            ImageOptions {
                max_colors: 257,
                ..Default::default()
            },
        ] {
            let saved = options.clone();
            assert!(
                image
                    .rasterize((1, 1).into(), (1, 1).into(), &options)
                    .is_err()
            );
            assert_eq!(options.scale.to_bits(), saved.scale.to_bits());
            assert_eq!(options.crop, saved.crop);
            assert_eq!(options.max_colors, saved.max_colors);
        }
    }

    #[cfg(feature = "jpeg")]
    fn small_jpeg() -> Vec<u8> {
        let pixels = RgbImage::from_fn(8, 4, |x, _| {
            if x < 4 {
                Rgb([255, 0, 0])
            } else {
                Rgb([0, 0, 255])
            }
        });
        let mut jpeg = io::Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(pixels)
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        jpeg.into_inner()
    }

    #[cfg(feature = "jpeg")]
    #[test]
    fn oversized_jpeg_is_rejected_before_pixel_decode() {
        let mut jpeg = small_jpeg();
        let marker = jpeg
            .windows(2)
            .position(|bytes| bytes == [0xff, 0xc0])
            .unwrap();
        jpeg[marker + 5..marker + 7].copy_from_slice(&5000u16.to_be_bytes());
        jpeg[marker + 7..marker + 9].copy_from_slice(&5000u16.to_be_bytes());
        assert!(jpeg.len() < 4096);
        let error = Image::decode(&jpeg).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            error.to_string(),
            "image dimensions exceed the supported range"
        );
    }

    #[cfg(feature = "jpeg")]
    #[test]
    fn jpeg_exif_orientation_is_applied() {
        let mut jpeg = small_jpeg();
        let plain = Image::decode(&jpeg).unwrap();
        // Little-endian TIFF with one orientation entry: rotate 90 degrees.
        let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
        let mut app1 = vec![0xff, 0xe1];
        app1.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        app1.extend_from_slice(exif);
        jpeg.splice(2..2, app1);
        let oriented = Image::decode(&jpeg).unwrap();
        assert_eq!(oriented.dimensions(), (4, 8));
        assert_eq!(*oriented.pixels(), imageops::rotate90(plain.pixels()));
    }
}
