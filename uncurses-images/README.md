# uncurses-images

Native and cell-based images for `uncurses`.

The crate prepares Kitty native, iTerm2, and Sixel protocol bytes.
It also prepares Kitty placeholder cells and Chafa-style symbols.
The symbol implementation uses the existing Rust port.

## Choose an output path

Use `ImageRenderer` to select a protocol and cache encoded image data.
Create it with `ImageRenderer::from_program` from terminal reports and environment,
or supply `ImageCapabilities` to `ImageRenderer::new`.
Selection prefers Kitty placeholders, Kitty, iTerm2, Sixel, then symbols.
It checks the screen's color profile, image dimensions, and available pixel geometry.

Automatic detection uses the reported terminal name, then nonempty `TERM_PROGRAM`, then exact `TERM` values.
Kitty, Rio, and Ghostty identities imply Kitty graphics support. A Kitty graphics reply also confirms support.
Rio and Ghostty select placeholders. Kitty selects placeholders after a version report confirms 0.28 or later.

The `TERM` fallbacks are `xterm-kitty`, `xterm-ghostty`, `xterm-rio`, `rio`, and `wezterm`.
`xterm-kitty` selects native Kitty until a version report confirms placeholder support.
`xterm-ghostty`, `xterm-rio`, and `rio` select placeholders. `wezterm` selects iTerm2.
Generic values such as `xterm-256color` are inconclusive.

Automatic iTerm2 detection recognizes iTerm2, Rio, WezTerm, mintty, and Konsole.
For optional image support, such as VS Code or xterm.js add-ons, set `ImageCapabilities::iterm2` after confirming support.
Run `images_auto` inside a supported terminal to see the selected protocol.

Inside tmux, `ImageRenderer::from_program` permits placeholders and symbols.
It wraps each Kitty upload chunk, virtual placement, and deletion command with tmux passthrough.
Placeholder cells stay in the pane's text grid.
Enable `allow-passthrough` in tmux. The library leaves tmux configuration unchanged.
Forward terminal identity and graphics queries to the outer terminal, as `images_auto` demonstrates.
Native Kitty, iTerm2, and Sixel remain disabled during automatic selection inside multiplexers.

`ImageRenderer::new` uses direct output. GNU Screen (`STY`) also permits placeholders and symbols, with resource forwarding supplied by the caller.
Use the raw `CellImage::upload` and `delete` bytes for manual forwarding.
Other transports require explicit resource forwarding. Nested multiplexers require a transport for each layer.

Call `draw(screen, id, area, image, options)` to stage an image.
Finish the frame with `screen.render()`.
The screen owns placements, uploads resources, and releases them after removal.
Call `remove(screen, id)` to stage removal.
Keep the renderer paired with one screen and terminal session.
After capability, geometry, or screen policy changes, update its configuration and draw the images again.

```rust
use uncurses::{layout::Rect, screen::Screen};
use uncurses_images::{Image, ImageCapabilities, ImageOptions, ImageRenderer};

fn main() -> std::io::Result<()> {
    let image = Image::from_rgb(1, 1, vec![255, 0, 0])?;
    let mut screen = Screen::new(Vec::new(), (20, 6));
    let mut images = ImageRenderer::new(ImageCapabilities::default());
    images.draw(&mut screen, 1, Rect::new(2, 1, 8, 3), &image, &ImageOptions::default())?;
    screen.render()?;
    images.remove(&mut screen, 1);
    screen.render()
}
```

Unknown capabilities select symbols. The automatic renderer preserves options
and shares prepared data across movement and equivalent copies. Repaint the background after movement or removal.
For cell images, removal replaces matching cells with default-style blanks and preserves changed overlay cells.
The screen emits one coordinate anchor per visible placeholder run.
It adds new anchors after overlays divide a row.
Prepared cells retain their coordinates for independent copies.
After an output failure, restore the terminal parser and cursor.
Then call `screen.invalidate()` before another render.

## Prepare output explicitly

Use `Image::prepare` for a `PreparedImage`.
Pass `placement()` to `Screen::set_placement`, then call `Screen::render`.
The placement includes its shared resource handle and a bounded encoder.
Supply the terminal's actual cell dimensions in pixels.

Use `Image::symbols` for ordinary cells. The method uses the screen's
color profile, width mode, and East Asian width policy. Draw the returned buffer
with `Surface::draw`. Use `symbols_with_policy` for an explicit policy.
`SymbolSet::contains(character, screen)` and `is_empty(screen)` use the same screen policy.
Their `_with_policy` methods accept an explicit width mode and ambiguous-width setting.

Use `Image::kitty_placeholders` for ordinary placeholder cells.
Pass `placement(screen)?` to `Screen::set_placement`.
The method checks the screen's true-color profile and shares cells and resources.
Render the screen to upload and display the image.

Managed native images follow ordinary cell ownership.
Later cell writes cover the image cells they replace.
The screen resolves visible regions, and the image encoder prepares their crops before frame output.
The image example uses this same API for Kitty, Sixel, and iTerm2.
Sixel and iTerm2 native visual behavior still requires confirmation in Rio and iTerm2.
The application redraws the underlying content after movement or when a dialog closes.

Use `raw_placement(id)?` for explicit protocol placement IDs and native layer behavior.
Raw Kitty placements use z-index -1, below text foregrounds and above cell backgrounds.
Use distinct protocol placement IDs for simultaneous raw copies.

## Preparation and lifetime

Reuse prepared values across frames. Clones share source pixels and encoded data.
Prepare images again when cell pixel dimensions change.
Remove placements with `Screen::remove_placement`, then render the frame.
The screen releases managed resources after their final placement disappears.
Dropping prepared data performs no terminal I/O.

`Screen::commit_rows` releases inline output while preserving its visible images.
Resources used by committed rows stay in the terminal after program teardown.
Cell-backed images can cross the commit boundary; their retained rows keep the same placement ID.
Commit whole opaque images and sized-text blocks. A boundary inside an opaque block returns an error before output.
Application IDs can be reused after a commit. Managed native output allocates fresh terminal instances.
The screen keeps compact identity records for committed resources and releases unused upload buffers.
For raw native placements, use distinct protocol placement IDs for copies that remain visible.
Terminal reset or eviction can discard committed resources.

For manual protocol output, use the raw `upload()` and `delete()` bytes.
Upload before drawing. Clear all visible copies before deleting the resource.
`CellImage::cells()` supports independent copies through `Surface::draw`.
Manual copies require true color and caller-managed resource lifetime.
Managed cell ownership covers each registered rectangle.

The loader uses the enabled `image` formats and applies EXIF orientation.
Rustdoc lists the input and allocation limits.
Cover and zoom resize the selected source crop before clipping it to the destination.
The canvas and resized image each accept at most 4,194,304 pixels and 16,384 pixels per axis.
Preparation rejects requests that exceed these limits or the shared buffer budget.

Sixel and iTerm2 need one spare row below the image. Sixel also requires
cursor-relative placement and graphics erasure through character operations.
Select a backend that the terminal supports.

## Examples

```sh
cargo run -p examples --example images_auto
cargo run -p examples --example images_auto -- --dump
cargo run -p examples --example images_auto -- --dump-placeholders
cargo run -p examples --example images_auto -- --dump-committed
cargo run -p examples --example images_auto -- --dump-shared
cargo run -p examples --example images_drag -- --symbols
cargo run -p examples --example images_drag -- --kitty
cargo run -p examples --example images_drag -- --kitty --overlay
cargo run -p examples --example images_drag -- --placeholders --inline
cargo run -p examples --example images_drag -- --iterm2 --overlay
cargo run -p examples --example images_drag -- --sixel --overlay
```

`images_auto` queries terminal support and cell dimensions, then selects a protocol.
Space moves or removes its image. Its dump exercises all five output paths.
The screen manages uploads and deletion.
Use `--dump-placeholders` to inspect compressed output across overlays, movement, resize, and cleanup.
Use `--dump-committed` to inspect permanent images, split placeholder rows, and reused application IDs.
Use `--dump-shared` to inspect shared uploads, image order, dialog cutouts, and resource replacement.

Supply an image path to use PNG or JPEG input. Add `--dump` for named frames
with hexadecimal output. Visible native output uses reported pixel geometry.
Use `--cell-pixels 8x16` only when those dimensions match your terminal.

The image example accepts these keys while it runs:

| Key | Action |
| --- | --- |
| Tab or `p` | Cycle symbols, Kitty, placeholders, iTerm2, and Sixel |
| Shift+Tab | Cycle protocols in reverse |
| `s` | Cycle symbol sets |
| `f` | Cycle fit modes |
| `r` | Cycle resampling filters |
| `d` | Cycle dithering methods, including none |
| `k` | Cycle Wu and K-means quantization |
| `c` | Cycle palette sizes from 256 to 2 colors |
| Space | Toggle the dialog |
| Shift+arrows | Move the dialog |

The header shows the protocol and the last selected option. Selections persist
across protocol changes. Drag the image or use arrow keys to move it.
Press `q` to quit.

Dithering affects Sixel and limited-color symbols. Quantization and palette
size affect Sixel. Select protocols that your terminal supports.
Missing pixel geometry or a restricted color profile produces a visible
message. You can select another protocol while that message remains visible.

Use `--dump-keys p,p,p,p,p,s,f,r,d,k,c` to capture scripted transitions.
The dump includes an unchanged frame after each key and a final cleanup frame.
Use `--dump-dialog` to capture dialog movement, origin coverage, and restoration.

The crate requires Rust 1.90. PNG decoding is always available. The `jpeg`
feature is enabled by default. The `gif` and `webp` features enable those formats.
