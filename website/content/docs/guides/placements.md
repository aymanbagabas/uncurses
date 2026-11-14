---
title: "Draw ANSI placements"
weight: 35
---

Use a placement when encoded terminal output occupies a known rectangle.
The screen tracks its geometry, schedules its bytes, and repaints text above it.
Your implementation encodes the protocol. The screen can also manage shared uploads and deletion.

## Stage a placement

Supply a nonzero ID, a rectangle, and `PlacementOptions`.
The screen copies the options, so you can reuse them when the placement moves.
The rectangle must fit inside the screen.

```rust
use uncurses::{
    layout::Rect,
    screen::{PlacementOptions, Screen},
    style::Style,
    text::{Encode, TextSurface},
};

fn main() -> std::io::Result<()> {
    let mut screen = Screen::new(Vec::new(), (30, 6));
    let options = PlacementOptions {
        draw: b"\x1b[31mPLACEMENT".to_vec(),
        erase: b"\x1b[9X".to_vec(),
        ..Default::default()
    };
    screen.set_placement(1, Rect::new(2, 2, 9, 1), &options)?;
    screen.set_str((5, 2), "TOP", Style::EMPTY);
    screen.render()?;
    println!("{:?}", screen.display().to_string());
    Ok(())
}
```

This example uses ordinary ANSI text as the opaque payload.
The renderer emits `PLACEMENT`, then writes `TOP` above its middle.
Run `cargo run -p examples --example placements` to move and remove the same placement.

Use `display()` or `encode()` to export the cell grid.
They replace placement references with blanks and preserve each cell's stored style.
The sample prints the exported string with escaped line breaks.

Register placements in their intended emission order, then write ordinary overlay cells.
An unchanged frame emits zero bytes.
Movement damages the previous and new areas, including unchanged overlay text.
Repaint the desired background when moving or removing content.

## Choose overwrite behavior

Static payloads retain a placement beneath ordinary text.
The renderer emits the payload before that text whenever the affected rows need a repaint.
Complete coverage removes the placement. Register it again when you restore the underlying content.

For cell-based composition, use a `PlacementEncoder` in `PlacementOptions::encoder`.
The screen divides the final visible cells into rectangles.
It calls `prepare` for changed content or geometry before any terminal output.
Prepared commands are reused for surrounding damage.
`PlacementContext::clip` is relative to the complete placement area.
The renderer positions each returned payload at that fragment's origin.
Keep the encoder in an `Arc` and reuse it across frames.
Prepared image placements use this path.
The portable `placements` example implements this interface for an ASCII label.

Set `erase_on_overwrite` for indivisible content such as sized text.
An overwrite then removes the complete placement before the replacement cells appear.
The remaining cells become blanks with their stored styles. Overlay cells keep their own styles.

## Draw sized text

Use `Screen::set_sized_text` to stage styled text in a terminal with Kitty text sizing support.
The helper computes the footprint, encodes the payload, and preserves the style on erasure.
An ordinary cell overwrite removes the complete block.

```rust
use uncurses::{
    color::Color,
    screen::{Screen, TextSizing},
    style::Style,
};

fn main() -> std::io::Result<()> {
    let sizing = TextSizing { scale: 2, width: 4, ..Default::default() };
    let style = Style::default().bold().fg(Color::White).bg(Color::Black);
    let mut screen = Screen::new(Vec::new(), (30, 8));
    screen.set_sized_text(1, (2, 2), "Demo", sizing, style)?;
    screen.render()?;
    Ok(())
}
```

Use `program.screen_mut().set_sized_text(...)` for a program's managed screen.
Call `program.screen_mut().render()` to emit the frame.
Use `screen.remove_placement(id)` to remove the block.

Run `cargo run -p examples --example sized_text_drag -- --overlay` in a terminal with Kitty text sizing support.

## Own image resources

Use `uncurses_images::ImageRenderer` for automatic protocol selection and resource management.
Create it from a `Program` with terminal reports and environment, or supply explicit `ImageCapabilities`.
It prefers Kitty placeholders, Kitty, iTerm2, Sixel, then symbols.
Unsupported paths or unsuitable geometry fall back to the next usable path.

Automatic detection uses the reported terminal name, then nonempty `TERM_PROGRAM`, then exact `TERM` values.
Kitty, Rio, and Ghostty identities imply Kitty graphics support. A Kitty graphics reply also confirms support.
Rio and Ghostty select placeholders. Kitty selects placeholders after a version report confirms 0.28 or later.

The `TERM` fallbacks are `xterm-kitty`, `xterm-ghostty`, `xterm-rio`, `rio`, and `wezterm`.
`xterm-kitty` selects native Kitty until a version report confirms placeholder support.
`xterm-ghostty`, `xterm-rio`, and `rio` select placeholders. `wezterm` selects iTerm2.
Generic values such as `xterm-256color` are inconclusive.

Automatic iTerm2 detection recognizes iTerm2, Rio, WezTerm, mintty, and Konsole.
For VS Code or xterm.js hosts with images enabled, set `ImageCapabilities::iterm2` explicitly.

Inside tmux, create the renderer with `ImageRenderer::from_program`.
It forwards each Kitty upload chunk, virtual placement, and deletion command through tmux passthrough.
Placeholder cells stay in the pane's text grid.
Enable `allow-passthrough` in tmux. The library leaves tmux configuration unchanged.
Forward terminal identity and graphics queries to the outer terminal, as the `images_auto` example demonstrates.
Automatic selection inside tmux uses placeholders or symbols.

`ImageRenderer::new` uses direct output. GNU Screen (`STY`) also permits placeholders and symbols, with resource forwarding supplied by the caller.
For manual forwarding, use `CellImage::upload` and `delete`.
Other transports require explicit resource forwarding. Nested multiplexers require a transport for each layer.

```rust
use uncurses::{layout::Rect, screen::Screen};
use uncurses_images::{Image, ImageCapabilities, ImageOptions, ImageRenderer};

fn main() -> std::io::Result<()> {
    let source = Image::from_rgb(1, 1, vec![255, 0, 0])?;
    let mut screen = Screen::new(Vec::new(), (30, 8));
    let mut images = ImageRenderer::new(ImageCapabilities::default());
    images.draw(&mut screen, 1, Rect::new(2, 2, 8, 4), &source, &ImageOptions::default())?;
    screen.render()?;
    images.remove(&mut screen, 1);
    screen.render()
}
```

Default capabilities select symbols. Use `ImageRenderer::from_program(&program)` for a terminal session.
Update `images.capabilities` with `ImageCapabilities::from_program(&program)` after new replies or changes in pixel geometry.
Draw retained images again after those changes or changes to the screen's color and width policies.

Finish managed image frames with `screen.render()`.
The screen uploads new resources and releases retired resources after successful frame output.
Use `images.remove(screen, id)` to stage removal.
Remove each image before the session ends, then render the frame.
Keep each renderer paired with one screen.
Repaint underlying content after movement or removal.

Run `cargo run -p examples --example images_auto` to try automatic selection.
Space moves or removes the image. Add `--dump` for output from all five paths.
The screen compresses placeholder runs and retains coordinate anchors at gaps.
Use `--dump-placeholders` to inspect frames with overlays, movement, and resize.
The low-level APIs below provide direct control over encoding and resource lifetime.

`uncurses-images` also prepares placements for an explicit protocol.
Their options include shared resource commands.
The screen uploads each resource and deletes it after the final managed placement disappears.
Cloned `PlacementResource` handles share identity and terminal lifetime.
Each new handle owns independent terminal resources with distinct protocol identifiers.
Its upload and deletion commands remain immutable.

```rust
use uncurses::{layout::Rect, screen::Screen};
use uncurses_images::{Backend, Image, ImageOptions};

fn main() -> std::io::Result<()> {
    let source = Image::from_rgb(1, 1, vec![255, 0, 0])?;
    let image = source.prepare(
        Backend::Kitty, (8, 4).into(), (8, 16).into(), &ImageOptions::default(),
    )?;
    let mut screen = Screen::new(Vec::new(), (30, 8));
    screen.set_placement(1, Rect::new(2, 2, 8, 4), &image.placement())?;
    screen.render()?;
    screen.remove_placement(1);
    screen.render()
}
```

Use the terminal's actual cell dimensions instead of the sample dimensions.
Prepare the image again when those dimensions change.
Keep one spare row below Sixel and iTerm2 images.

Managed images follow ordinary cell ownership.
Later cell writes cover the image cells they replace.
The image encoder prepares visible fragments after the screen resolves overlaps.
The application draws the image and dialog through their normal APIs.

Use `raw_placement(id)?` for explicit protocol IDs and native layer behavior.
Raw Kitty placements use z-index -1, below text foregrounds and above cell backgrounds.

The raster output checks verify fragment dimensions and text order.
Visual behavior still requires confirmation in Rio and iTerm2:

```sh
cargo run -p examples --example images_drag -- --sixel --overlay
cargo run -p examples --example images_drag -- --iterm2 --overlay
```

`--overlay` starts either drag example with the dialog visible. `--dialog` is an alias.
Space toggles the dialog. Shift+arrows move it.
Drag the image or use the arrow keys.
For Kitty placeholders, pass `CellImage::placement(screen)?` to `set_placement`.
This checks the screen's color profile and includes the shared cells and resources.
For manual output, use the raw upload, cell, and deletion accessors with an explicit resource lifetime.

## Commit inline output

Use `screen.commit_rows(count)` to release completed inline rows as permanent output.
Retained placements receive coordinates relative to the new origin.
Cell-backed placements can cross the boundary; their retained rows keep the same placement ID.
Commit whole opaque placements, including native images and sized text.
A boundary inside an opaque placement returns `InvalidInput` before any output.

Resources used by committed rows remain in the terminal after managed removal and program teardown.
Reuse application IDs for new managed images after a commit.
The encoder allocates fresh native instances and preserves committed copies.
The screen retains compact resource identities and releases unused historical upload buffers.
For raw native placements, use distinct protocol placement IDs for copies that remain visible.
Terminal reset or eviction can discard committed resources.

Run the portable placement example:

```sh
cargo run -p examples --example placements -- --commit
```

Inspect native-image and placeholder output:

```sh
cargo run -p examples --example images_auto -- --dump-committed
cargo run -p examples --example images_auto -- --dump-shared
```

## Handle changes and errors

Resize removes placements whose complete rectangle no longer fits.
Register a new placement after preparing suitable dimensions.
`insert_above` temporarily removes displayed placements, then restores the desired frame.
Program handoffs release displayed placements and their managed resources.
The next render restores retained placements and their uploads.
For explicit resource control, restore the required resources after a terminal reset or screen change.

A placement ID belongs to one screen and one positioned rectangle.
Register another ID for another instance.
Use `set_placement` to move content instead of translating its marker cells through buffer operations.
Cell-backed resource ownership covers registered rectangles.
Copies outside those rectangles require explicit resource lifetime management.

Malformed markers and unresolved references fail before frame output.
After an output failure, restore the terminal parser and cursor before calling `invalidate()` and retrying.
The screen discards partially transmitted buffers and retains placement cleanup records.
With explicit resource control, restore any shared resources affected by the failed output.
For managed images, `screen.invalidate()` also restores resource uploads on the next render.

See [`PlacementOptions`](/api/uncurses/screen/struct.PlacementOptions.html) for the payload contract.
