# uncurses examples

Runnable demos for the [uncurses](../README.md) workspace. Run any of them
with:

```sh
cargo run --example <name>
```

They are grouped by what they show. If you are not sure where to start,
read one example from each of the first three sections — "read input
only", "draw only", and "the full mix" — and you will have seen the whole
shape of the library.

## Start here

| Example | What it shows |
| --- | --- |
| `counter` | The simplest full `Screen` app: render a button, react to keys and mouse. The [tutorial](../website/content/docs/tutorial.md) builds it step by step. |
| `low_level` | The same lifecycle by hand — a `Terminal`, a `TextBuffer`, and an `EventSource` wired up directly, no `Screen` facade. |

## Read input only

No rendering. Just turn terminal bytes into typed events.

| Example | What it shows |
| --- | --- |
| `input_only` | A raw-mode `EventSource` that decodes and prints every event. Doubles as a "what does this key send?" probe. |
| `query` | The request/reply model: write a query from the `ansi` module, read the answer back as an `Event` (background color, cursor position, cell size, device attributes). |

## Draw only

No event loop driving the output. Render and go.

| Example | What it shows |
| --- | --- |
| `draw_only` | A bouncing marquee on a fixed frame budget, rendered with an output-only `Screen` (no capability queries, since it reads no input), with a countdown to exit. |
| `offscreen` | A `TextBuffer` rendering a framed card into a byte buffer via the `Encode` trait, with no terminal at all — the building block for snapshot tests and transcripts. |
| `styles` | SGR attributes, the underline variants, 16/256/true color, and an OSC 8 hyperlink, written to stdout with a `Style` as the opening sequence and an empty `Style` as the reset, dropped into plain `writeln!`. |

## The full mix (input + render)

| Example | What it shows |
| --- | --- |
| `interactive` | A minimal app that polls for events with a timeout, ticking a clock even while idle. |
| `terminal` | A compact `Screen` app feeding both rendering and the event loop. |
| `cursor_pad` | A scratch pad: move the cursor with the arrows or the mouse and type into the grid. |
| `modal` | A modal dialog toggled over a static background. |
| `tour` | An animated tour through framing, color, and text-rendering features. |
| `editor` | Shell out to `$EDITOR` with `Screen::pause` / `resume`, then show the edited text. |
| `paste` | Bracketed paste: capture pasted text as one unit and reassemble the chunks into a `Vec<u8>`. |
| `paste_to_file` | Streaming paste that keeps small pastes in memory and spills large ones to a temp file past a threshold. |
| `gradient` | A half-block color field (two colors per cell) built from HSL, with a Photoshop-style hover inspector: click to open a swatch/hex/RGB/HSL panel, using pixel-accurate mouse to read the exact sub-pixel where supported. |
| `keylog` | A logger that prints every decoded event, with suspend and resume. |
| `chaos` | A stress test: pre-generated random frames pushed as fast as the renderer allows. |

## Mouse and async

| Example | What it shows |
| --- | --- |
| `mouse` | Mouse tracking: clicks, motion, and the scroll wheel, enabled through `ScreenOptions`. |
| `async_input` | The same decode-and-react loop, but `.await`-driven via `Screen::events` (the `async` feature) on a tokio runtime. |
| `file_explorer` | A two-pane file explorer with a live preview, driven by the async event stream. |

## Inline rendering (no alternate screen)

These paint a small region anchored in the normal buffer, leaving
scrollback and the returning shell prompt intact.

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

Bring [ratatui](https://docs.rs/ratatui) widgets and let uncurses render
them. See [`uncurses-ratatui`](../uncurses-ratatui/README.md).

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
