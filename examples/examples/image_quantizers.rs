//! Compare Wu, K-means, and a custom palette on the same image.
//! Run `cargo run -p examples --example image_quantizers`.

use std::{io, sync::Arc};
use uncurses_image::{
    RgbImage,
    dither::{Bayer8x8, Dither, FloydSteinberg},
    quant::{KMeans, Quantize, QuantizeContext, Wu},
};

#[derive(Debug)]
struct GrayRamp;

impl Quantize for GrayRamp {
    fn palette(&self, context: QuantizeContext<'_>) -> io::Result<Vec<[u8; 3]>> {
        let count = usize::from(context.max_colors);
        Ok((0..count)
            .map(|i| [(i * 255 / count.saturating_sub(1).max(1)) as u8; 3])
            .collect())
    }
}

fn main() -> io::Result<()> {
    let mut pixels = RgbImage::new(64, 32);
    for (x, y, pixel) in pixels.enumerate_pixels_mut() {
        pixel.0 = [(x * 4) as u8, (y * 8) as u8, ((x + y) * 2) as u8];
    }
    let context = QuantizeContext {
        pixels: &pixels,
        max_colors: 8,
    };
    let algorithms: [(&str, Arc<dyn Quantize>); 3] = [
        ("Wu", Arc::new(Wu)),
        ("KMeans", Arc::new(KMeans::default())),
        ("Custom gray ramp", Arc::new(GrayRamp)),
    ];
    for (name, algorithm) in algorithms {
        let image = algorithm.quantize(context)?;
        println!(
            "{name}: {} colors, {} pixels, palette {:?}",
            image.palette.len(),
            image.indices.len(),
            image.palette
        );
        let prepared = algorithm.prepare(context)?;
        for (dither_name, dither) in [
            ("Floyd-Steinberg", &FloydSteinberg::default() as &dyn Dither),
            ("Bayer8x8", &Bayer8x8::default()),
        ] {
            let indices = dither.dither(prepared.context())?;
            let changed = indices
                .iter()
                .zip(&image.indices)
                .filter(|(a, b)| a != b)
                .count();
            println!(
                "  Prepared {dither_name}: {} pixels, {changed} changed indices",
                indices.len()
            );
        }
    }
    Ok(())
}
