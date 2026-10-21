# uncurses-image

Images for uncurses screens and ordinary cell surfaces.

This crate requires Rust 1.90 or later.

The crate supports Kitty native graphics, Kitty Unicode placeholders, relative image
groups, iTerm2 images, Sixel, and an automatic symbol fallback.
Fit options include contain, cover, stretch, original size, scale, custom ratios,
and source crops.

Keep `Image` values across frames to share decoded pixels and terminal resources.
Use `Image::from_rgb(width, height, pixels)` for packed RGB bytes.
Use `Image::from_rgba(width, height, pixels)` when the bytes include alpha.
`ImageRenderer::draw` stages native regions or ordinary cells.
`Screen::render` owns output, text order, and resource completion.

Use `Image::kitty_placeholders` for Kitty images that move with ordinary text.
Use `ImageGroup::attach` for relative images that follow those cells automatically.
The last anchor's removal removes the dependent native images.

Use `ImageCapabilities::detect(program.capabilities(), program.env())` for automatic detection.
It reads Kitty replies, DA1 Sixel support, and identity hints from XTVERSION and environment variables.
Confirm the required protocol support before explicit native calls.
Automatic selection uses common block and border symbols when support is unknown.
With Kitty support, it selects Unicode placeholders for true color and at most
297 cells per dimension. Otherwise, it selects native Kitty placements.
This policy assumes Unicode support from basic Kitty graphics support.
Select `Protocol::Kitty` for native-only terminals.
Sixel also requires compatible cursor and character-erasure modes.

Use `Protocol::Symbols(SymbolSet::BRAILLE)` to select a symbol family.
Combine families with `|` and exclude glyphs with `-`:

```rust
use uncurses_image::{ImageRenderer, Protocol, SymbolSet};

let renderer = ImageRenderer {
    protocol: Protocol::Symbols(
        (SymbolSet::BLOCK | SymbolSet::BORDER) - SymbolSet::DIAGONAL,
    ),
    ..Default::default()
};
```

`SymbolSet` covers all symbol classes in the Chafa manual, plus octants and letter
subsets. Parse selectors such as `"block+border-diagonal"` for ordered selection.
Use `Image::symbols` to build an ordinary cell buffer directly.
It uses code-point measurement and one-cell ambiguous characters.
Use `Image::symbols_with_policy` to pass a destination's `width_mode()` and `eaw_wide()`.
`ImageRenderer::draw` reads these policies from the screen automatically.
Set `screen.set_eaw_wide(true)` when the terminal uses two-cell ambiguous characters.
Redraw text and images after a policy change.

`SymbolSet::NARROW` and `WIDE` follow the selected policy, including unions and exclusions.
`SymbolSet::UPPER_LOWER` uses two samples per cell for undithered or true-color output
when its glyphs occupy one cell. Two-cell half blocks use the general matcher.
`SymbolSet::LEFT_RIGHT` selects left and right halves.
Use the `upper_lower` and `left_right` selectors to select these families by name.
Active palette dithering uses the shared 8x8 sample grid.
Symbol masks approximate the terminal font. Recent Unicode mosaics and wide
glyphs require font support.

The implementation uses independent geometric masks and the public-domain
letter masks from the MIT-licensed `font8x8` crate.
Font import is outside this API's scope.
Select `Protocol::Kitty` for native placements or `Protocol::KittyPlaceholders`
for ordinary placeholder cells.

See the [image guide](https://uncurses.org/docs/guides/images/) and
[API reference](https://uncurses.org/api/uncurses_image/).
Run the synthetic example from the repository root:

```sh
cargo run -p examples --example images
```

Add `-- --kitty` for Kitty placements, placeholder cells, and relative groups.
Add `-- --kitty-placeholders --preview` to demonstrate explicit placeholder selection.
Add `-- --auto-kitty --preview` to demonstrate automatic Unicode selection with Kitty support.
Add `--inline` for the normal terminal buffer.

For a draggable image with protocol selection, run:

```sh
cargo run -p examples --example images_drag
```

Append `-- image.png` to load a file. Drag with the left mouse button.
Keys `0` through `4` select Auto, symbols, iTerm2, Sixel, and Kitty native.
Key `5` selects Kitty Unicode placeholders, which require true color.
`Tab` cycles all six choices. `s` selects symbols and cycles every symbol family.
The footer shows the current family. `f` cycles fit modes, and `q` or `Esc` quits.
Forced native protocols require a compatible terminal.

For the same mouse controls in an inline viewport, run:

```sh
cargo run -p examples --example images_inline
cargo run -p examples --example images_inline -- image.png
```

The example uses up to 16 rows in the normal screen and preserves earlier output.
Space inserts a log line above the viewport. Mouse coordinates follow the viewport
after log insertion and terminal resize. Both drag examples share their implementation.

Compare every symbol family with the gallery:

```sh
cargo run -p examples --example image_symbols
cargo run -p examples --example image_symbols -- --symbols 'quad+braille' --plain
cargo run -p examples --example image_symbols -- --symbols upper_lower --symbols left_right
cargo run -p examples --example image_symbols -- --eaw-wide --symbols upper_lower
```

Append an image path to use your own picture. `--plain` uses light glyphs on a
dark background without color escapes.
Use `--eaw-wide` for two-cell ambiguous characters, or `--grapheme` for grapheme measurement.
Match these flags to the terminal's settings. The gallery passes its screen policy to the image renderer.

## Quantization and dithering

Set `ImageOptions::dither` for Sixel and indexed or monochrome symbols.
Defaults match icy_sixel 0.7: Wu, 256 colors, and Floyd-Steinberg at strength 0.875.
The `quant` module provides `Wu`, `KMeans`, and the `Quantize` trait.
The `dither` module provides `FloydSteinberg`, `Bayer8x8`, `Noise`, and the `Dither` trait.

```rust
use std::sync::Arc;
use uncurses_image::{dither, quant, ImageOptions};

let options = ImageOptions {
    quantizer: Arc::new(quant::KMeans::default()),
    max_colors: 64,
    dither: Some(Arc::new(dither::FloydSteinberg::default())),
    ..Default::default()
};
```

Set `dither: None` for undithered output.
Built-ins own their settings and default to 1x1 grain.
Grain axes accept 1, 2, 4, or 8. Use 4x4 grain for coarse symbol patterns.
Palette matching and diffusion use Oklab.
True-color symbols, Kitty, and iTerm2 retain full-color output.

Custom algorithms implement the same traits and use `Arc` handles.
Built-ins compare configurations by value. Custom implementations reuse their shared handles
or implement `same` for value-based frame reuse. Keep configurations immutable while in use.

Sixel generates adaptive palettes and dithers before clipping.
Indexed and monochrome symbols use fixed palettes.
`Quantize::prepare` returns a `PreparedImage` with the palette, lookup, and shared converted pixels.
Pass `prepared.context()` to a dither to reuse that data.
Wu retains its histogram lookup. K-means shares converted pixels and prepares lookup for the destination colors.
Custom quantizers can implement only `palette` and use the default preparation.
Use `DitherContext::new` for a fixed palette, and its accessors to read pixels and colors.
The default `Quantize::quantize` method supplies shared Oklab mapping for custom palettes.

Run the built-in and custom comparisons:

```sh
cargo run -p examples --example image_quantizers
cargo run -p examples --example image_dithers
cargo run -p examples --example image_dithers -- --dither-grain 2x4 --dither-intensity 0.5
cargo run -p examples --example images -- --sixel --quantizer kmeans --colors 64 --preview
cargo run -p examples --example image_symbols -- --plain --symbols braille --dither fs --dither-grain 4x4
```

The drag example uses `d` to cycle dithering modes.

## Source layout

`src/lib.rs` defines the crate exports. `image.rs` owns image data, decoding,
and rasterization. `options.rs` defines the public settings.
`geometry.rs` contains shared geometry calculations.
`quant.rs` exposes palette generators. `dither.rs` exposes palette mapping contracts.
Built-in palette, indexed, and prepared output share one quantization core.
The core retains only the data each output needs and deduplicates large undithered images.
`color.rs` shares Oklab conversion, prepared lookups, and result validation.
`dither/builtins.rs` implements the built-in algorithms through the `Dither` trait.

`renderer.rs` selects a backend and connects native images to screen regions.
`protocols/` contains capability detection and the Kitty, iTerm2, Sixel, and
symbol backends. The symbol backend separates selection, glyph data, and rendering.
Backend tests stay beside their implementations.
