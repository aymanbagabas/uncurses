# uncurses

<img width="480" height="294" alt="output" src="https://github.com/user-attachments/assets/3e9d7066-f435-40aa-9000-fe80185e6966" />


A low-level terminal library for Rust. The name winks at the venerable
`curses` and `ncurses`, then quietly walks away from their baggage: no
terminfo database, no compatibility matrix for terminals that haven't
shipped in decades. uncurses assumes a modern, VT100/xterm-compatible
terminal and talks to it straight.

It hands you the pieces to build a terminal UI and then steps out of the
way. You own the event loop. You decide when bytes hit the wire. No widget
tree, no hidden global state, no framework to wrestle.

## What you get

- A cell-based `Screen` that diffs frames and ships only the bytes that
  actually changed.
- An input parser that turns raw terminal bytes into typed `Event`
  values: keys, mouse, paste, focus, resize, and query replies.
- ANSI escape helpers for styles, colors, the cursor, and the long tail
  of terminal modes: alt screen, bracketed paste, mouse, kitty keyboard,
  and friends.
- A `Terminal` handle for raw mode and window size.
- Typed terminal queries (background color, device attributes, cell size,
  and the rest), issued through the event source or async stream and
  answered without ever swallowing the user's keystrokes.
- Optional async input over a `futures_core::Stream`, behind a feature
  flag, with no runtime baked in.

Drawing is infallible and cheap. Every draw call writes into an in-memory
buffer and hands back nothing to check. The one place I/O can fail is
`flush`, when that buffer goes to the terminal. The hot path stays simple
and the error handling stays honest.

## A taste

```rust
use uncurses::terminal::Terminal;
use uncurses::color::BasicColor;
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::WrapMode;

fn main() -> std::io::Result<()> {
    let term = Terminal::stdio();
    let mut screen = Screen::new(term.output(), term.window_size().unwrap_or_default());

    let style = Style::default().bold().fg(BasicColor::Green.into());
    screen.set_str_with((0, 0), "Hello, terminal!", WrapMode::Truncate, style);

    // render() stages the frame diff, flush() commits it. present() does both.
    screen.present()
}
```

Want the whole loop: raw mode, an alternate screen, keyboard and mouse,
and a teardown that always cleans up after itself? The
[tutorial](docs/tutorial.md) builds a small interactive app from scratch
in a few minutes. New to terminals in general? [How terminals actually
work](docs/terminals.md) is the five-minute mental model behind all of it.

## Terminal features

uncurses runs on all the major platforms: Linux, macOS, Windows, and the
BSDs. It does not use terminfo. If you want to know what a terminal can do
before leaning on it, ask the terminal yourself with a
[query](uncurses/README.md#queries).

- 24-bit RGB color, with automatic downsampling to 256, 16, or no color
- [Hyperlinks](https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda) (OSC 8)
- [Fancy underlines](https://sw.kovidgoyal.net/kitty/underlines/): curly, double, dotted, dashed, plus underline color
- Bracketed paste
- [Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/), all five enhancement flags
- modifyOtherKeys
- Mouse: X10, SGR (1006), SGR-pixel (1016), URxvt (1015), and UTF-8 (1005), with press, release, drag, and wheel
- Focus events (Mode 1004)
- Synchronized output (Mode 2026)
- [Unicode core](https://github.com/contour-terminal/terminal-unicode-core) (Mode 2027)
- Color scheme updates (Mode 2031)
- [In-band resize reports](https://gist.github.com/rockorager/e695fb2924d36b2bcf1fff4a3704bd83) (Mode 2048)
- System clipboard (OSC 52)
- System notifications (OSC 9, Kitty OSC 99, URxvt OSC 777)
- Cursor shapes and visibility, window title (OSC 2)
- Wide characters and grapheme clusters
- Alt screen and inline rendering, both diffed cell by cell
- 7-bit and 8-bit (C1) control sequences
- Typed terminal queries: background color, device attributes, cell and pixel size, cursor position, and more
- Optional async input over a `futures_core::Stream`

Planned: real image rendering, currently only low-level encoders exist.
The goal is to place images like text across Unicode blocks (half blocks
and quadrants), Sixel, iTerm2, and the
[Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
(classic, Unicode placeholder, and
[text-sized](https://github.com/kovidgoyal/kitty/blob/master/docs/text-sizing-protocol.rst)
placements).

## Crates

| Crate | What it is |
| --- | --- |
| [`uncurses`](uncurses/README.md) | The core library: screen, renderer, input, ANSI, terminal. |
| [`uncurses-ratatui`](uncurses-ratatui/README.md) | A ratatui `Backend` built on `uncurses::screen::Screen`. |

Each crate carries its own README with the details. The `examples/`
directory is stuffed with runnable demos, from a click counter to a
two-pane file explorer. Pick one and run it:

```sh
cargo run --example counter
cargo run --example file_explorer
```

## Install

uncurses tracks the latest stable Rust on the 2024 edition.

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
```

### Features

- `unicode-rs` (default): pure-Rust width and segmentation tables. Small
  and fast.
- `icu`: ICU4X-backed segmentation and properties. Larger build, more
  correct on emoji and grapheme edge cases. Takes precedence when both
  are on.
- `async`: adds `EventStream`, a runtime-agnostic `futures_core::Stream`
  of events. Pulls in only `futures-core`.

## License

MIT. See [LICENSE](LICENSE).
