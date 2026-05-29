# uncurses

A Rust terminal rendering library.

## Crates

- `uncurses`: core library (screen, renderer, input, ANSI helpers).
- `uncurses-ratatui`: ratatui `Backend` adapter over `uncurses::screen::Screen`.
- `examples`: runnable examples, including ratatui ports.

## Build

```sh
cargo build
cargo test
```

## License

MIT
