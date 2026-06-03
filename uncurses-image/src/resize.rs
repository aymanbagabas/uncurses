pub use image::imageops::FilterType;

/// Strategy for fitting an image into a placement [`uncurses::Rect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resize {
    /// Scale down to fit inside the rect while preserving aspect ratio.
    /// The image is never enlarged beyond its source size.
    Fit(FilterType),
    /// Scale to cover the rect, then crop the overflow at `anchor`.
    Crop(CropAnchor),
    /// Scale to exactly fill the rect (aspect ratio not preserved).
    Scale(FilterType),
}

impl Default for Resize {
    fn default() -> Self {
        Self::Fit(FilterType::Triangle)
    }
}

/// Anchor used by [`Resize::Crop`] when cropping the scaled image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CropAnchor {
    #[default]
    Center,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}
