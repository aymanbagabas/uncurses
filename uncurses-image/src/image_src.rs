use std::path::Path;

use image::DynamicImage;

use crate::error::Result;

/// A decoded source image. Decoding is eager; resizing happens at
/// paint time using the placement's [`crate::Resize`] strategy.
#[derive(Debug, Clone)]
pub struct Image {
    pixels: DynamicImage,
    /// Stable hash of the source pixels used to skip re-encoding when
    /// neither the image nor its placement has changed.
    hash: u64,
}

impl Image {
    /// Decode an image from raw bytes. The format is detected from
    /// the byte stream (PNG / JPEG / GIF / WebP, depending on which
    /// crate features are enabled).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let pixels = image::load_from_memory(bytes)?;
        Ok(Self::from_dynamic(pixels))
    }

    /// Decode an image from a filesystem path.
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let pixels = image::open(path)?;
        Ok(Self::from_dynamic(pixels))
    }

    /// Wrap an already-decoded `DynamicImage`.
    pub fn from_dynamic(pixels: DynamicImage) -> Self {
        let hash = compute_hash(&pixels);
        Self { pixels, hash }
    }

    /// Source dimensions in pixels.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.pixels.width(), self.pixels.height())
    }

    /// Borrow the underlying decoded pixels.
    pub fn pixels(&self) -> &DynamicImage {
        &self.pixels
    }

    /// Stable hash of the source pixels. Two images with identical
    /// pixel data hash equal; any change to the data hashes
    /// differently. Used by the layer to skip retransmission.
    pub fn content_hash(&self) -> u64 {
        self.hash
    }
}

/// Hash a `DynamicImage`'s pixel buffer using the same hasher used
/// elsewhere in the workspace. The hash is over the canonicalized
/// RGBA8 pixel buffer so two visually identical images compare equal
/// regardless of their original color type.
fn compute_hash(pixels: &DynamicImage) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = rustc_hash::FxHasher::default();
    let rgba = pixels.to_rgba8();
    rgba.width().hash(&mut hasher);
    rgba.height().hash(&mut hasher);
    rgba.as_raw().hash(&mut hasher);
    hasher.finish()
}
