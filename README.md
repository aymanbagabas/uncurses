# uncurses

<img width="480" height="294" alt="output" src="https://github.com/user-attachments/assets/3e9d7066-f435-40aa-9000-fe80185e6966" />

A low-level terminal library for Rust. The name winks at the venerable
`curses` and `ncurses`, then quietly walks away from their baggage: no
terminfo database, no compatibility matrix for terminals that stopped
shipping decades ago. uncurses assumes a modern, VT100/xterm-compatible
terminal and talks to it straight.

It hands you the pieces to build a terminal UI and then steps out of the
way. You own the event loop. You decide when bytes hit the wire. No widget
tree, no hidden global state, no framework to wrestle.

## A taste

A `Style` is a value you build and hand to a writer. Its `Display` adapters
render the escape sequences for you, so styled output is just `write!` —
no raw mode, no setup, no teardown:

```rust
use std::io::{self, Write};
use uncurses::color::{BasicColor, Color};
use uncurses::style::Style;

fn main() -> io::Result<()> {
    let mut out = io::stdout().lock();
    let title = Style::new().bold().fg(BasicColor::Green);
    let link = Style::new()
        .underline()
        .fg(Color::hex("#78aaff").unwrap())
        .link("https://github.com/aymanbagabas/uncurses", "");

    writeln!(out, "{}", title.styled("uncurses"))?;
    writeln!(out, "a terminal library that {}", link.styled("gets out of the way"))?;
    out.flush()
}
```

That is the styling layer on its own. When you want an interactive app — an
event loop, mouse, and teardown that always cleans up — reach for `Screen`.
A session is bracketed by `init()` and `finish()`, and in between it starts
**inline** (drawing alongside your shell output, not taking over the window)
with the **cursor visible**; the alternate screen and a hidden cursor are
opt-in:

```rust,no_run
use uncurses::buffer::Bounded;
use uncurses::event::{Event, Key};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init()?; // raw mode + capability detection; starts inline, cursor visible
    let w = screen.width();
    screen.resize((w, 1)); // inline: one row tall

    screen.set_str((0, 0), "Hello! Press q to quit.", Style::new());
    screen.present()?;

    let q: Key = "q".parse().unwrap();
    while !matches!(screen.read_event()?, Event::KeyPress(k) if k == q) {}

    screen.finish() // restore the terminal — always, one call
}
```

For a full-screen app, add `screen.enter_alt_screen()?` after `init`. The
[`uncurses` quick start](uncurses/README.md#screen-the-easy-button) builds
one in a dozen lines, or run `cargo run --example counter`.

## Pick your layer

| You want to...                                                               | Reach for                                                                                                         |
| ---------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| Ship an interactive app fast                                                 | `Screen` — owns the terminal, renderer, and input; handles raw mode, capability detection, defaults, and teardown |
| Render cells to anything that's `Write` (a `Vec`, a socket, a snapshot test) | `Canvas` — the cell grid and diffing renderer, no input or lifecycle opinion                                      |
| Turn raw bytes into typed events                                             | `EventSource` — keys, mouse, paste, focus, resize, query replies                                                  |
| Enter raw mode, ask the window size                                          | `Terminal`                                                                                                        |
| Emit a specific escape sequence                                              | the `ansi` module                                                                                                 |

`Screen` is `Canvas` + `EventSource` + `Terminal` with the lifecycle
wired up. When you outgrow the defaults, drop a layer; nothing is hidden
from you.

Drawing is infallible and cheap: every draw writes into an in-memory buffer
and returns nothing to check. The one place I/O can fail is `flush`, when
that buffer goes to the terminal. The hot path stays simple and the error
handling stays honest.

## What you get

- A self-managing **`Screen`** that owns the terminal, a drawing surface,
  and the input source, and handles raw mode, capability detection, sane
  defaults, and teardown.
- A cell-based **`Canvas`** that diffs frames and ships only the bytes that
  changed — over a terminal, or over any other `Write` sink.
- An **input decoder** that turns raw bytes into typed `Event` values:
  keys, mouse, paste, focus, resize, and query replies.
- **ANSI helpers** for styles, colors, the cursor, and the long tail of
  terminal modes: alt screen, bracketed paste, mouse, kitty keyboard, and
  friends.
- A **`Terminal`** handle for raw mode and window size.
- **Typed queries** — background color, device attributes, cell size, and
  the rest — answered through the event stream without ever swallowing a
  keystroke.
- Optional **async input** over a `futures_core::Stream`, behind a feature
  flag, with no runtime baked in.
- A **ratatui backend** so you can bring ratatui widgets and let uncurses
  do the rendering.

## Terminal features

uncurses runs on Linux, macOS, Windows, and the BSDs. It does not use
terminfo, and it does not guess: if you want to know what a terminal can do
before leaning on it, ask the terminal yourself with a
[query](uncurses/README.md#asking-the-terminal-what-it-can-do).

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
- Typed queries: background color, device attributes, cell and pixel size, cursor position, and more
- Optional async input over a `futures_core::Stream`

Planned: real image rendering. Today only the low-level encoders exist; the
goal is to place images like text across Unicode blocks (half blocks and
quadrants), Sixel, iTerm2, and the
[Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/).

## Crates

| Crate                                            | What it is                                                 |
| ------------------------------------------------ | ---------------------------------------------------------- |
| [`uncurses`](uncurses/README.md)                 | The core library: screen, renderer, input, ANSI, terminal. |
| [`uncurses-ratatui`](uncurses-ratatui/README.md) | A ratatui `Backend` built on the `Screen` facade.          |

## Examples

The [`examples/`](examples/README.md) directory is full of runnable demos,
grouped by use case. A few to orient yourself:

```sh
cargo run --example input_only    # read input only: decode and print events
cargo run --example draw_only      # draw only: animate the screen, no input
cargo run --example offscreen      # render to a Vec<u8>, no terminal at all
cargo run --example counter        # the full mix: render + keys + mouse
cargo run --example file_explorer  # a two-pane explorer (async input)
cargo run --example ratatui_hello  # ratatui widgets via the uncurses backend
```

New to building TUIs? The [tutorial](docs/tutorial.md) builds a small
interactive app from scratch. New to terminals in general? [How terminals
actually work](docs/terminals.md) is the five-minute mental model behind
all of it.

## Install

uncurses tracks the latest stable Rust on the 2024 edition.

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
```

### Features

- `unicode-rs` _(default)_: pure-Rust width and segmentation tables. Small
  and fast.
- `icu`: ICU4X-backed segmentation and properties. Larger build, more
  correct on emoji and grapheme edge cases. Takes precedence when both are
  on.
- `async`: adds `EventStream`, a runtime-agnostic `futures_core::Stream` of
  events. Pulls in only `futures-core`.

## License

MIT. See [LICENSE](LICENSE).
