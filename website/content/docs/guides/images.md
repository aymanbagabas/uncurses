---
title: "Draw images"
weight: 16
---

Use `uncurses-image` for native terminal graphics or a symbol fallback.
Keep decoded images across frames. The screen owns terminal output and resource
cleanup.

## Add the crate

`uncurses-image` requires Rust 1.90 or later.

Use the repository versions together:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
uncurses-image = { git = "https://github.com/aymanbagabas/uncurses" }
```

The default format features enable PNG and JPEG. Enable `gif` or `webp` when
you need those decoders. Decoding selects a still image and applies orientation
metadata. It does not play animations.

`Image::open` decodes a file. `Image::decode` accepts encoded bytes.
`Image::from_rgb` accepts three bytes per pixel and sets alpha to 255.
`Image::from_rgba` accepts four bytes per pixel, including alpha.
Both constructors require tightly packed rows and an exact byte count.

## Draw with a fallback

This complete sample writes a frame into memory:

```rust
use uncurses::layout::Rect;
use uncurses::screen::Screen;
use uncurses_image::{Image, ImageOptions, ImageRenderer, Protocol, SymbolSet};

fn main() -> std::io::Result<()> {
    let image = Image::from_rgba(2, 2, vec![
        255, 0, 0, 255, 0, 255, 0, 255,
        0, 0, 255, 255, 255, 255, 255, 255,
    ])?;
    let mut screen = Screen::new(Vec::new(), (20, 8));
    let renderer = ImageRenderer::default();
    let selected = renderer.draw(
        &mut screen, "picture", Rect::new(2, 1, 8, 4),
        &image, &ImageOptions::default(),
    )?;
    assert_eq!(selected, Protocol::Symbols(SymbolSet::DEFAULT));
    assert!(screen.writer().is_empty());
    screen.render()?;
    assert!(!screen.writer().is_empty());
    Ok(())
}
```

For interactive applications, use the screen from `Program::screen_mut()`.
Select the alt screen only when the application needs fullscreen output.

Automatic selection prefers Kitty placeholders, native Kitty placements, iTerm2,
Sixel, and symbols, in that order.
Automatic placeholders require detected Kitty support, true color, and at most 297 cells per dimension.
If those requirements are unmet, Kitty uses native placements.
Unknown support selects common block and border symbols.

Call `ImageCapabilities::detect(program.capabilities(), program.env())` to combine
terminal replies with the program's environment. Call it again after replies arrive.
Kitty uses graphics replies and identity hints for kitty, Ghostty, WezTerm, Rio, and Konsole.
Sixel uses capability `4` in a VT220-or-later DA1 reply.
The first DA1 parameter identifies the terminal class rather than a capability.
For iTerm2, the detector recognizes iTerm2, WezTerm, Rio, Konsole, and mintty.
These include `TERM_PROGRAM`, `LC_TERMINAL`, session markers, and terminal families in `TERM`.
Both Kitty and iTerm2 detection use XTVERSION names, with or without a version suffix.
A basic Kitty graphics reply confirms graphics support.
Automatic selection assumes Unicode placeholder support when Kitty support is detected.
Select `Protocol::Kitty` for terminals that support native placements only.

`ImageCapabilities::from(program.capabilities())` uses only terminal replies, including XTVERSION.
Override the detected flags when terminal settings or multiplexer behavior require it.
Explicit `Protocol` choices trust the application's selection.

iTerm2 output requires the `png` feature. Sixel requires cursor-relative output
and character erasure of graphics in the active terminal mode.
A device-attribute reply alone does not establish those Sixel settings.
Sixel also requires one spare row beneath the image. Automatic selection uses
symbols when that row is unavailable.

## Drag an image inline

Run the inline example to keep earlier terminal output visible:

```sh
cargo run -p examples --example images_inline
cargo run -p examples --example images_inline -- image.png
```

The viewport uses up to 16 rows in the normal screen.
Drag the image with the left mouse button. Space adds a log line above the viewport.
Keys `0` through `5` select Auto, symbols, iTerm2, Sixel, Kitty native, and Kitty placeholders.
Press `Tab` to cycle protocols, `s` for symbol sets, `d` for dithers, or `f` for fit modes.
Press `q`, `Esc`, or `Ctrl-C` to quit.
Native selections require a compatible terminal.

The example renders before it calls `Program::request_origin()`.
It uses `Program::mouse_to_origin()` to translate terminal coordinates into viewport coordinates.
After log insertion or terminal resize, it requests the origin again.
The example clears its viewport before `Program::finish()` restores the terminal.
The fullscreen `images_drag` example shares the same drawing and drag logic.

## Select image symbols

`Protocol::Symbols(SymbolSet)` approximates an image with ordinary cells.
Use `|` to combine families and `-` to exclude glyphs.
Exclusion removes matching glyphs even when they belong to several families.

`ImageRenderer::draw` uses the screen's color profile and text-width policy.
Set `screen.set_eaw_wide(true)` when the terminal uses two-cell ambiguous characters.
Use `screen.set_grapheme_clusters()` to select its text measurement mode.
These settings must match the terminal.
After a policy change, redraw text and images before `render()` repaints the screen.

`Image::symbols` uses code-point measurement and one-cell ambiguous characters.
For a direct buffer with another policy, pass the destination's policy to `Image::symbols_with_policy`:

```rust
use uncurses::buffer::Surface;
use uncurses::color::Profile;
use uncurses::layout::{Position, Size};
use uncurses::screen::Screen;
use uncurses::text::TextSurface;
use uncurses_image::{Image, ImageOptions, SymbolSet};

fn main() -> std::io::Result<()> {
    let image = Image::from_rgba(1, 1, vec![255, 100, 0, 255])?;
    let mut screen = Screen::new(Vec::new(), (8, 4));
    screen.set_color_profile(Profile::TrueColor);
    screen.set_eaw_wide(true);
    let symbols = (SymbolSet::BLOCK | SymbolSet::BORDER) - SymbolSet::DIAGONAL;
    let cells = image.symbols_with_policy(
        Size::new(8, 4), &ImageOptions::default(), screen.color_profile(), symbols,
        screen.width_mode(), screen.eaw_wide(),
    )?;
    assert_eq!(cells.width(), 8);
    cells.draw(&mut screen, Position::ORIGIN);
    screen.render()?;
    Ok(())
}
```

The built-in families cover the symbol classes in the
[Chafa manual](https://hpjansson.org/chafa/man/), plus octants and letter subsets.
Select a class by its name, such as `"braille".parse::<SymbolSet>()?`.
Use `upper_lower` for upper and lower halves, or `left_right` for left and right halves.
Use `"block+border-diagonal"` for ordered additions and exclusions.
A leading sign starts from the default set.
Select built-in characters with `[abc]`, `u2580`, or `u2580..u259f`.

`SymbolSet::NARROW` selects one-cell glyphs under the active policy.
`SymbolSet::WIDE` selects two-cell glyphs, including ambiguous characters when the policy makes them wide.
Combined selectors preserve this behavior. For example, `"block-wide"` excludes every block that occupies two cells.
`contains_with_policy` and `is_empty_with_policy` let applications inspect a selection under a specific policy.

`SymbolSet::UPPER_LOWER` uses two samples per cell for undithered or true-color output
when its glyphs occupy one cell. Two-cell half blocks use the general matcher.
Active palette dithering uses the shared 8x8 sample grid.
Other selections compare glyph shapes and fit foreground and background colors.
Color profiles constrain those colors before the matcher compares candidates.
Colorless output assumes light glyphs on a dark background.
Wide glyphs occupy two cells; a wide-only selection leaves an odd final column blank.

The masks approximate the font in your terminal.
Confirm font support before selecting recent mosaics or wide glyphs.
The implementation uses independent geometric masks and public-domain letter masks
from the MIT-licensed `font8x8` crate.
It targets the same symbol families, rather than identical Chafa output.
Font import is outside this API's scope.

Run the gallery to compare the families:

```sh
cargo run -p examples --example image_symbols
cargo run -p examples --example image_symbols -- --symbols 'quad+braille' --plain
cargo run -p examples --example image_symbols -- --symbols upper_lower --symbols left_right
cargo run -p examples --example image_symbols -- --eaw-wide --symbols upper_lower
```

Append an image path to replace the generated pattern.
Use `--eaw-wide` for two-cell ambiguous characters, or `--grapheme` for grapheme measurement.
Match these flags to the terminal's settings.
The gallery uses a screen with these policies and draws through `ImageRenderer`.
The [SymbolSet reference](/api/uncurses_image/struct.SymbolSet.html) lists all classes and selectors.

## Dither limited-color output

Use dithering to approximate gradients with a limited palette.
Set `ImageOptions::dither` before drawing Sixel or indexed-color symbols.
It also applies to monochrome symbols.

```rust
use uncurses::color::Profile;
use uncurses::layout::Size;
use std::sync::Arc;
use uncurses_image::{dither::FloydSteinberg, Image, ImageOptions, SymbolSet};

fn main() -> std::io::Result<()> {
    let image = Image::from_rgb(2, 1, vec![64, 64, 64, 192, 192, 192])?;
    let options = ImageOptions {
        dither: Some(Arc::new(FloydSteinberg {
            grain: (4, 4),
            ..Default::default()
        })),
        ..Default::default()
    };
    let cells = image.symbols(Size::new(16, 4), &options, Profile::Ascii, SymbolSet::BRAILLE)?;
    assert_eq!(cells.width(), 16);
    Ok(())
}
```

The default is Floyd-Steinberg at strength 0.875, matching icy_sixel 0.7.
Set `dither: None` for undithered output.
The `dither` module also provides `Bayer8x8` and `Noise`.
Algorithm settings belong to their concrete types.
Use grain dimensions of 1, 2, 4, or 8 on each axis.
Built-ins default to 1x1 grain. Use 4x4 grain to preserve coarse symbol patterns.
Glyph matching can average away a pattern that is finer than the glyph's visible parts.
The methods preserve caller options.

Floyd-Steinberg strength must be finite and within 0 through 1.
Bayer8x8 and noise intensity must be finite and nonnegative.
Zero strength maps each pixel to its nearest palette color.
Built-ins use Oklab for palette matching and diffusion.
True-color symbols, Kitty, and iTerm2 preserve their full-color output.

Sixel generates a palette for the complete image and dithers before extracting visible fragments.
Moving or clipping an image preserves its pattern.
An identical frame produces identical pixels, including noise mode.

To supply your own algorithm, implement [`Dither`](/api/uncurses_image/dither/trait.Dither.html).
Built-in algorithms use the same trait and receive the same validation.
Store it in `Some(Arc::new(your_dither))`.
The callback receives sampled pixels and destination colors.
Read them through `DitherContext::pixels()` and `DitherContext::palette()`.
Return one valid palette index per pixel, in row order.
Use `DitherContext::nearest` to reuse Oklab palette matching.
Use `DitherContext::new` when your algorithm maps a modified image against a fixed palette.
The image layer rejects invalid results and propagates callback errors.

Keep a custom handle across frames, or implement `same` for value-based reuse.
Built-ins compare their configurations by value.
Create a new handle when its configuration changes.
The `image_dithers` example includes a custom diagonal pattern:

```sh
cargo run -p examples --example image_dithers
cargo run -p examples --example image_symbols -- --plain --symbols braille --dither bayer8x8 --dither-grain 4x4
cargo run -p examples --example images -- --sixel --dither noise --preview
```

The examples also accept `--dither-grain WIDTHxHEIGHT` and `--dither-intensity VALUE`.
The CLI accepts `bayer8x8` for Bayer dithering and `fs` for Floyd-Steinberg.
In `images_drag`, press `d` to cycle dithering modes.
The [dither reference](/api/uncurses_image/dither/) describes the algorithms and settings.

## Choose an adaptive palette

Sixel uses Wu with a maximum of 256 colors by default.
Choose K-means for palette refinement, or reduce the color budget for smaller output.

```rust
use std::sync::Arc;
use uncurses_image::{quant::KMeans, ImageOptions};

let options = ImageOptions {
    quantizer: Arc::new(KMeans::default()),
    max_colors: 64,
    ..Default::default()
};
```

Implement [`Quantize`](/api/uncurses_image/quant/trait.Quantize.html) to supply another palette generator.
Its context contains the sampled image and requested color limit.
The pipeline calls `prepare` when dithering follows.
Its default implementation calls `palette` and prepares nearest-color lookup.
For undithered output, it calls `quantize`, whose default implementation uses Oklab nearest-color mapping.
Override that method when your quantizer can produce indices more efficiently.

The pipeline validates palette size, dimensions, and every returned index.
Indexed symbols retain their fixed terminal palette; full-color output retains its original raster.
Quantization and dithering process the complete Sixel raster before clipping.

```sh
cargo run -p examples --example image_quantizers
cargo run -p examples --example images -- --sixel --quantizer kmeans --colors 64 --preview
cargo run -p examples --example images -- --sixel --dither none --preview
```

### Reuse a prepared image

Call `Quantize::prepare` to reuse a palette and converted pixels across dither calls.
The returned `PreparedImage` binds that data to its source image.
Pass its context to a built-in or custom dither:

```rust
use uncurses_image::{
    RgbImage,
    dither::{Dither, FloydSteinberg, Bayer8x8},
    quant::{Quantize, QuantizeContext, Wu},
};

fn main() -> std::io::Result<()> {
    let mut pixels = RgbImage::new(64, 32);
    for (x, y, pixel) in pixels.enumerate_pixels_mut() {
        pixel.0 = [(x * 4) as u8, (y * 8) as u8, ((x + y) * 2) as u8];
    }
    let prepared = Wu.prepare(QuantizeContext {
        pixels: &pixels,
        max_colors: 16,
    })?;
    let diffusion = FloydSteinberg::default().dither(prepared.context())?;
    let ordered = Bayer8x8::default().dither(prepared.context())?;
    assert_eq!(diffusion.len(), 64 * 32);
    assert_eq!(ordered.len(), diffusion.len());
    assert_ne!(ordered, diffusion);
    Ok(())
}
```

Wu retains its histogram lookup. K-means shares its converted pixels and prepares lookup for the destination colors.
Custom quantizers can implement only `palette` and use the default preparation.
Override `prepare` to delegate to another quantizer or construct a `PreparedImage` with your palette.
`DitherContext::nearest` always selects nearest colors, including with a prepared Wu image.
Wu's histogram lookup can select different colors than a nearest-color search.

Sixel rounds destination colors before diffusion and uses those colors for error feedback.
The quantizer example compares two dithers on each prepared image, including a custom gray palette.

## Fit the destination

The destination rectangle uses cells. Fit calculations use physical pixels,
because terminal cells are usually taller than they are wide.

| Option | Behavior |
|---|---|
| `Fit::Contain` | Preserve the ratio and show the whole source. |
| `Fit::Cover` | Preserve the ratio and crop the excess. |
| `Fit::Stretch` | Resize each axis to fill the destination. |
| `Fit::Original` | Start with the source pixel dimensions. |
| `scale` | Apply a positive, uniform zoom after the fit. |
| `aspect_ratio` | Override the source width-to-height ratio. |
| `crop` | Select a source rectangle before the fit. |
| `background` | Composite transparent pixels and fill unused space. |
| `filter` | Select the resampling filter. |

Images remain centered. Stretch uses the destination ratio.
The methods leave caller options unchanged.

Set `cell_pixels` from `Program::cell_pixels()` when the terminal reports it.
Use the returned `Size` as `(width, height)`. Request fresh cell dimensions after
a font or window change. The default `(8, 16)` is an estimate.
The window-pixel fallback can include terminal padding.

Symbols use the same physical fit as native images.
Their sampling resolution stays independent of `cell_pixels`.

## Keep frame order

For native images, reuse the placement key when the image moves.
Equal image data, options, and geometry reuse uploaded resources.
The same `Image` can supply several placements.

Later text, `fill_rect`, and `clear_rect` replace the affected image cells.
Native clipping preserves the rest of the image. `remove_region` restores the
current underlying text. A full `clear` or `fill` removes image declarations.
An identical rebuilt frame produces no extra bytes.

iTerm2 and Sixel output clears terminal text within each visible image fragment
before it draws the pixels. The screen retains the underlying text for restoration.
Later text edits remain visible in the clipped areas.

Symbols are ordinary cells. Clear their previous cells explicitly when they
move. Remove them with ordinary text operations.

## Use Kitty placeholder cells

Select `Protocol::Kitty` for native placements.
Select `Protocol::KittyPlaceholders` to draw placeholder cells through `ImageRenderer::draw`.
Automatic selection uses Kitty support, the color profile, and the destination dimensions to choose placeholders.
Use `Image::kitty_placeholders` to retain a prepared cell buffer for direct cell operations.

Confirm Unicode placeholder support and preserve a true-color profile.
The placeholder cells encode image identifiers in their colors.
The screen rejects a lower profile before output while their resources remain
bound.

```rust
use uncurses::buffer::SurfaceMut;
use uncurses::layout::{Position, Rect, Size};
use uncurses::screen::Screen;
use uncurses_image::{Image, ImageOptions};

fn main() -> std::io::Result<()> {
    let image = Image::from_rgba(1, 1, vec![255, 0, 0, 255])?;
    let cells = image.kitty_placeholders(Size::new(4, 2), &ImageOptions::default())?;
    let mut screen = Screen::new(Vec::new(), (20, 8));
    cells.draw(&mut screen, "picture", Position::new(2, 1))?;
    screen.render()?;
    screen.clear_rect(Rect::new(2, 1, 4, 2));
    cells.draw(&mut screen, "picture", Position::new(10, 1))?;
    screen.render()?;
    screen.clear();
    screen.render()?;
    Ok(())
}
```

Keep `Placeholders` across frames. Each dimension supports at most 297 cells.
Each cell contains complete row, column, and image information.
Clipped copies therefore keep their source coordinates.

For another surface, copy `Placeholders::cells()` with `Surface::draw`.
Bind its shared `resource()` on the destination screen with
`set_region_resource`. Full screen clear/fill removes these bindings.
An explicit binding retains the resource until removal, even after its cells
disappear.

## Build relative groups

`ImageGroup` combines layers with named parents and signed cell offsets.
Layers paint in declaration order. The group resolves nested offsets together,
then uses native relative placements around one anchor.
Missing parents and cycles return errors before output.

Use `ImageGroup::draw` with a regular native anchor. Redeclare it at another
position to move the group. Use `attach` to follow ordinary placeholder cells:

```rust
use uncurses::buffer::SurfaceMut;
use uncurses::cell::Cell;
use uncurses::layout::{Position, Size};
use uncurses::screen::Screen;
use uncurses_image::{Image, ImageGroup, ImageOptions};

fn main() -> std::io::Result<()> {
    let image = Image::from_rgba(1, 1, vec![0, 160, 255, 255])?;
    let options = ImageOptions::default();
    let anchor = image.kitty_placeholders(Size::new(1, 1), &options)?;
    let mut group = ImageGroup::new()?;
    group.set("picture", None, (0, 0), Size::new(8, 3), &image, &options)?;
    group.set("detail", Some("picture"), (4, 1), Size::new(4, 2), &image, &options)?;
    let mut screen = Screen::new(Vec::new(), (30, 10));
    anchor.draw(&mut screen, "anchor", Position::new(2, 2))?;
    group.attach(&mut screen, "pictures", &anchor)?;
    screen.render()?;
    screen.insert_lines(0, 1, 10, &Cell::BLANK);
    screen.render()?;
    screen.clear();
    screen.render()?;
    Ok(())
}
```

Confirm Kitty relative-placement support before these calls.
The anchor uses the minimum column and minimum row among its placeholder cells.
Row operations, column operations, and ordinary copies can move it.
The last anchor's removal removes the dependent native images.
Their declaration remains available if the anchor cells return.

Anchor movement establishes a new placement order. Later text clips the images
at their new position. The group clips images at screen edges and preserves text
in gaps between its layers.

`attach` snapshots the group. Attach it again after you change its layers.
`ImageGroup::remove` removes a layer and its descendants.
`Screen::remove_region` removes the whole group declaration.
Full screen clear/fill also removes it, so reattach after a complete frame reset.

## Resize, replay, and cleanup

Move or remove ordinary native image regions that exceed a resized screen.
Attached groups instead resolve their visible bounds from the current anchors.
Refresh pixel geometry when the terminal's cell dimensions change.

`insert_above` replays the displayed frame and preserves pending edits.
`release_regions` removes displayed native placements and resources for a handoff.
`Program` uses this lifecycle during session changes.
After an output error, recover the terminal, then call `invalidate` for a full
repaint.

## Run the example

For a draggable image with runtime protocol selection, run:

```sh
cargo run -p examples --example images_drag
```

Pass an image path after `--` to use your own PNG or JPEG.
Without a path, the example generates a color pattern.
Drag the image with the left mouse button.
Keys `0` through `4` select Auto, symbols, iTerm2, Sixel, and Kitty native.
Key `5` selects Kitty Unicode placeholders.
`Tab` cycles all six choices. `s` selects symbols and cycles every symbol family.
The footer shows the current family.
`f` cycles the fit modes. `q` or `Esc` quits.

Key `4` uses native placements, while `5` draws ordinary placeholder cells.
Both choices support dragging and the fit controls.
Placeholders require terminal support for Kitty Unicode placeholders and a true-color output profile.

Auto uses environment hints immediately and updates its choice as terminal replies arrive.
It selects symbols when support is unknown.
The other protocol keys force your choice, so use them only with compatible terminals.
The footer shows the requested protocol, selected backend, position, and cell geometry.
The example requests cell dimensions at startup and after resize.
It reserves a row beneath the image for Sixel and clears the old image when it moves.
The minimum window size is 24 columns and 11 rows.

For the staged image demonstration, run:

```sh
cargo run -p examples --example images
```

The example generates its pixels locally. It demonstrates fit modes, scale,
ratio, crop, movement, text overlays, replay, and cleanup.
Add `-- --kitty` for native Kitty images, placeholder cells, and relative groups.
Use `-- --symbols braille` to select a symbol family.
Use `-- --iterm2` or `-- --sixel` only with a compatible terminal.
Add `--inline` for the normal buffer, or `--preview` for byte counts.

With Kitty installed, run its native parser checks:

```sh
cargo run -p examples --example images -- --kitty --dump |
  kitty +runpy 'import runpy; runpy.run_path("examples/tests/images_native.py", run_name="__main__")'
```

The [API reference](/api/uncurses_image/) contains all parameters and contracts.
