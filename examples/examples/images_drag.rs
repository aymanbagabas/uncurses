//! Drag native or ordinary-cell images, in fullscreen or inline mode.
//!
//! Press Tab or p to cycle protocols. Use s/f/r/d/k/c for symbol sets, fit,
//! resampling, dithering, quantization, and palette size. --help lists controls.

#[path = "support/placements_drag.rs"]
mod placements_drag;

use std::{io, sync::Arc};

use placements_drag::{Content, Options, PixelGeometry, Preparation, Visual};
use uncurses::color::Profile;
use uncurses::event::Key;
use uncurses::layout::Size;
use uncurses_images::{
    Backend, FilterType, Fit, Image, ImageOptions, SymbolSet,
    dither::{Bayer8x8, FloydSteinberg, Noise},
    quant::{KMeans, Wu},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Native(Backend),
    Placeholders,
    Symbols,
}

const MODES: &[(Mode, &str)] = &[
    (Mode::Symbols, "symbols"),
    (Mode::Native(Backend::Kitty), "kitty"),
    (Mode::Placeholders, "placeholders"),
    (Mode::Native(Backend::Iterm2), "iterm2"),
    (Mode::Native(Backend::Sixel), "sixel"),
];
const SYMBOLS: &[(&str, SymbolSet)] = &[
    ("default", SymbolSet::DEFAULT),
    ("all", SymbolSet::ALL),
    ("half", SymbolSet::HALF),
    ("block", SymbolSet::BLOCK),
    ("braille", SymbolSet::BRAILLE),
    ("ascii", SymbolSet::ASCII),
    ("quadrant", SymbolSet::QUADRANT),
    ("sextant", SymbolSet::SEXTANT),
    ("octant", SymbolSet::OCTANT),
    ("legacy", SymbolSet::LEGACY),
];
const FITS: &[(&str, Fit)] = &[
    ("contain", Fit::Contain),
    ("cover", Fit::Cover),
    ("stretch", Fit::Stretch),
    ("original", Fit::Original),
];
const FILTERS: &[(&str, FilterType)] = &[
    ("nearest", FilterType::Nearest),
    ("triangle", FilterType::Triangle),
    ("catmull-rom", FilterType::CatmullRom),
    ("gaussian", FilterType::Gaussian),
    ("lanczos3", FilterType::Lanczos3),
];
const DITHERS: &[&str] = &["none", "floyd-steinberg", "bayer8x8", "noise"];
const QUANTIZERS: &[&str] = &["wu", "k-means"];
const COLORS: &[u16] = &[256, 128, 64, 32, 16, 8, 4, 2];
const CONTROLS: &str =
    "Tab/p protocol  s symbols  f fit  r filter  d dither  k quantizer  c colors  q quit";

struct ImageDemo {
    image: Image,
    mode: usize,
    symbols: usize,
    fit: usize,
    filter: usize,
    dither: usize,
    quantizer: usize,
    colors: usize,
    selection: String,
}

impl ImageDemo {
    fn new(image: Image, mode: Mode) -> Self {
        Self {
            image,
            mode: MODES.iter().position(|&(value, _)| value == mode).unwrap(),
            symbols: 0,
            fit: 0,
            filter: 1,
            dither: 1,
            quantizer: 0,
            colors: 0,
            selection: "symbols: default".to_owned(),
        }
    }

    fn options(&self, pixels: Option<Size>) -> ImageOptions {
        let mut options = ImageOptions {
            fit: FITS[self.fit].1,
            filter: FILTERS[self.filter].1,
            dither: match self.dither {
                0 => None,
                1 => Some(Arc::new(FloydSteinberg::default())),
                2 => Some(Arc::new(Bayer8x8::default())),
                3 => Some(Arc::new(Noise::default())),
                _ => unreachable!("the dither index wraps at the list length"),
            },
            quantizer: match self.quantizer {
                0 => Arc::new(Wu),
                1 => Arc::new(KMeans::default()),
                _ => unreachable!("the quantizer index wraps at the list length"),
            },
            max_colors: COLORS[self.colors],
            ..ImageOptions::default()
        };
        if let Some(pixels) = pixels {
            options.cell_pixels = pixels.into();
        }
        options
    }
}

impl Content for ImageDemo {
    fn title(&self) -> String {
        format!("Images: {} | {}", MODES[self.mode].1, self.selection)
    }

    fn controls(&self) -> &str {
        CONTROLS
    }

    fn geometry(&self) -> PixelGeometry {
        if MODES[self.mode].0 == Mode::Symbols {
            PixelGeometry::Optional
        } else {
            PixelGeometry::Required
        }
    }

    fn on_key(&mut self, key: &Key) -> bool {
        if key.matches_any(["tab", "p", "shift+tab"]) {
            let step = if key.matches("shift+tab") {
                MODES.len() - 1
            } else {
                1
            };
            self.mode = (self.mode + step) % MODES.len();
            self.selection = "Shift+Tab reverses".to_owned();
        } else if key.matches("s") {
            self.symbols = (self.symbols + 1) % SYMBOLS.len();
            self.selection = format!("symbols: {}", SYMBOLS[self.symbols].0);
        } else if key.matches("f") {
            self.fit = (self.fit + 1) % FITS.len();
            self.selection = format!("fit: {}", FITS[self.fit].0);
        } else if key.matches("r") {
            self.filter = (self.filter + 1) % FILTERS.len();
            self.selection = format!("filter: {}", FILTERS[self.filter].0);
        } else if key.matches("d") {
            self.dither = (self.dither + 1) % DITHERS.len();
            self.selection = format!("dither: {}", DITHERS[self.dither]);
        } else if key.matches("k") {
            self.quantizer = (self.quantizer + 1) % QUANTIZERS.len();
            self.selection = format!("quantizer: {}", QUANTIZERS[self.quantizer]);
        } else if key.matches("c") {
            self.colors = (self.colors + 1) % COLORS.len();
            self.selection = format!("colors: {}", COLORS[self.colors]);
        } else {
            return false;
        }
        true
    }

    fn prepare(&mut self, context: Preparation) -> io::Result<Visual> {
        let size = Size::new(12, 4);
        let options = self.options(context.cell_pixels);
        match MODES[self.mode].0 {
            Mode::Native(backend) => Ok(Visual::Native(
                self.image.prepare(
                    backend,
                    size,
                    context
                        .cell_pixels
                        .expect("native preparation requires pixel geometry"),
                    &options,
                )?,
            )),
            Mode::Placeholders => {
                if context.profile != Profile::TrueColor {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "Kitty placeholders require a true-color terminal profile",
                    ));
                }
                let prepared = self.image.kitty_placeholders(
                    size,
                    context
                        .cell_pixels
                        .expect("placeholder preparation requires pixel geometry"),
                    &options,
                )?;
                Ok(Visual::Placeholders(prepared))
            }
            Mode::Symbols => Ok(Visual::Cells(self.image.symbols_with_policy(
                size,
                &options,
                context.profile,
                SYMBOLS[self.symbols].1.clone(),
                context.width_mode,
                context.eaw_wide,
            )?)),
        }
    }
}

fn main() -> io::Result<()> {
    let mut mode = Mode::Symbols;
    let mut options = Options::default();
    let mut path = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--kitty" => mode = Mode::Native(Backend::Kitty),
            "--iterm2" => mode = Mode::Native(Backend::Iterm2),
            "--sixel" => mode = Mode::Native(Backend::Sixel),
            "--placeholders" => mode = Mode::Placeholders,
            "--symbols" => mode = Mode::Symbols,
            "--inline" => options.inline = true,
            "--dump" => options.dump = true,
            "--overlay" | "--dialog" => options.dialog = true,
            "--dump-dialog" => {
                options.dump = true;
                options.dump_dialog = true;
            }
            "--dump-keys" => {
                let keys = arguments.next().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--dump-keys needs comma-separated keys",
                    )
                })?;
                options.replay_keys = keys
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<_, _>>()
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
                options.dump = true;
            }
            "--cell-pixels" => {
                options.cell_pixels = Some(placements_drag::parse_cell_pixels(arguments.next())?);
            }
            "--help" | "-h" => {
                println!(
                    "images_drag [--symbols|--kitty|--placeholders|--iterm2|--sixel] \
                     [--inline] [--cell-pixels WxH] [--overlay|--dialog] [--dump] [--dump-dialog] [--dump-keys KEYS] [image]\n\
                     {CONTROLS}\n\
                     Shift+Tab reverses protocols. Drag or use arrows to move.\n\
                     Space toggles a dialog. Shift+arrows move the dialog.\n\
                     Dithering affects Sixel and limited-color symbols.\n\
                     Quantization and palette size affect Sixel.\n\
                     Select protocols supported by your terminal."
                );
                return Ok(());
            }
            value if !value.starts_with('-') && path.is_none() => path = Some(argument),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown argument: {argument}"),
                ));
            }
        }
    }
    let image = match path {
        Some(path) => Image::open(path)?,
        None => {
            let mut pixels = Vec::with_capacity(32 * 32 * 3);
            for y in 0..32 {
                for x in 0..32 {
                    pixels.extend_from_slice(&[x * 8, y * 8, 160]);
                }
            }
            Image::from_rgb(32, 32, pixels)?
        }
    };
    placements_drag::run_interactive(options, ImageDemo::new(image, mode))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uncurses::text::WidthMode;

    fn demo() -> ImageDemo {
        ImageDemo::new(
            Image::from_rgb(2, 1, vec![255, 0, 0, 0, 255, 0]).unwrap(),
            Mode::Symbols,
        )
    }

    #[test]
    fn protocols_cycle_both_directions_and_preserve_options() {
        let mut demo = demo();
        demo.on_key(&"f".parse().unwrap());
        let options = demo.options(Some((16, 16).into()));
        for &(mode, _) in MODES.iter().skip(1).chain(&MODES[..1]) {
            assert!(demo.on_key(&"tab".parse().unwrap()));
            assert_eq!(MODES[demo.mode].0, mode);
            assert_eq!(demo.options(Some((16, 16).into())), options);
        }
        demo.on_key(&"shift+tab".parse().unwrap());
        assert_eq!(MODES[demo.mode].0, Mode::Native(Backend::Sixel));
        demo.on_key(&"p".parse().unwrap());
        assert_eq!(MODES[demo.mode].0, Mode::Symbols);
    }

    #[test]
    fn every_control_wraps_and_prepares_valid_content() {
        let mut demo = demo();
        for (key, count) in [
            ("s", SYMBOLS.len()),
            ("f", FITS.len()),
            ("r", FILTERS.len()),
            ("d", DITHERS.len()),
            ("k", QUANTIZERS.len()),
            ("c", COLORS.len()),
        ] {
            let before = demo.options(Some((8, 16).into()));
            let symbols = demo.symbols;
            for _ in 0..count {
                assert!(demo.on_key(&key.parse().unwrap()));
                demo.prepare(Preparation {
                    width_mode: WidthMode::Wc,
                    eaw_wide: false,
                    profile: Profile::TrueColor,
                    cell_pixels: Some((8, 16).into()),
                })
                .unwrap();
            }
            assert_eq!(demo.options(Some((8, 16).into())), before);
            assert_eq!(demo.symbols, symbols);
        }
        assert!(!demo.on_key(&"x".parse().unwrap()));
    }

    #[test]
    fn selected_options_reach_image_preparation() {
        let mut demo = demo();
        for key in ["f", "r", "d", "k", "c", "s"] {
            demo.on_key(&key.parse().unwrap());
        }
        let options = demo.options(Some((16, 12).into()));
        assert_eq!(options.fit, Fit::Cover);
        assert_eq!(options.filter, FilterType::CatmullRom);
        assert!(options.dither.unwrap().same(&Bayer8x8::default()));
        assert!(options.quantizer.same(&KMeans::default()));
        assert_eq!(options.max_colors, 128);
        assert_eq!(options.cell_pixels, (16, 12));
        assert_eq!(SYMBOLS[demo.symbols].1, SymbolSet::ALL);
    }
}
