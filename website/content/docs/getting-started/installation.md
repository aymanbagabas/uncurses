---
title: "Installation"
weight: 1
---

uncurses is currently installed from Git. It is not published on crates.io yet.

## Add the dependency

In your `Cargo.toml`:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
```

Then build normally:

```sh
cargo check
```

{{< callout type="info" >}}
The repository also contains `uncurses-ratatui`, a ratatui backend crate for using ratatui widgets with uncurses rendering.
{{< /callout >}}

## Rust version

uncurses tracks the latest stable Rust and uses edition 2024.

Edition 2024 itself is the baseline, and current source uses let-chains, so the effective MSRV is about Rust 1.88. Use a recent stable toolchain:

```sh
rustup update stable
```

## Feature flags

The default feature set is usually right for a first app:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
```

Choose features when you need different Unicode behavior or async event delivery.

| Feature | Default | What it adds | Choose it when |
| --- | --- | --- | --- |
| `unicode-rs` | Yes | Pure-Rust width and grapheme segmentation through `unicode-width` and `unicode-segmentation`. | You want the default, small, fast Unicode stack. |
| `icu` | No | ICU4X-backed segmentation and Unicode properties. It takes precedence over `unicode-rs` when both are enabled. | You care more about emoji and grapheme edge-case correctness than binary size. |
| `async` | No | `EventStream`, a runtime-agnostic `futures_core::Stream` of decoded events. | Your event loop is async and you want `.await`-driven input without uncurses choosing a runtime. |
| `bench` | No | Exposes benchmark-only renderer internals and enables the renderer benchmark target. | You are running or developing uncurses benchmarks. |

For ICU-backed Unicode:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses", features = ["icu"] }
```

For async input:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses", features = ["async"] }
```

## Supported platforms

uncurses supports modern VT100/xterm-style terminals on Linux, macOS, Windows, and the BSDs. It does not use terminfo; when you need to know what the terminal supports, ask it with the query APIs and read the answer through events.

## Next steps

- Write the smallest complete program in [Hello, terminal]({{< relref "hello-world.md" >}}).
- Learn the main building blocks in [The four layers]({{< relref "the-layers.md" >}}).
- Browse the generated [API reference](../../../api/).
