<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" height="120" srcset="https://github.com/user-attachments/assets/6ad669a1-d6c4-4a1d-b4a1-26e0288c1797">
    <source media="(prefers-color-scheme: light)" height="120" srcset="https://github.com/user-attachments/assets/1e5b0ca9-1e9a-4a91-8896-d49287365ec7">
    <img alt="uncurses" height="120" src="https://github.com/user-attachments/assets/1e5b0ca9-1e9a-4a91-8896-d49287365ec7">
  </picture>
</p>

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
