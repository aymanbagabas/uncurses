//! Shared palette validation and Oklab mapping.

use std::{any::Any, borrow::Cow, io};

use image::RgbImage;
use quantette::{
    ImageRef, Palette, PaletteBuf,
    color_map::{IndexedColorMap, NearestNeighborColorMap},
    color_space::srgb8_to_oklab,
    deps::palette::{Oklab, Srgb},
    dither::FloydSteinberg,
    wu::WuF32x3ColorMap,
};

use crate::invalid;

pub(crate) type ColorMap = NearestNeighborColorMap<Oklab, f32, 3>;

#[derive(Clone, Debug)]
pub(crate) enum Lookup {
    Nearest(ColorMap),
    Wu(WuF32x3ColorMap<Oklab, 32, 16, 16>),
}

impl Lookup {
    pub(crate) fn palette(&self) -> &Palette<Oklab> {
        match self {
            Self::Nearest(map) => map.palette(),
            Self::Wu(map) => map.palette(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Mapping {
    lookup: Lookup,
    palette: PaletteBuf<Oklab>,
}

impl Mapping {
    pub(crate) fn new(palette: &[[u8; 3]]) -> io::Result<Self> {
        let map = color_map(palette)?;
        Ok(Self {
            palette: map.palette().to_owned(),
            lookup: Lookup::Nearest(map),
        })
    }

    pub(crate) fn with_lookup(lookup: Lookup, palette: &[[u8; 3]]) -> io::Result<Self> {
        let mut map = Self {
            palette: palette_colors(palette)?,
            lookup,
        };
        map.update_nearest();
        Ok(map)
    }

    pub(crate) fn replace_palette(&mut self, palette: &[[u8; 3]]) -> io::Result<()> {
        self.palette = palette_colors(palette)?;
        self.update_nearest();
        Ok(())
    }

    fn update_nearest(&mut self) {
        if let Lookup::Nearest(map) = &mut self.lookup
            && map.palette().as_slice() != self.palette.as_slice()
        {
            *map = ColorMap::new(self.palette.clone());
        }
    }

    pub(crate) fn nearest(&self) -> Cow<'_, ColorMap> {
        match &self.lookup {
            Lookup::Nearest(map) => Cow::Borrowed(map),
            Lookup::Wu(_) => Cow::Owned(ColorMap::new(self.palette.clone())),
        }
    }

    pub(crate) fn diffuse(&self, image: ImageRef<'_, Oklab>, algorithm: FloydSteinberg) -> Vec<u8> {
        // Select the mapper once, outside the diffusion loop.
        match &self.lookup {
            Lookup::Nearest(map) => algorithm.dither_image_to_indexed(image, map).into_parts().1,
            Lookup::Wu(map) => {
                algorithm
                    .dither_image_to_indexed(
                        image,
                        DestinationMap {
                            lookup: map,
                            palette: &self.palette,
                        },
                    )
                    .into_parts()
                    .1
            }
        }
    }
}

struct DestinationMap<'a, M> {
    lookup: &'a M,
    palette: &'a Palette<Oklab>,
}

impl<M: IndexedColorMap<Oklab>> IndexedColorMap<Oklab> for DestinationMap<'_, M> {
    type Output = Oklab;

    fn into_palette(self) -> PaletteBuf<Oklab> {
        self.palette.to_owned()
    }

    fn palette(&self) -> &Palette<Oklab> {
        self.palette
    }

    fn base_palette(&self) -> &Palette<Oklab> {
        self.palette
    }

    #[inline]
    fn palette_index(&self, color: &Oklab) -> u8 {
        self.lookup.palette_index(color)
    }
}

impl IndexedColorMap<Oklab> for Mapping {
    type Output = Oklab;

    fn into_palette(self) -> PaletteBuf<Oklab> {
        self.palette
    }

    fn palette(&self) -> &Palette<Oklab> {
        &self.palette
    }

    fn base_palette(&self) -> &Palette<Oklab> {
        // Diffusion must use the destination colors, including Sixel rounding.
        &self.palette
    }

    #[inline]
    fn palette_index(&self, color: &Oklab) -> u8 {
        match &self.lookup {
            Lookup::Nearest(map) => map.palette_index(color),
            Lookup::Wu(map) => map.palette_index(color),
        }
    }
}

pub(crate) fn same<T: Any + PartialEq>(value: &T, other: &dyn Any) -> bool {
    other.downcast_ref::<T>() == Some(value)
}

pub(crate) fn validate_palette(palette: &[[u8; 3]], max_colors: u16) -> io::Result<()> {
    if !(1..=256).contains(&max_colors)
        || palette.is_empty()
        || palette.len() > usize::from(max_colors)
    {
        return Err(invalid(
            "palette must contain 1 through max_colors colors, with max_colors in 1..=256",
        ));
    }
    Ok(())
}

pub(crate) fn validate_indices(indices: &[u8], count: usize, palette_len: usize) -> io::Result<()> {
    if indices.len() != count || indices.iter().any(|&i| usize::from(i) >= palette_len) {
        return Err(invalid(
            "mapping must return one valid palette index per pixel",
        ));
    }
    Ok(())
}

pub(crate) fn rgb_colors(pixels: &RgbImage) -> Vec<Srgb<u8>> {
    pixels
        .pixels()
        .map(|c| Srgb::new(c[0], c[1], c[2]))
        .collect()
}

pub(crate) fn colors(pixels: &RgbImage) -> Vec<Oklab> {
    srgb8_to_oklab(&rgb_colors(pixels))
}

pub(crate) fn color_map(palette: &[[u8; 3]]) -> io::Result<ColorMap> {
    Ok(ColorMap::new(palette_colors(palette)?))
}

fn palette_colors(palette: &[[u8; 3]]) -> io::Result<PaletteBuf<Oklab>> {
    validate_palette(palette, 256)?;
    let rgb: Vec<_> = palette
        .iter()
        .map(|c| Srgb::new(c[0], c[1], c[2]))
        .collect();
    PaletteBuf::new(srgb8_to_oklab(&rgb)).map_err(|error| invalid(error.to_string().as_str()))
}

pub(crate) fn nearest(pixels: &RgbImage, palette: &[[u8; 3]]) -> io::Result<Vec<u8>> {
    let map = color_map(palette)?;
    Ok(colors(pixels)
        .iter()
        .map(|c| map.palette_index(c))
        .collect())
}
