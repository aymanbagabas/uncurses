//! iTerm2 inline PNG images.

use std::io;

use ::image::{DynamicImage, ImageFormat, RgbImage};
use base64::{Engine, engine::general_purpose::STANDARD};
use uncurses::ansi::iterm2;
use uncurses::layout::Rect;

pub(crate) fn encode(pixels: RgbImage, rect: Rect) -> io::Result<Vec<u8>> {
    let mut png = io::Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(pixels)
        .write_to(&mut png, ImageFormat::Png)
        .map_err(io::Error::other)?;
    let payload = format!(
        "File=inline=1;width={};height={};preserveAspectRatio=0:{}",
        rect.width,
        rect.height,
        STANDARD.encode(png.into_inner()),
    );
    let mut bytes = Vec::new();
    iterm2::write_iterm2(&mut bytes, &payload)?;
    Ok(bytes)
}

#[cfg(all(test, feature = "png"))]
mod tests {
    use super::*;

    #[test]
    fn iterm_payload_decodes_to_exact_pixels() {
        let pixels = RgbImage::from_pixel(3, 5, image::Rgb([12, 34, 56]));
        let bytes = encode(pixels.clone(), Rect::new(0, 0, 1, 2)).unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        let (_, payload) = text.split_once(':').unwrap();
        let encoded = payload.trim_end_matches('\x07').trim_end_matches("\x1b\\");
        let png = STANDARD.decode(encoded).unwrap();
        let decoded = image::load_from_memory_with_format(&png, ImageFormat::Png)
            .unwrap()
            .into_rgb8();
        assert_eq!(decoded, pixels);
        assert!(text.contains("width=1;height=2;preserveAspectRatio=0"));
    }
}
