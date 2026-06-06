//! Pixel-content hashing for image cache keys.
//!
//! Backends use this to recognize "same image as last paint"
//! without the host having to track an explicit identity. The hash
//! is over the RGBA8 canonicalization of the source pixels plus
//! the dimensions, so two images with identical visible content
//! hash equal even if they were decoded from different formats or
//! stored as different `image::DynamicImage` variants.

use std::hash::{Hash, Hasher};

use image::DynamicImage;
use rustc_hash::FxHasher;

/// Hash the canonical RGBA8 pixel data of `image` and its
/// dimensions. Stable across program runs for identical inputs;
/// not cryptographic.
pub(crate) fn pixel_hash(image: &DynamicImage) -> u64 {
    let rgba = image.to_rgba8();
    let mut h = FxHasher::default();
    rgba.width().hash(&mut h);
    rgba.height().hash(&mut h);
    rgba.as_raw().hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn img(fill: [u8; 4]) -> DynamicImage {
        let mut buf = RgbaImage::new(4, 4);
        for px in buf.pixels_mut() {
            *px = Rgba(fill);
        }
        DynamicImage::ImageRgba8(buf)
    }

    #[test]
    fn identical_pixels_hash_equal() {
        let a = img([1, 2, 3, 255]);
        let b = img([1, 2, 3, 255]);
        assert_eq!(pixel_hash(&a), pixel_hash(&b));
    }

    #[test]
    fn different_pixels_hash_differently() {
        let a = img([1, 2, 3, 255]);
        let b = img([1, 2, 4, 255]);
        assert_ne!(pixel_hash(&a), pixel_hash(&b));
    }

    #[test]
    fn dimensions_are_part_of_the_hash() {
        let a = DynamicImage::ImageRgba8(RgbaImage::new(2, 4));
        let b = DynamicImage::ImageRgba8(RgbaImage::new(4, 2));
        assert_ne!(pixel_hash(&a), pixel_hash(&b));
    }
}
