---
name: uncurses
description: This skill should be used when building a terminal UI in Rust with the uncurses crate, or when the user asks to "use uncurses", "build a TUI with uncurses", "draw to the terminal", "read keyboard/mouse events", "handle terminal resize", "use uncurses with ratatui", "enable mouse in uncurses", "read events async", or is writing code that imports `uncurses::`. Provides the uncurses mental model plus the non-obvious gotchas an LLM will not infer from type signatures.
version: 0.1.0
---

# uncurses

uncurses is a terminal toolkit library for Rust: building blocks for a terminal
UI with no terminfo, no widget tree, no hidden global state, no framework. It
talks to a modern VT100/xterm-style terminal directly. The full API reference,
guides, and concepts live at **https://uncurses.org** and in rustdoc. This skill
covers the mental model and the sharp edges, not the whole API.

## Mental model

- A `Screen` owns the terminal session. It is a double-buffered cell grid: draw
  into the back buffer, then `render()` diffs and flushes only what changed.
- A session is bracketed: `init()` sets up raw mode and probes capabilities;
  `finish()` restores the terminal, always, in one call. Treat `finish()` like a
  destructor: call it on every exit path.
- Event reads are pure: `read_event(&self)`, `try_read_event(&self)`,
  `poll_event(&self, timeout)`, `unread_event(&self, event)`, and async
  `event_stream(&self)` do not mutate screen capabilities. Feed each event
  through `screen.observe_event(&ev)?` to keep capability tracking alive (mouse,
  kitty keyboard, in-band resize, truecolor, grapheme); skip it and reads still
  work, you just lose those upgrades. ratatui's uncurses backend reads the same
  pure way, so call `observe_event` there too.
- Default session is **inline** (draws in the normal buffer, cursor visible).
  The alternate screen and a hidden cursor are opt-in, not the default.
- Coordinates: `x`/`y` are **0-based**, origin top-left (matching the `Position`
  API). Terminal rows/columns in docs are 1-based like CUP; the code API is
  0-based.

## The canonical skeleton

Compile-proven. Start from this shape, not from memory:

```rust,no_run
use uncurses::buffer::Bounded;
use uncurses::event::Event;
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init()?; // raw mode + capability detection; inline, cursor visible
    let w = screen.width();
    screen.resize((w, 2)); // inline: one text row plus a trailing blank line

    screen.set_str((0, 0), "Hello! Press q to quit.", Style::new());
    screen.render()?;

    loop {
        let ev = screen.read_event()?;
        screen.observe_event(&ev)?;
        if matches!(ev, Event::KeyPress(ref k) if k.matches("q")) {
            break;
        }
    }

    screen.finish() // restore the terminal, always, one call
}
```

For inline apps, reserve one extra trailing blank row (e.g. `resize((w, 2))` for
one text line) so the shell prompt resumes on a fresh line after `finish()`.

## Gotchas (the reason this skill exists)

These are the things that produce confidently-wrong uncurses code:

- **`KeyCode::Space` is its own variant, not `KeyCode::Char(' ')`.** A
  `Char(' ')` match arm never fires. Enter, Tab, and Backspace are likewise
  their own named variants. Match `KeyCode::Space`, not `Char(' ')`.
- **Matching keys: prefer `Key::matches` / `matches_any`.** `PartialEq`/`Hash`/`Display`/
  `FromStr` all compare the *structural* key identity (code + non-lock
  modifiers) and ignore the `text`, `shifted_key`, and `base_key` fields. Use
  `key.matches("ctrl+c")` or `key.matches_any(["esc", "ctrl+c"])`. `matches()`
  is more permissive: it consults the typed `text` first (good for layout-
  independent bindings like `"!"` or `"G"`), then falls back to structural
  parsing.
- **`Key::text` is the typed glyph, not the binding.** `to_string()`/`Display`
  spell the space bar as `"space"`, never `" "`. For the character the user
  actually typed, read `key.text` or call `key.char()`.
- **`enable_mouse()` is not capability-gated.** It emits mouse modes
  unconditionally (1000+1002 tracking, 1006 SGR, +1003 for motion, +1016 for
  pixels); unsupported terminals ignore modes and degrade to SGR cells. Turn it
  off with `disable_mouse()`. Mouse-tracking options are `MouseTracking`
  bitflags (`MOTION`, `PIXELS`); there is no named zero flag, use
  `MouseTracking::empty()`.
- **`init()` probes the terminal by default.** It queries a fixed capability set
  (kitty keyboard, mouse/sync/resize/unicode DEC modes, XTVERSION, truecolor via
  XTGETTCAP). Reads are pure and do not apply replies or capabilities; raw
  `Screen` code applies them by passing each event to `observe_event(&ev)?`. Opt
  out with `ScreenOptions { query_capabilities: false, ..Default::default() }` if
  a clean handshake is required.
- **Cursor: stage, then render.** `set_cursor_position(pos)` stages the desired
  cursor position; it is flushed as part of the next `render()`, not
  immediately. `clear_cursor_position()` unstages it. `show_cursor()` /
  `hide_cursor()` control visibility. (`move_cursor_to` / `move_cursor_by` do
  flush right away, for imperative use outside the render loop.)
- **Bitflag sets never get a named zero value.** `KeyModifiers`, `AttrFlags`,
  `MouseTracking`, kitty keyboard flags all use `::empty()` for "none". Do not
  look for or add a `NONE` variant.
- **Renderer/decode/poll are internals.** Do not import `renderer`,
  `event::decode`, or `event::poll`; they are `pub(crate)`. Use the screen and
  event APIs.

## Async events

With the `async` feature, `screen.event_stream()` returns an owned
`EventStream` (a `futures_core::Stream`) over the screen's own decoder. Use it
with any futures-compatible runtime or executor. Call `screen.observe_event(&ev)?`
before handling an event when you want capability tracking. There is no
`Screen::events()` adapter. See `examples/async_screen.rs` and
`examples/async_arcade.rs`.

## ratatui

The `uncurses-ratatui` crate bridges uncurses to a ratatui backend. See
`examples/ratatui_hello_world.rs` and the other `ratatui_*.rs` examples. Backend
event reads are pure too: use `read_event`, `try_read_event`, `poll_event`, or
`event_stream()`, then call `backend.observe_event(&ev)?` when you want
capability tracking.

## Cargo features

- `unicode-rs` *(default)*: `unicode-segmentation` + `unicode-width` for width
  and grapheme segmentation.
- `icu`: ICU4X-backed segmentation/width for maximum correctness.
- `async`: the `EventStream` described above.

Do not rename `unicode-rs` to `unicode`; both backends implement Unicode (ICU =
International Components for Unicode).

## Where to look next

Before writing non-trivial code, read the closest example in the crate's
`examples/examples/` directory rather than guessing the API:

- **Minimal / interactive:** `counter.rs`, `interactive.rs`, `tour.rs`
- **Inline & modal:** `inline_input.rs`, `modal_inline.rs`, `marquee_hello.rs`
- **Input:** `keylog.rs`, `input_only.rs`, `paste.rs`, `mouse.rs`, `cursor_pad.rs`
- **Async / custom events:** `async_screen.rs`, `async_arcade.rs`, `async_input.rs`,
  `custom_events.rs`
- **Capabilities / low level:** `query.rs`, `low_level.rs`, `terminal.rs`, `offscreen.rs`
- **ratatui:** `ratatui_hello_world.rs`, `ratatui_user_input.rs`, `ratatui_popup.rs`
- **Styling:** `styles.rs`, `gradient.rs`

For anything not covered here, consult **https://uncurses.org** (guides +
concepts) and the rustdoc API reference. Prefer reading the actual example or
rustdoc over inventing method names.
