//! Compare built-in image symbol families.
//!
//! Run `cargo run -p examples --example image_symbols`.
//! Use `--symbols 'block+border-diagonal'` for a selection, or `--plain` for
//! monochrome text. Append an image path to replace the generated pattern.
//! Recent mosaics and wide glyphs require a suitable terminal font.
//! Use `--plain --dither bayer8x8` to compare monochrome dithering.
//! Use `--eaw-wide` when the terminal measures ambiguous characters as two cells.

use std::io::{self, Write};

use uncurses::color::Profile;
use uncurses::layout::Rect;
use uncurses::screen::Screen;
use uncurses::text::Encode;
use uncurses_image::{Fit, Image, ImageOptions, ImageRenderer, Protocol, SymbolSet};

#[path = "support/image_symbol_sets.rs"]
mod image_symbol_sets;
use image_symbol_sets::SYMBOL_SETS;

#[path = "support/image_dither_args.rs"]
mod image_dither_args;

fn pattern() -> io::Result<Image> {
    let pixels = (0..64)
        .flat_map(|y| {
            (0..192).flat_map(move |x| {
                let bright = match x / 48 {
                    0 => (x + y) % 24 < 12,
                    1 => (x / 6 + y / 6) % 2 == 0,
                    2 => (x % 48 - 24_i32).pow(2) + (y - 32_i32).pow(2) < 400,
                    _ => y < x % 48 + 8,
                };
                if bright {
                    [255, 220, 100]
                } else {
                    [20, 40, 90]
                }
            })
        })
        .collect();
    Image::from_rgb(192, 64, pixels)
}

fn main() -> io::Result<()> {
    let mut selectors = Vec::new();
    let mut profile = Profile::TrueColor;
    let mut eaw_wide = false;
    let mut grapheme_clusters = false;
    let mut path = None;
    let mut options = ImageOptions {
        fit: Fit::Stretch,
        ..Default::default()
    };
    let mut args = std::env::args().skip(1);
    let mut image_args = image_dither_args::ImageArgs::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--symbols" => selectors.push(args.next().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "--symbols needs a selector")
            })?),
            "--plain" => profile = Profile::Ascii,
            "--eaw-wide" => eaw_wide = true,
            "--grapheme" => grapheme_clusters = true,
            "--help" => {
                println!(
                    "Usage: image_symbols [--symbols SELECTOR] [--plain] [--eaw-wide] [--grapheme] [--dither MODE] [--dither-grain WxH] [--dither-intensity N] [image.png]\nSelectors: {}\nCombine with + or comma; exclude with -. Without --symbols, show every family.\nUse --eaw-wide for two-cell ambiguous characters and --grapheme for grapheme measurement. Match the terminal's settings.",
                    SYMBOL_SETS
                        .iter()
                        .map(|(name, _)| *name)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                return Ok(());
            }
            _ if image_dither_args::parse(&arg, &mut args, &mut image_args)? => {}
            _ if !arg.starts_with('-') && path.is_none() => path = Some(arg),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unexpected argument: {arg}"),
                ));
            }
        }
    }
    image_args.apply(&mut options)?;
    if selectors.is_empty() {
        selectors.extend(SYMBOL_SETS.iter().map(|(name, _)| name.to_string()));
    }
    let image = if let Some(path) = path {
        Image::open(path)?
    } else {
        pattern()?
    };
    let mut out = io::stdout().lock();
    if eaw_wide {
        writeln!(out, "Ambiguous width: 2 cells")?;
    }
    for name in selectors {
        let symbols: SymbolSet = name.parse()?;
        let mut screen = Screen::new(Vec::new(), (32, 6));
        screen.set_color_profile(profile);
        screen.set_eaw_wide(eaw_wide);
        screen.set_grapheme_clusters(grapheme_clusters);
        let renderer = ImageRenderer {
            protocol: Protocol::Symbols(symbols),
            ..Default::default()
        };
        renderer.draw(
            &mut screen,
            "image",
            Rect::new(0, 0, 32, 6),
            &image,
            &options,
        )?;
        writeln!(out, "{name}")?;
        screen.encode_with(&mut out, profile)?;
        writeln!(out, "\n")?;
    }
    Ok(())
}
