# uncurses

<img width="480" height="294" alt="output" src="https://github.com/user-attachments/assets/3e9d7066-f435-40aa-9000-fe80185e6966" />

A low-level terminal library for Rust. The name winks at the venerable
`curses` and `ncurses`, then quietly walks away from their baggage: no
terminfo database, no compatibility matrix for terminals that stopped
shipping decades ago. uncurses assumes a modern, VT100/xterm-compatible
terminal and talks to it straight.

It hands you the pieces to build a terminal UI and then steps out of the way.
You own the event loop. You decide when bytes hit the wire. No widget tree, no
hidden global state, no framework to wrestle.

## Docs

Everything lives on the website: guides, concepts, examples, and the full API
reference.

### [uncurses-website.pages.dev](https://uncurses-website.pages.dev/)

## Crates

| Crate | What it is |
| --- | --- |
| [`uncurses`](uncurses/) | The core library: screen, renderer, input, ANSI, terminal. |
| [`uncurses-ratatui`](uncurses-ratatui/) | A [ratatui](https://docs.rs/ratatui) `Backend` built on the `Screen` facade. |

## Install

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
```

Runs on Linux, macOS, Windows, and the BSDs; tracks the latest stable Rust on
the 2024 edition.

## License

MIT. See [LICENSE](LICENSE).
