---
title: "Installation"
weight: 1
---

uncurses is pure Rust with a tiny dependency footprint. Add it, pick a width
backend if the default does not suit you, and you are ready to draw.

## Add the dependency

uncurses is not on crates.io yet, so depend on it straight from git:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
```

Using the [ratatui](https://docs.rs/ratatui) backend instead? Add both:

```toml
[dependencies]
uncurses-ratatui = { git = "https://github.com/aymanbagabas/uncurses" }
ratatui = "0.30"
```

## Feature flags

Every app needs a width backend, since terminal layout is measured in cells and
some characters are two cells wide. uncurses ships two, and you pick one.

| Feature | Default | What it does |
| --- | :---: | --- |
| `unicode-rs` | yes | Pure-Rust width and segmentation tables. Small build, conservative on emoji and zero-width-joiner edge cases. |
| `icu` | no | ICU4X-backed segmentation and Unicode properties. Strictly more correct on the tricky clusters, at the cost of a larger binary. Wins when both are on. |
| `async` | no | Adds `EventStream`, a runtime-agnostic [`futures_core::Stream`](https://docs.rs/futures-core) of decoded events. Pulls in only `futures-core`, no executor. |

The default is `unicode-rs`. To trade size for correctness, switch to `icu`:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses", default-features = false, features = ["icu"] }
```

To await events instead of blocking a thread, add `async`:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses", features = ["async"] }
```

## Supported platforms

uncurses runs on Linux, macOS, Windows, the BSDs, and other Unix-like systems.
It does not use terminfo and assumes
a modern, VT100/xterm-compatible terminal, so there is no platform database to
install and no per-terminal configuration to manage.

## Rust version

uncurses tracks the latest stable Rust on the 2024 edition. There is no separate
toolchain or build step: a normal `cargo build` is all it takes.

## Next steps

With the crate added, the next page writes the smallest complete program:
[Hello, terminal]({{< relref "hello-world.md" >}}).
