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
  <a href="https://www.rust-lang.org"><img alt="Rust 1.88+" src="https://img.shields.io/badge/rust-1.88%2B-orange.svg"></a>
  <a href="https://uncurses.org"><img alt="Website" src="https://img.shields.io/badge/website-uncurses.org-blue.svg"></a>
</p>

uncurses is a Rust library for building terminal user interfaces. It provides a
direct, framework-free way to draw to the terminal and read input, giving you
control over every cell and your own event loop, whether you run inline, take
over the full screen, mix the two, or leave the console unmanaged and just
shape your output.

It includes a diffing renderer that redraws only what changed, Unicode-aware
width, truecolor styling with automatic downsampling, hyperlinks, and typed
keyboard, mouse, and paste input. It asks the terminal what it supports
instead of looking it up in a terminfo database, so the same code runs on
Linux, macOS, and Windows.

Full guides, concepts, and API reference: [uncurses.org](https://uncurses.org)

## Features

Nothing is read from terminfo. `Program` asks the terminal what it supports,
reads the reply, and uses only what is actually there.

- Capability discovery by querying the terminal
- Diffing renderer: only changed cells are written
- Inline or [alternate screen](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html), with suspend and resume
- Truecolor that degrades to [256-color, ANSI, or mono](https://github.com/termstandard/colors)
- [Styled underlines](https://sw.kovidgoyal.net/kitty/underlines/): curly, dotted, dashed, double
- [Clickable hyperlinks](https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda) (OSC 8)
- [Unicode core](https://contour-terminal.org/vt-extensions/unicode-core/) (Mode 2027): width by grapheme cluster
- Unambiguous keys, with release: [kitty keyboard](https://sw.kovidgoyal.net/kitty/keyboard-protocol/) or [`modifyOtherKeys`](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Alt-and-Meta-Keys)
- [Mouse](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-Mouse-Tracking) clicks, motion, wheel, and [pixel positions](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Extended-coordinates)
- [Pointer shapes](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Operating-System-Commands) (OSC 22)
- [Bracketed paste](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Functions-using-CSI-_-ordered-by-the-final-character_s_) (Mode 2004)
- [Focus events](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Functions-using-CSI-_-ordered-by-the-final-character_s_) (Mode 1004)
- [Visibility reports](https://rockorager.dev/misc/visibility-reports/) (Mode 2033): idle when off screen
- [In-band resize](https://rockorager.dev/misc/in-band-resize-notifications/) (Mode 2048): no signal handler
- [Synchronized output](https://gist.github.com/christianparpart/d8a62cc1ab659194337d73e399004036) (Mode 2026): frames without tearing
- Terminal colors: read, set, reset (OSC 4/10/11/12)
- [Color scheme updates](https://contour-terminal.org/vt-extensions/color-palette-update-notifications/) (Mode 2031): follow light and dark
- [Clipboard](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Operating-System-Commands) (OSC 52), system and primary
- Window title and icon name (OSC 0/1/2)
- [Taskbar progress](https://conemu.github.io/en/AnsiEscapeCodes.html#ConEmu_specific_OSC) (OSC 9;4)
- Typed key, mouse, paste, focus, and resize events

## Quickstart

`Program` owns the session: raw mode, terminal modes, input, and teardown.
`Screen` is the renderer it draws with: a cell grid that ships only the cells
that changed. Reach for the screen through `screen_mut()`.

```rust,no_run
use uncurses::event::Event;
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    program.init()?;
    program.enter_alt_screen()?; // take over the full screen

    let screen = program.screen_mut();
    screen.set_str((0, 0), "hello, uncurses!", Style::new());
    screen.render()?;

    while !matches!(program.read_event()?, Event::KeyPress(_)) {} // wait for a key
    program.finish() // restores the main screen
}
```

Only need output? A `Screen` stands alone over any `Write`, with no terminal
session at all:

```rust
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;

let mut screen = Screen::new(Vec::new(), (20, 1));
screen.set_str((0, 0), "hello, uncurses!", Style::new());
screen.render().unwrap();
assert!(!screen.writer().is_empty());
```

Per-crate guides and more examples:

- [`uncurses`](uncurses/): program, screen, rendering, input, and the event loop.
- [`uncurses-ratatui`](uncurses-ratatui/): drive ratatui widgets through uncurses.

### Examples

A curated few, each `cargo run -p examples --example <name>`:

**`tour`**

An animated showcase that cycles through sprinkles, nested colored panels, a
styled banner, a marquee, and bouncing balls. The quickest way to see what the
renderer can do.

<p>
  <picture>
    <img width="440" alt="tour" src="https://github.com/user-attachments/assets/57a39054-306b-44a0-8851-988b084c1f6f" />
  </picture>
</p>

**`gradient`**

A smooth truecolor field that packs two colors per cell with half-block
sub-pixels, plus a mouse-driven inspector that reports the color under the
pointer. Shows color handling and mouse input together.

<p>
  <picture>
    <img width="440" alt="gradient" src="https://github.com/user-attachments/assets/65f2ce3c-39a4-419b-9337-43b800814690" />
  </picture>
</p>

**`file_explorer`**

A real two-pane file browser with a live preview that scrolls by column, so a
side pane moves without disturbing the file list. A full end-to-end app.


<p>
  <picture>
    <img width="440" alt="file_explorer" src="https://github.com/user-attachments/assets/65f3a526-c74e-4f48-9e1d-3da6112c75dc" />
  </picture>
</p>

**`task_picker`**

An inline picker (no alternate screen) that hands off to an animated progress
bar, then exits cleanly. Shows how uncurses draws in the normal buffer.

<p>
  <picture>
    <img width="440" alt="task_picker" src="https://github.com/user-attachments/assets/7e822d76-36f9-40da-933b-698ae3bbfd8f" />
  </picture>
</p>

For more, browse the [`examples/`](examples/examples) directory.

## Crates

- [`uncurses`](uncurses/): the core library: screen, renderer, input, ANSI, terminal.
- [`uncurses-ratatui`](uncurses-ratatui/): a [ratatui](https://ratatui.rs) `Backend` built on the `Program` facade.

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
