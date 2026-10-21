//! Compare built-in dithers and a custom diagonal pattern on a gray ramp.
//! Run `cargo run -p examples --example image_dithers`.
//! Add `--dither-grain 2x4` or `--dither-intensity 0.5`.

use std::{
    io::{self, Write},
    sync::Arc,
};
use uncurses::{color::Profile, layout::Size, text::Encode};
use uncurses_image::{
    Fit, Image, ImageOptions, SymbolSet,
    dither::{Dither, DitherContext},
};

#[path = "support/image_dither_args.rs"]
mod image_dither_args;

#[derive(Debug)]
struct Diagonal {
    grain: (u8, u8),
    intensity: f32,
}

impl Dither for Diagonal {
    fn dither(&self, context: DitherContext<'_>) -> io::Result<Vec<u8>> {
        let mut pixels = context.pixels().clone();
        for (x, y, pixel) in pixels.enumerate_pixels_mut() {
            let stripe = (x / u32::from(self.grain.0) + y / u32::from(self.grain.1)) % 4;
            let offset = (f64::from(stripe) - 1.5) * 48.0 * f64::from(self.intensity);
            for channel in &mut pixel.0 {
                *channel = (f64::from(*channel) + offset).clamp(0.0, 255.0) as u8;
            }
        }
        DitherContext::new(&pixels, context.palette()).nearest()
    }
}

fn main() -> io::Result<()> {
    let image = Image::from_rgb(
        256,
        32,
        (0..32)
            .flat_map(|_| (0..=255).flat_map(|x| [x; 3]))
            .collect(),
    )?;
    let mut options = ImageOptions {
        fit: Fit::Stretch,
        ..Default::default()
    };
    let mut args = std::env::args().skip(1);
    let mut image_args = image_dither_args::ImageArgs::default();
    while let Some(arg) = args.next() {
        if !image_dither_args::parse(&arg, &mut args, &mut image_args)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unexpected argument: {arg}"),
            ));
        }
    }
    image_args.apply(&mut options)?;
    let modes = if let Some(mode) = &image_args.mode {
        vec![(mode.as_str(), options.dither.clone())]
    } else {
        vec![
            ("None", None),
            ("Bayer8x8", image_args.algorithm("bayer8x8")?),
            ("FloydSteinberg", image_args.algorithm("fs")?),
            ("Noise", image_args.algorithm("noise")?),
            (
                "Custom diagonal",
                Some(Arc::new(Diagonal {
                    grain: image_args.grain.unwrap_or((1, 1)),
                    intensity: image_args.intensity.unwrap_or(1.0),
                }) as Arc<dyn Dither>),
            ),
        ]
    };
    let mut out = io::stdout().lock();
    for (name, algorithm) in modes {
        writeln!(out, "{name}")?;
        options.dither = algorithm;
        image
            .symbols(
                Size::new(64, 4),
                &options,
                Profile::Ascii,
                SymbolSet::BRAILLE,
            )?
            .encode_with(&mut out, Profile::Ascii)?;
        writeln!(out, "\n")?;
    }
    Ok(())
}
