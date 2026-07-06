# uncurses

<img width="480" height="294" alt="uncurses" src="https://github.com/user-attachments/assets/3e9d7066-f435-40aa-9000-fe80185e6966" />

A terminal toolkit library for Rust. It hands you the building blocks for a
terminal UI and then gets out of the way: you own the event loop, you decide
when bytes hit the wire, and there is no widget tree or hidden global state to
fight.

The name winks at `curses` and `ncurses`, then walks away from their baggage.
No terminfo database, no compatibility matrix for terminals that stopped
shipping decades ago. uncurses assumes a modern, VT100/xterm-compatible
terminal and talks to it straight.

## Docs

Guides, concepts, examples, and the full API reference live on the website.

### [uncurses.org](https://uncurses.org)

## Crates

| Crate | What it is |
| --- | --- |
| [`uncurses`](uncurses/) | The core library: screen, renderer, input, ANSI, terminal. |
| [`uncurses-ratatui`](uncurses-ratatui/) | A [ratatui](https://ratatui.rs) `Backend` built on the `Screen` facade. |

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
  winks at it, even as uncurses leaves terminfo behind.
- [ultraviolet](https://github.com/charmbracelet/ultraviolet): Charm's
  low-level terminal library, and the inspiration for how uncurses models cells
  and screens.
- [colorprofile](https://github.com/charmbracelet/colorprofile): Charm's color
  degradation library, the model behind uncurses color profiles.
- [ratatui](https://ratatui.rs): the Rust TUI framework uncurses ships a
  backend for.

## License

MIT. See [LICENSE](LICENSE).
