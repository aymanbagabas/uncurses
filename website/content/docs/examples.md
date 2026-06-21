---
title: Examples
weight: 6
---

The workspace ships a large set of runnable demos, grouped by what they show.
Run any of them from the repository root:

```sh
cargo run --example <name>
```

If you are not sure where to start, read one example from each of the first
three sections, "read input only", "draw only", and "the full mix", and you
will have seen the whole shape of the library.

{{< callout type="info" >}}
Browse the source for any example under
[`examples/examples/`](https://github.com/aymanbagabas/uncurses/tree/main/examples/examples)
on GitHub.
{{< /callout >}}

## Start here

| Example | What it shows |
| --- | --- |
| `counter` | The simplest full `Screen` app: render a button, react to keys and mouse. The [tutorial]({{< relref "tutorial.md" >}}) builds it step by step. |
| `low_level` | The same lifecycle by hand: a `Terminal`, a `Canvas`, and an `EventSource` wired up directly, no `Screen` facade. |

## Read input only

No rendering, just turn terminal bytes into typed events.

| Example | What it shows |
| --- | --- |
| `input_only` | A raw-mode `EventSource` that decodes and prints every event. Doubles as a "what does this key send?" probe. |
| `query` | The request/reply model: write a query from the `ansi` module, read the answer back as an `Event`. |

## Draw only

No event loop driving the output. Render and go.

| Example | What it shows |
| --- | --- |
| `draw_only` | A bouncing marquee on a fixed frame budget, rendered with `Canvas` directly (no `Screen`, since it reads no input). |
| `offscreen` | A `Canvas<Vec<u8>>` rendering a framed card into a byte buffer with no terminal at all. |
| `styles` | SGR attributes, underline variants, 16/256/true color, and an OSC 8 hyperlink, written to stdout with `Style` open/close sequences. |

## The full mix (input + render)

| Example | What it shows |
| --- | --- |
| `interactive` | A minimal app that polls for events with a timeout, ticking a clock even while idle. |
| `terminal` | A compact `Screen` app feeding both rendering and the event loop. |
| `cursor_pad` | A scratch pad: move the cursor with the arrows or the mouse and type into the grid. |
| `modal` | A modal dialog toggled over a static background. |
| `tour` | An animated tour through framing, color, and text-rendering features. |
| `editor` | Shell out to `$EDITOR` with `Screen::pause` / `resume`, then show the edited text. |
| `paste` | Bracketed paste: capture pasted text as one unit and reassemble the chunks. |
| `paste_to_file` | Streaming paste that keeps small pastes in memory and spills large ones to a temp file. |
| `gradient` | A half-block color field with a Photoshop-style hover inspector (swatch, hex, RGB, HSL). |
| `keylog` | A logger that prints every decoded event, with suspend and resume. |
| `chaos` | A stress test: pre-generated random frames pushed as fast as the renderer allows. |

## Mouse and async

| Example | What it shows |
| --- | --- |
| `mouse` | Mouse tracking: clicks, motion, and the scroll wheel, enabled through `ScreenOptions`. |
| `async_input` | The same decode-and-react loop, but `.await`-driven via `Screen::events` (the `async` feature). |
| `file_explorer` | A two-pane file explorer with a live preview, driven by the async event stream. |

## Inline rendering (no alternate screen)

These paint a small region anchored in the normal buffer, leaving scrollback
and the returning shell prompt intact.

| Example | What it shows |
| --- | --- |
| `inline_input` | A multi-line prompt that grows as you type and commits into the scrollback. |
| `task_picker` | A two-view flow (pick a task, watch a progress bar) sized to each frame. |
| `card_swap` | Two layered cards at a fixed inline height; press a key to swap their order. |
| `modal_inline` | A modal with a scrim over inline content. |
| `screen_toggle` | Flips a live app between inline and the alternate screen. |

## Animation and throughput

| Example | What it shows |
| --- | --- |
| `space` | An animated grayscale starfield, capped at 60 FPS. |
| `space_unlimited` | The same starfield, uncapped, for measuring raw renderer throughput. |

## ratatui backend

Bring [ratatui](https://docs.rs/ratatui) widgets and let uncurses render them.
See the [ratatui backend guide](../guides/ratatui-backend/).

| Example | What it shows |
| --- | --- |
| `ratatui_hello` | A minimal ratatui app on the uncurses backend. |
| `ratatui_minimal` | Port of ratatui's `minimal` example. |
| `ratatui_hello_world` | Port of ratatui's `hello-world` example. |
| `ratatui_modifiers` | Port of ratatui's `modifiers` example (text attributes). |
| `ratatui_popup` | Port of ratatui's `popup` example. |
| `ratatui_user_input` | Port of ratatui's `user-input` example. |
| `ratatui_inline` | An inline viewport anchored at the cursor. |
| `ratatui_space` | An animated starfield rendered through ratatui, capped at 60 FPS. |
| `ratatui_space_unlimited` | The uncapped variant. |
