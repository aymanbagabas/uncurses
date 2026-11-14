---
title: "Installation"
weight: 1
---

uncurses is pure Rust with a tiny dependency footprint. Add it, keep the
default width backend or pick another one, and you are ready to draw.

## Add the dependency

Published releases of [uncurses](https://crates.io/crates/uncurses) and
[uncurses-ratatui](https://crates.io/crates/uncurses-ratatui) are available on
crates.io. Select a released version with its matching
[docs.rs reference](https://docs.rs/uncurses).

This website follows the current repository source. For its examples and
features, use a git dependency:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
```

Using the [ratatui](https://docs.rs/ratatui) backend too? Add the core crate,
the backend crate, and ratatui:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
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

The default is `unicode-rs`. If you turn default features off, choose
`unicode-rs` or `icu`; building without a width backend fails. To trade size
for correctness, switch to `icu`:

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

CI covers Linux (glibc and musl), macOS, Windows, FreeBSD, OpenBSD, NetBSD,
illumos, and Solaris. uncurses does not use terminfo and assumes a modern,
VT100/xterm-compatible terminal, so there is no platform database to install
and no per-terminal configuration to manage.

## Rust version

uncurses uses the 2024 edition, currently needs Rust 1.88 or newer, and tracks
the latest stable toolchain. There is no separate toolchain or build step: a
normal `cargo build` is all it takes.

## AI assistants

The repository includes an
[uncurses skill](https://github.com/aymanbagabas/uncurses/blob/main/skills/README.md)
for assistants that help build applications. It explains the ownership model,
common API mistakes, installation, and revision compatibility.

## Next steps

With the crate added, the next page writes the smallest complete interactive
program: [Hello, terminal]({{< relref "hello-world.md" >}}).
