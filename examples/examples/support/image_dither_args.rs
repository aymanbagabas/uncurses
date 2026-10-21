use std::{io, sync::Arc};
use uncurses_image::{
    ImageOptions,
    dither::{Bayer8x8, Dither, FloydSteinberg, Noise},
    quant,
};

#[derive(Default)]
pub struct ImageArgs {
    pub mode: Option<String>,
    pub grain: Option<(u8, u8)>,
    pub intensity: Option<f32>,
    quantizer: Option<String>,
    max_colors: Option<u16>,
}

impl ImageArgs {
    pub fn algorithm(&self, mode: &str) -> io::Result<Option<Arc<dyn Dither>>> {
        let grain = self.grain.unwrap_or((1, 1));
        let algorithm: Arc<dyn Dither> = match mode {
            "none" => return Ok(None),
            "bayer8x8" => Arc::new(Bayer8x8 {
                grain,
                intensity: self.intensity.unwrap_or(1.0),
            }),
            "diffusion" | "fs" => Arc::new(FloydSteinberg {
                grain,
                strength: self.intensity.unwrap_or(0.875),
            }),
            "noise" => Arc::new(Noise {
                grain,
                intensity: self.intensity.unwrap_or(1.0),
            }),
            _ => {
                return Err(invalid("expected none, bayer8x8, diffusion/fs, or noise"));
            }
        };
        algorithm.validate()?;
        Ok(Some(algorithm))
    }

    pub fn apply(&self, options: &mut ImageOptions) -> io::Result<()> {
        options.dither = self.algorithm(self.mode.as_deref().unwrap_or("fs"))?;
        if let Some(name) = &self.quantizer {
            options.quantizer = match name.as_str() {
                "wu" => Arc::new(quant::Wu),
                "kmeans" => Arc::new(quant::KMeans::default()),
                _ => return Err(invalid("expected wu or kmeans")),
            };
        }
        if let Some(max_colors) = self.max_colors {
            options.max_colors = max_colors;
        }
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

pub fn parse(
    arg: &str,
    args: &mut impl Iterator<Item = String>,
    options: &mut ImageArgs,
) -> io::Result<bool> {
    if !matches!(
        arg,
        "--dither" | "--dither-grain" | "--dither-intensity" | "--quantizer" | "--colors"
    ) {
        return Ok(false);
    }
    let value = args
        .next()
        .ok_or_else(|| invalid(format!("{arg} needs a value")))?;
    match arg {
        "--dither" => {
            let value = value.trim().to_ascii_lowercase();
            if !matches!(
                value.as_str(),
                "none" | "bayer8x8" | "diffusion" | "fs" | "noise"
            ) {
                return Err(invalid("expected none, bayer8x8, diffusion/fs, or noise"));
            }
            options.mode = Some(value);
        }
        "--quantizer" => options.quantizer = Some(value.trim().to_ascii_lowercase()),
        "--colors" => {
            let count = value.parse::<u16>().map_err(|e| invalid(e.to_string()))?;
            if !(1..=256).contains(&count) {
                return Err(invalid("colors must be in 1..=256"));
            }
            options.max_colors = Some(count);
        }
        "--dither-grain" => {
            let (w, h) = value
                .split_once('x')
                .ok_or_else(|| invalid("expected WIDTHxHEIGHT"))?;
            let w = w.parse::<u8>().map_err(|e| invalid(e.to_string()))?;
            let h = h.parse::<u8>().map_err(|e| invalid(e.to_string()))?;
            if !matches!(w, 1 | 2 | 4 | 8) || !matches!(h, 1 | 2 | 4 | 8) {
                return Err(invalid("grain axes must be 1, 2, 4, or 8"));
            }
            options.grain = Some((w, h));
        }
        "--dither-intensity" => {
            let intensity = value.parse::<f32>().map_err(|e| invalid(e.to_string()))?;
            if !intensity.is_finite() || intensity < 0.0 {
                return Err(invalid("intensity must be finite and nonnegative"));
            }
            options.intensity = Some(intensity);
        }
        _ => unreachable!(),
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bayer_mode_uses_its_algorithm_name() {
        for (name, valid) in [("bayer8x8", true), ("bayer", false), ("ordered", false)] {
            let mut options = ImageArgs::default();
            let mut args = [name.to_owned()].into_iter();
            assert_eq!(parse("--dither", &mut args, &mut options).is_ok(), valid);
            assert_eq!(options.algorithm(name).is_ok(), valid);
        }
    }
}
