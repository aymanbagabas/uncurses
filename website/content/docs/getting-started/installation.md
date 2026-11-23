---
title: "Installation"
weight: 1
---

uncurses is pure Rust with a tiny dependency footprint. Add it, keep the
default width backend or pick another one, and you are ready to draw.

## Add the dependency

Install v0.0.8 of [uncurses](https://crates.io/crates/uncurses) and
[uncurses-ratatui](https://crates.io/crates/uncurses-ratatui) from crates.io.
Use the matching API reference:
[uncurses v0.0.8](https://docs.rs/uncurses/0.0.8) or
[uncurses-ratatui v0.0.8](https://docs.rs/uncurses-ratatui/0.0.8).

The dependency examples on this page target v0.0.8:

```toml
[dependencies]
uncurses = "0.0.8"
```

Using the [Ratatui](https://docs.rs/ratatui) backend too? Add the core crate,
the backend crate, and Ratatui:

```toml
[dependencies]
uncurses = "0.0.8"
uncurses-ratatui = "0.0.8"
ratatui = "0.30"
```

The rest of this website follows the current repository source.
For unreleased APIs, use matching git dependencies from
[the repository](https://github.com/aymanbagabas/uncurses).

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
uncurses = { version = "0.0.8", default-features = false, features = ["icu"] }
```

To await events instead of blocking a thread, add `async`:

```toml
[dependencies]
uncurses = { version = "0.0.8", features = ["async"] }
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

## Next steps

With the crate added, the next page writes the smallest complete interactive
program: [Hello, terminal]({{< relref "hello-world.md" >}}).

The [uncurses skill](https://github.com/aymanbagabas/uncurses/blob/main/skills/README.md)
helps coding assistants write uncurses applications and find relevant examples.
