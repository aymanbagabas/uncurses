<br>
<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" width="320" srcset="https://github.com/user-attachments/assets/6ad669a1-d6c4-4a1d-b4a1-26e0288c1797">
    <source media="(prefers-color-scheme: light)" width="320" srcset="https://github.com/user-attachments/assets/1e5b0ca9-1e9a-4a91-8896-d49287365ec7">
    <img alt="uncurses" width="320" src="https://github.com/user-attachments/assets/1e5b0ca9-1e9a-4a91-8896-d49287365ec7">
  </picture>
</p>

<p align="center">
  <a href="https://github.com/aymanbagabas/uncurses/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/aymanbagabas/uncurses/actions/workflows/ci.yml/badge.svg"></a>
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="Rust 1.88+" src="https://img.shields.io/badge/rust-1.88%2B-orange.svg">
  <a href="https://uncurses.org"><img alt="Website" src="https://img.shields.io/badge/website-uncurses.org-blue.svg"></a>
</p>

uncurses is a Rust library for building terminal user interfaces. It provides a
direct, framework-free way to draw to the terminal and read input, giving you
control over every cell and your own event loop, whether you run inline, take
over the full screen, mix the two, or leave the console unmanaged and just
shape your output.

It includes a diffing renderer that redraws only what changed, Unicode-aware
width, truecolor styling with automatic downsampling, hyperlinks, and typed
keyboard, mouse, and paste input. Rather than a terminfo database, it detects
capabilities through terminal queries and sets sane defaults, and runs across
Linux, macOS, and Windows.

Full guides, concepts, and API reference: [uncurses.org](https://uncurses.org)

## Quickstart

`Screen` is the core: it owns raw mode, a diffed back buffer, and input. Draw
into it, call `render()` to ship only the changed cells, and `finish()` to
restore the terminal.

```rust,no_run
use uncurses::event::Event;
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init()?;
    screen.enter_alt_screen()?; // take over the full screen
    screen.set_str((0, 0), "hello, uncurses!", Style::new());
    screen.render()?;
    while !matches!(screen.read_event()?, Event::KeyPress(_)) {} // wait for a key
    screen.finish() // restores the main screen
}
```

Per-crate guides and more examples:

- [`uncurses`](uncurses/): screen, rendering, input, and the event loop.
- [`uncurses-ratatui`](uncurses-ratatui/): drive ratatui widgets through uncurses.

### Examples

A curated few, each `cargo run -p examples --example <name>`:

**`tour`**

An animated showcase that cycles through sprinkles, nested colored panels, a
styled banner, a marquee, and bouncing balls. The quickest way to see what the
renderer can do.

<a href="examples/examples/tour.rs"><img src="assets/tour.gif" width="440" alt="tour example"></a>

**`gradient`**

A smooth truecolor field that packs two colors per cell with half-block
sub-pixels, plus a mouse-driven inspector that reports the color under the
pointer. Shows color handling and mouse input together.

<a href="examples/examples/gradient.rs"><img src="assets/gradient.gif" width="440" alt="gradient example"></a>

**`file_explorer`**

A real two-pane file browser with a live preview that scrolls by column, so a
side pane moves without disturbing the file list. A full end-to-end app.

<a href="examples/examples/file_explorer.rs"><img src="assets/file_explorer.gif" width="440" alt="file explorer example"></a>

**`task_picker`**

An inline picker (no alternate screen) that hands off to an animated progress
bar, then exits cleanly. Shows how uncurses draws in the normal buffer.

<a href="examples/examples/task_picker.rs"><img src="assets/task_picker.gif" width="440" alt="task picker example"></a>

For more, browse the [`examples/`](examples/examples) directory.

## Crates

- [`uncurses`](uncurses/): the core library: screen, renderer, input, ANSI, terminal.
- [`uncurses-ratatui`](uncurses-ratatui/): a [ratatui](https://ratatui.rs) `Backend` built on the `Screen` facade.

## Install

Not on crates.io yet, so depend on it straight from git:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
```

Runs on Linux, macOS, Windows, and the BSDs. Uses the 2024 edition and tracks
the latest stable Rust (currently 1.88 or newer).

## Credits

uncurses stands on the shoulders of projects worth naming:

- [ncurses](https://invisible-island.net/ncurses/): the original. The name
  nods to it, even as uncurses leaves terminfo behind.
- [ultraviolet](https://github.com/charmbracelet/ultraviolet): Charm's
  low-level terminal library, and the inspiration for how uncurses models cells
  and screens.
- [colorprofile](https://github.com/charmbracelet/colorprofile): Charm's color
  degradation library, the model behind uncurses color profiles.
- [ratatui](https://ratatui.rs): the Rust TUI framework uncurses ships a
  backend for.

## License

MIT. See [LICENSE](LICENSE).
