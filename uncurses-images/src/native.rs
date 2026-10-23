//! Image-owned protocol encoding and shared placement resources.

use std::io::{self, Write};
use std::sync::Arc;

use image::RgbImage;
use uncurses::layout::{Rect, Size};
use uncurses::screen::PlacementOptions;

use crate::{Backend, ImageOptions, invalid, protocols};

#[derive(Debug)]
pub(crate) struct KittyResource {
    pub id: u32,
    pub upload: Arc<[u8]>,
    pub delete: Arc<[u8]>,
}

impl KittyResource {
    pub fn new(id: u32, upload: Vec<u8>) -> Self {
        Self {
            id,
            upload: upload.into(),
            delete: format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
                .into_bytes()
                .into(),
        }
    }
}

#[derive(Clone, Debug)]
enum Source {
    Kitty(Arc<KittyResource>),
    Png(Arc<RgbImage>),
    Sixel(Arc<crate::quant::IndexedImage>),
}

/// An encoded image whose placements share their source and resources.
///
/// Pass [`placement`](Self::placement) to `Screen::set_placement`, then render.
/// The screen uploads shared resources and releases them after their final
/// placement removal succeeds. Clones share encoded data.
/// Dropping this value performs no terminal I/O.
#[derive(Clone, Debug)]
pub struct PreparedImage {
    source: Source,
    pixels: (u32, u32, u32, u32),
    size: Size,
    bytes: Arc<[u8]>,
}

impl PreparedImage {
    /// The destination footprint in cells.
    pub fn size(&self) -> Size {
        self.size
    }

    /// The selected native protocol.
    pub fn backend(&self) -> Backend {
        match self.source {
            Source::Kitty(_) => Backend::Kitty,
            Source::Png(_) => Backend::Iterm2,
            Source::Sixel(_) => Backend::Sixel,
        }
    }

    /// The shared Kitty image identifier, when the protocol uses one.
    pub fn resource_id(&self) -> Option<u32> {
        match &self.source {
            Source::Kitty(resource) => Some(resource.id),
            _ => None,
        }
    }

    /// Raw upload bytes for manual protocol output.
    ///
    /// Screen placements already include these bytes.
    pub fn upload(&self) -> &[u8] {
        match &self.source {
            Source::Kitty(resource) => &resource.upload,
            _ => &[],
        }
    }

    /// Raw resource deletion bytes for manual protocol output.
    ///
    /// Screen placements already manage resource deletion.
    pub fn delete(&self) -> &[u8] {
        match &self.source {
            Source::Kitty(resource) => &resource.delete,
            _ => &[],
        }
    }

    /// Encode a nonzero protocol placement ID.
    ///
    /// Kitty uses separate placement and image identifiers. Raster protocols
    /// draw their complete encoded image. Reserve one spare row below raster
    /// placements to prevent terminal scrolling at the bottom edge.
    /// The returned options share resource bytes across clones and crops.
    /// Register each simultaneous Kitty placement with a distinct protocol ID.
    pub fn placement(&self, id: u32) -> io::Result<PlacementOptions> {
        if id == 0 {
            return Err(invalid("placement identifier must be nonzero"));
        }
        let mut options = PlacementOptions::default();
        if let Source::Kitty(resource) = &self.source {
            options.upload = resource.upload.clone();
            options.delete = resource.delete.clone();
            let (x, y, width, height) = self.pixels;
            write!(
                options.draw,
                "\x1b_Ga=p,i={},p={id},c={},r={},x={x},y={y},w={width},h={height},C=1,q=2\x1b\\",
                resource.id, self.size.width, self.size.height
            )?;
            write!(
                options.erase,
                "\x1b_Ga=d,d=i,i={},p={id},q=2\x1b\\",
                resource.id
            )?;
        } else {
            options.draw = self.bytes.to_vec();
        }
        Ok(options)
    }

    /// Encode a cell-aligned crop while retaining the shared source.
    ///
    /// Native Kitty crops share the original upload. Raster crops encode
    /// their selected pixels. Callers decide whether the protocol needs crops.
    pub fn crop(&self, area: Rect) -> io::Result<Self> {
        if area.width == 0
            || area.height == 0
            || u32::from(area.x) + u32::from(area.width) > u32::from(self.size.width)
            || u32::from(area.y) + u32::from(area.height) > u32::from(self.size.height)
        {
            return Err(invalid("crop must fit the prepared image"));
        }
        if area == Rect::new(0, 0, self.size.width, self.size.height) {
            return Ok(self.clone());
        }
        let edge = |value: u16, pixels: u32, cells: u16| {
            (u64::from(value) * u64::from(pixels) / u64::from(cells)) as u32
        };
        let (x, y, width, height) = self.pixels;
        let left = edge(area.x, width, self.size.width);
        let top = edge(area.y, height, self.size.height);
        let right = edge(area.right(), width, self.size.width);
        let bottom = edge(area.bottom(), height, self.size.height);
        Self::encode(
            self.source.clone(),
            (x + left, y + top, right - left, bottom - top),
            Size::new(area.width, area.height),
        )
    }

    fn encode(source: Source, pixels: (u32, u32, u32, u32), size: Size) -> io::Result<Self> {
        let (x, y, width, height) = pixels;
        if width == 0 || height == 0 {
            return Err(invalid("crop contains no source pixels"));
        }
        let bytes = match &source {
            Source::Kitty(_) => Vec::new(),
            Source::Png(image) => protocols::iterm2::encode(
                image::imageops::crop_imm(image.as_ref(), x, y, width, height).to_image(),
                size,
            )?,
            Source::Sixel(image) => protocols::sixel::encode(image, pixels)?,
        };
        Ok(Self {
            source,
            pixels,
            size,
            bytes: bytes.into(),
        })
    }
}

pub(crate) fn prepare(
    backend: Backend,
    pixels: RgbImage,
    size: Size,
    options: &ImageOptions,
) -> io::Result<PreparedImage> {
    let dimensions = (0, 0, pixels.width(), pixels.height());
    let source = match backend {
        Backend::Kitty => {
            let id = protocols::kitty::next_id()?;
            Source::Kitty(Arc::new(KittyResource::new(
                id,
                protocols::kitty::encode(&pixels, id)?,
            )))
        }
        Backend::Iterm2 => Source::Png(Arc::new(pixels)),
        Backend::Sixel => Source::Sixel(Arc::new(crate::quant::sixel(&pixels, options)?)),
    };
    PreparedImage::encode(source, dimensions, size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Image;
    use uncurses::screen::Screen;

    #[test]
    fn screen_placements_share_uploads_and_release_the_last_copy() {
        let image = Image::from_rgb(1, 1, vec![255, 0, 0])
            .unwrap()
            .prepare(
                Backend::Kitty,
                (2, 1).into(),
                (1, 1).into(),
                &ImageOptions::default(),
            )
            .unwrap();
        let mut screen = Screen::new(Vec::new(), (8, 4));
        for id in [1, 2] {
            let crop = image.crop(Rect::new((id - 1) as u16, 0, 1, 1)).unwrap();
            screen
                .set_placement(
                    u64::from(id),
                    Rect::new((id * 3 - 3) as u16, 0, 1, 1),
                    &crop.placement(id).unwrap(),
                )
                .unwrap();
        }
        assert!(screen.writer().is_empty());
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert_eq!(output.matches("a=t,").count(), 1);
        assert_eq!(output.matches("a=p,").count(), 2);
        screen.writer_mut().clear();
        screen.remove_placement(1);
        screen.render().unwrap();
        assert!(!String::from_utf8_lossy(screen.writer()).contains("d=I,"));
        screen.writer_mut().clear();
        screen.remove_placement(2);
        screen.render().unwrap();
        assert_eq!(
            String::from_utf8_lossy(screen.writer())
                .matches("d=I,")
                .count(),
            1
        );
        assert!(image.placement(0).is_err());
        assert!(screen.writer().ends_with(image.delete()));
        screen.writer_mut().clear();
        screen.render().unwrap();
        assert!(screen.writer().is_empty());
    }

    #[test]
    fn direct_and_nested_crops_preserve_source_pixels() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let pixels = RgbImage::from_fn(8, 9, |x, y| {
            image::Rgb([(x * 30) as u8, (y * 25) as u8, 90])
        });
        let options = ImageOptions::default();
        for backend in [Backend::Kitty, Backend::Iterm2, Backend::Sixel] {
            let full = prepare(backend, pixels.clone(), (4, 3).into(), &options).unwrap();
            for crop in [
                full.crop(Rect::new(1, 1, 2, 1)).unwrap(),
                full.crop(Rect::new(1, 0, 3, 3))
                    .unwrap()
                    .crop(Rect::new(0, 1, 2, 1))
                    .unwrap(),
            ] {
                let placement = crop.placement(7).unwrap();
                let output = String::from_utf8_lossy(&placement.draw);
                assert_eq!(crop.size(), (2, 1).into());
                match backend {
                    Backend::Kitty => {
                        assert!(output.contains("c=2,r=1,x=2,y=3,w=4,h=3,C=1"));
                        assert_eq!(crop.resource_id(), full.resource_id());
                        let original = full.placement(1).unwrap();
                        assert!(Arc::ptr_eq(&placement.upload, &original.upload));
                        assert!(Arc::ptr_eq(&placement.delete, &original.delete));
                        assert_eq!(placement.upload.as_ref(), full.upload());
                        assert_eq!(placement.delete.as_ref(), full.delete());
                    }
                    Backend::Iterm2 => {
                        let payload = output
                            .split_once("preserveAspectRatio=0:")
                            .unwrap()
                            .1
                            .split('\x07')
                            .next()
                            .unwrap();
                        let png = STANDARD.decode(payload).unwrap();
                        assert_eq!(
                            image::load_from_memory(&png).unwrap().into_rgb8(),
                            image::imageops::crop_imm(&pixels, 2, 3, 4, 3).to_image()
                        );
                    }
                    Backend::Sixel => {
                        let indexed = crate::quant::sixel(&pixels, &options).unwrap();
                        assert_eq!(
                            placement.draw,
                            protocols::sixel::encode(&indexed, (2, 3, 4, 3)).unwrap()
                        );
                    }
                }
            }
        }
    }
}
