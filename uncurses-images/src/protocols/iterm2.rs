//! iTerm2 inline PNG images with an explicit cell footprint.

use std::io;

use base64::{Engine, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageFormat, RgbImage};
use uncurses::{ansi::iterm2, layout::Size};

pub(crate) fn encode(pixels: RgbImage, size: Size) -> io::Result<Vec<u8>> {
    let mut png = io::Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(pixels)
        .write_to(&mut png, ImageFormat::Png)
        .map_err(io::Error::other)?;
    let payload = format!(
        "File=inline=1;width={};height={};preserveAspectRatio=0:{}",
        size.width,
        size.height,
        STANDARD.encode(png.into_inner()),
    );
    let mut bytes = Vec::new();
    iterm2::write_iterm2(&mut bytes, &payload)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_preserves_pixel_dimensions_and_channels() {
        let pixels = RgbImage::from_pixel(3, 5, image::Rgb([12, 34, 56]));
        let bytes = encode(pixels.clone(), Size::new(1, 2)).unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        let (_, payload) = text.split_once(':').unwrap();
        let png = STANDARD.decode(payload.trim_end_matches('\x07')).unwrap();
        let decoded = image::load_from_memory_with_format(&png, ImageFormat::Png)
            .unwrap()
            .into_rgb8();
        assert_eq!(decoded, pixels);
        assert!(
            text.starts_with("\x1b]1337;File=inline=1;width=1;height=2;preserveAspectRatio=0:")
        );
        assert_eq!(bytes.last(), Some(&b'\x07'));
    }
}
