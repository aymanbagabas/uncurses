//! Shared image pixels, decoding, and rasterization.

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};

use ::image::{DynamicImage, Rgb, RgbImage, RgbaImage, imageops};
use uncurses::layout::Size;

use crate::protocols::kitty::CachedResource;
use crate::{ImageOptions, geometry, invalid};

/// Immutable RGBA pixels shared by image placements.
#[derive(Clone, Debug)]
pub struct Image {
    pub(crate) pixels: Arc<RgbaImage>,
    pub(crate) cache: Arc<Mutex<Vec<CachedResource>>>,
}

impl Image {
    /// Adopt decoded pixels. Both dimensions must be nonzero.
    pub fn new(image: DynamicImage) -> io::Result<Self> {
        if image.width() == 0 || image.height() == 0 {
            return Err(invalid("image dimensions must be nonzero"));
        }
        Ok(Self {
            pixels: Arc::new(image.into_rgba8()),
            cache: Arc::default(),
        })
    }

    /// Convert tightly packed RGB bytes, with exactly three bytes per pixel.
    ///
    /// Both dimensions must be nonzero. Pixels follow row order, from left to
    /// right and top to bottom. Each pixel receives an alpha value of 255.
    ///
    /// ```
    /// use uncurses_image::Image;
    ///
    /// let image = Image::from_rgb(2, 1, vec![255, 0, 0, 0, 255, 0])?;
    /// assert_eq!(image.pixels().get_pixel(0, 0).0, [255, 0, 0, 255]);
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn from_rgb(width: u32, height: u32, pixels: Vec<u8>) -> io::Result<Self> {
        let length = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(3));
        if width == 0
            || height == 0
            || length.is_none()
            || u64::try_from(pixels.len()).ok() != length
        {
            return Err(invalid("RGB data must match nonzero image dimensions"));
        }
        let pixels = RgbImage::from_raw(width, height, pixels)
            .ok_or_else(|| invalid("image dimensions exceed the addressable buffer"))?;
        Self::new(DynamicImage::ImageRgb8(pixels))
    }

    /// Adopt tightly packed RGBA bytes, with exactly four bytes per pixel.
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> io::Result<Self> {
        let length = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(4));
        if width == 0
            || height == 0
            || length.is_none()
            || u64::try_from(pixels.len()).ok() != length
        {
            return Err(invalid("RGBA data must match nonzero image dimensions"));
        }
        let pixels = RgbaImage::from_raw(width, height, pixels)
            .ok_or_else(|| invalid("image dimensions exceed the addressable buffer"))?;
        Ok(Self {
            pixels: Arc::new(pixels),
            cache: Arc::default(),
        })
    }

    /// Decode an image file with the enabled format features.
    ///
    /// The decoder applies the file's orientation metadata.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let reader = image::ImageReader::open(path)?.with_guessed_format()?;
        let mut decoder = reader.into_decoder().map_err(io::Error::other)?;
        use image::ImageDecoder;
        let orientation = decoder.orientation().map_err(io::Error::other)?;
        let mut decoded = DynamicImage::from_decoder(decoder).map_err(io::Error::other)?;
        decoded.apply_orientation(orientation);
        Self::new(decoded)
    }

    /// Decode an image from bytes with the enabled format features.
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        let reader = image::ImageReader::new(io::Cursor::new(bytes)).with_guessed_format()?;
        let mut decoder = reader.into_decoder().map_err(io::Error::other)?;
        use image::ImageDecoder;
        let orientation = decoder.orientation().map_err(io::Error::other)?;
        let mut decoded = DynamicImage::from_decoder(decoder).map_err(io::Error::other)?;
        decoded.apply_orientation(orientation);
        Self::new(decoded)
    }

    /// Source width and height in pixels.
    pub fn dimensions(&self) -> (u32, u32) {
        self.pixels.dimensions()
    }

    /// Borrow the immutable source pixels.
    pub fn pixels(&self) -> &RgbaImage {
        &self.pixels
    }

    /// Rasterize into a cell rectangle at its configured pixel resolution.
    ///
    /// Transparent pixels and unused space use `options.background`.
    pub fn rasterize(&self, size: Size, options: &ImageOptions) -> io::Result<RgbImage> {
        let width = u32::from(size.width) * u32::from(options.cell_pixels.0);
        let height = u32::from(size.height) * u32::from(options.cell_pixels.1);
        self.sample(size, options, (width, height))
    }

    pub(crate) fn sample(
        &self,
        cells: Size,
        options: &ImageOptions,
        samples: (u32, u32),
    ) -> io::Result<RgbImage> {
        let geometry = geometry::resolve(self.dimensions(), cells, samples, options)?;
        let mut result = RgbImage::from_pixel(samples.0, samples.1, Rgb(options.background));
        let source = geometry.source;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Crop, FilterType, Fit, SymbolSet};
    use uncurses::buffer::Surface;
    use uncurses::cell::Cell;
    use uncurses::color::{Color, Profile};
    use uncurses::layout::Position;
    use uncurses::style::Style;

    #[test]
    fn rgb_pixels_preserve_channels_and_receive_opaque_alpha() {
        let rgb = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 17, 93, 201];
        let image = Image::from_rgb(2, 2, rgb.clone()).unwrap();
        assert_eq!(image.dimensions(), (2, 2));
        assert_eq!(
            image.pixels().as_raw(),
            &[
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 17, 93, 201, 255
            ]
        );
        let options = ImageOptions {
            cell_pixels: (1, 1),
            ..Default::default()
        };
        assert_eq!(
            image.rasterize(Size::new(2, 2), &options).unwrap().as_raw(),
            &rgb
        );
        for (width, height, pixels) in [
            (0, 1, vec![]),
            (1, 0, vec![]),
            (1, 1, vec![0; 2]),
            (1, 1, vec![0; 4]),
            (u32::MAX, u32::MAX, vec![]),
        ] {
            assert_eq!(
                Image::from_rgb(width, height, pixels).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    fn invalid_pixels_and_geometry_return_errors() {
        for (w, h, pixels) in [
            (0, 1, vec![]),
            (1, 0, vec![]),
            (1, 1, vec![0; 3]),
            (u32::MAX, u32::MAX, vec![]),
        ] {
            assert_eq!(
                Image::from_rgba(w, h, pixels).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        let image = Image::from_rgba(1, 1, vec![255; 4]).unwrap();
        for options in [
            ImageOptions {
                scale: 0.0,
                ..Default::default()
            },
            ImageOptions {
                scale: -1.0,
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
                cell_pixels: (0, 16),
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
        ] {
            assert_eq!(
                image
                    .rasterize(Size::new(1, 1), &options)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        assert!(
            image
                .rasterize(Size::new(0, 1), &ImageOptions::default())
                .is_err()
        );
        assert!(Image::decode(b"not an image").is_err());
    }

    #[test]
    fn transparency_background_crop_and_half_blocks_match_pixels() {
        let image = Image::from_rgba(
            2,
            2,
            vec![
                255, 0, 0, 255, 255, 0, 255, 0, 0, 0, 255, 255, 0, 255, 0, 128,
            ],
        )
        .unwrap();
        let options = ImageOptions {
            fit: Fit::Stretch,
            cell_pixels: (1, 2),
            background: [0, 0, 0],
            filter: FilterType::Nearest,
            ..Default::default()
        };
        let saved = options.clone();
        let pixels = image.rasterize(Size::new(2, 1), &options).unwrap();
        assert_eq!(pixels.as_raw(), &[255, 0, 0, 0, 0, 0, 0, 0, 255, 0, 128, 0]);
        let cells = image
            .symbols(
                Size::new(2, 1),
                &options,
                Profile::TrueColor,
                SymbolSet::UPPER_LOWER,
            )
            .unwrap();
        let first = cells.cell(Position::ORIGIN).unwrap();
        assert_eq!(first.content(), "\u{2580}");
        assert_eq!(
            first.style,
            Style::default()
                .fg(Color::Rgb(255, 0, 0))
                .bg(Color::Rgb(0, 0, 255))
        );
        let monochrome = Image::from_rgba(1, 2, vec![255, 255, 255, 255, 0, 0, 0, 255])
            .unwrap()
            .symbols(
                Size::new(1, 1),
                &options,
                Profile::Ascii,
                SymbolSet::UPPER_LOWER,
            )
            .unwrap();
        assert_eq!(
            monochrome.cell(Position::ORIGIN).unwrap(),
            &Cell::new("\u{2580}", 1)
        );
        let cropped = image
            .rasterize(
                Size::new(1, 1),
                &ImageOptions {
                    crop: Some(Crop {
                        x: 1,
                        y: 1,
                        width: 1,
                        height: 1,
                    }),
                    ..options.clone()
                },
            )
            .unwrap();
        assert!(cropped.pixels().all(|pixel| pixel.0 == [0, 128, 0]));
        assert_eq!(options, saved);
    }
}
