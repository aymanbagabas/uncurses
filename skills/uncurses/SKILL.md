---
name: uncurses
description: Use for Rust terminal UIs with the uncurses crate or code that imports uncurses::. Covers terminal sessions, drawing, keyboard and mouse input, resize, async events, and the uncurses-ratatui backend. Includes the ownership model, examples, and API pitfalls.
version: 0.1.0
---

# uncurses

uncurses provides terminal UI components for Rust. It communicates directly
with modern terminals and discovers capabilities through queries instead of
terminfo. This skill covers the ownership model and common mistakes. Use
**https://uncurses.org**, rustdoc, and the source for detailed API information.

Check the dependency revision before you copy an example. This skill follows
the source in this checkout.

## Mental model

- `Program` owns the terminal session, input source, capabilities, and a
  `Screen`. Use `Program::stdio()` for a session over standard input and output.
- `Screen<W>` owns a cell grid, renderer, and writer. Draw through
  `program.screen_mut()`, then call `render()` to diff and flush the frame.
  For output alone, use `Screen::new(writer, size)` with any `Write`.
- `program.init()` enters raw mode and applies default session settings.
  Capability discovery starts when you call `program.query_capabilities(&[])?`.
- `program.finish()` consumes the program and restores the terminal. Call it
  after both success and failure, including a failed initialization.
- The default session draws **inline**, with a visible cursor. Use
  `program.enter_alt_screen()?` for the alt screen and `program.hide_cursor()?`
  for a hidden cursor.
- Cell coordinates are **0-based**, with the origin at the top left. Screen
  coordinates are relative to the managed area. Mouse coordinates describe
  positions in the physical terminal.

## The canonical skeleton

Use this shape for a synchronous inline application:

```rust
use uncurses::buffer::Bounded;
use uncurses::event::Event;
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    let result = (|| {
        program.init()?;
        let width = program.screen().width();
        program.screen_mut().resize((width, 2));

        loop {
            let screen = program.screen_mut();
            screen.set_str((0, 0), "Hello! Press q to quit.", Style::new());
            screen.render()?;

            match program.read_event()? {
                Event::KeyPress(key) if key.matches_any(["q", "ctrl+c"]) => break,
                Event::Resize(_) => program.autoresize()?,
                _ => {}
            }
        }
        Ok(())
    })();

    let finish = program.finish();
    result.and(finish)
}
```

The closure keeps teardown reachable when initialization, rendering,
or input fails. Both operations run before `result.and(finish)` selects the
error to return.

For inline apps, reserve a trailing blank row for the shell prompt. The
example uses two rows for one text line.

This minimal example uses the default session settings. Add explicit
capability discovery when the application needs terminal reports or automatic
mode adoption, as described below.

## Events and capability discovery

Observe each event once. Choose the rule that matches its source:

| Input path | Observation |
|---|---|
| `program.read_event()` / `try_read_event()` | Automatic; handle the returned event directly |
| `program.event_stream()` | Call `program.observe_event(&ev)?` before application handling |
| Reads through `program.event_source()` | Call `program.observe_event(&ev)?` yourself |
| `program.unread_event(ev)` | Returns an already-observed event to the program's queue |

An extra observation can consume a second pending request for the same reply.
Use one input consumer in steady state; shared readers compete for events.

- `read_event(&mut self)` blocks until an event arrives.
- `try_read_event(&mut self)` returns `io::Result<Option<Event>>`.
  `Ok(None)` means the queue is empty. It takes queued input, but observation
  can write and flush output. An observation error consumes that event.
- `poll_event(&self, timeout)` drives input and reports availability. Follow
  `Ok(true)` with a read to remove and observe the event.
- `query_capabilities(&[])` writes requests without waiting for replies.
  Process replies through the event loop. Drain pending replies before
  teardown, or the shell can receive them as input. Use the final
  `Event::PrimaryDeviceAttributes` reply to end a dedicated discovery wait.
  Bound that wait with `poll_event`; a terminal can ignore a query.
- `ProgramOptions` holds initialization settings, query choices, and mode
  preferences. The `query_*` fields apply when `query_capabilities` runs.
  The `prefer_*` fields control adoption when reports confirm support.
- Observation records resize reports. Call `program.autoresize()?` on
  `Event::Resize` to refit the managed area. Inline, it preserves the managed
  height; on the alt screen, it uses the full terminal size.

## Drawing and cursor pitfalls

- Import `TextSurface` for `set_str` and text measurement. Import `Bounded`
  for dimensions and `SurfaceMut` for cell writes and `clear`.
- `set_str` paints literal text with the supplied style. Use `text::Painter`
  when the input contains ANSI styling or hyperlink escapes.
- `set_str_truncate(pos, s, s_style, tail, tail_style)` uses separate text
  and indicator styles. `set_str_rect_truncate` takes a rectangle instead of
  a position. Both `TextSurface` and `Painter` use this argument order.
- Measure text with the destination's width policy, such as `screen.str_width`.
  Byte length and character count can differ from terminal column width.
- `Cell::new(content, width)` takes an explicit column width. Use
  `cell.content()` and `cell.width()` to read those values. Surface writes
  create continuation cells automatically. Prefer `set_str` for ordinary text.
- `screen.set_cursor_position((x, y))` stages a position that persists across
  frames. `render()` applies it after the cell diff.
  `clear_cursor_position()` ends this placement policy and preserves visibility.
- Use `program.show_cursor()` / `hide_cursor()` for terminal visibility.
  `screen.move_cursor_to()` / `move_cursor_by()` move and flush immediately.
- Program mode methods keep related screen properties consistent with the terminal.
  Screen property setters configure the renderer; use the program methods
  for session changes such as the alt screen or grapheme mode.

## Keyboard and mouse pitfalls

- Match the space bar with `KeyCode::Space`. Enter, Tab, and Backspace also
  use named variants. Legacy decoder flags control ambiguous input.
- Prefer `key.matches("ctrl+c")` or `key.matches_any(["esc", "ctrl+c"])`.
  These check typed text first, then parsed key identity.
  Equality and hashing use the code and modifiers, with lock modifiers excluded.
- For text entry, read `key.text`. Display formats a key binding, such as
  `"space"`. `key.char()` reads the code's character, including a space for
  `KeyCode::Space`; use `text` for the produced glyph.
- `ProgramOptions::legacy_keys` sets initial decoder flags. Use
  `program.decoder_flags()` and `set_decoder_flags(flags)` for runtime changes.
  Those changes preserve options, terminal modes, and already-decoded events.
  Legacy LF defaults to Ctrl+J; `DecoderFlags::LF_IS_ENTER` selects Enter.
- Backarrow reports update decoding. `program.set_backspace_mode(bool)` also
  changes the terminal mode and records a choice for teardown and resume.
- `program.enable_mouse(MouseTracking::empty())?` enables basic button and
  drag reports. `MOTION` adds hover; `PIXELS` requests pixel coordinates.
  Requests are unconditional; unsupported terminals ignore them.
  Use `program.disable_mouse()?` to disable tracking.
- For inline mouse input, request the origin with `program.request_origin()?`.
  Process its reply before you use `mouse_to_origin` for local coordinates.
  Refresh the origin after resize or resume. Check physical bounds first:
  `mouse_to_origin` clamps positions above or left of the origin to zero.
- Convert pixel reports with `mouse_pixels_to_cells` before the origin offset.
  It returns `None` when geometry is unknown. Request cell dimensions with
  `request_cell_pixel_size()` and refresh them after resize or font changes.
- Use `::empty()` for empty `KeyModifiers`, `AttrFlags`, `MouseTracking`, and
  kitty keyboard flags.

## Async events

With the `async` feature, `program.event_stream()` returns an owned
`EventStream` over the shared decoder. It implements
`futures_core::Stream<Item = io::Result<Event>>` and works with
futures-compatible executors. Handle each item error, then call
`program.observe_event(&ev)?` before application handling.

See `examples/examples/async_screen.rs` for an event loop with a frame timer.

## ratatui

`uncurses-ratatui::UncursesBackend` wraps a `Program`. Access it through
`program()` / `program_mut()`. Synchronous backend reads observe events
automatically. Async backend streams require `backend.observe_event(&ev)?`.
Start explicit discovery with `backend.program_mut().query_capabilities(&[])?`.

See `examples/examples/ratatui_hello_world.rs` for setup, drawing, and teardown.

## Cargo features

- `unicode-rs` *(default)*: `unicode-segmentation` and `unicode-width` for width
  and grapheme segmentation.
- `icu`: ICU4X segmentation and Unicode properties. Takes precedence when both
  Unicode backends are enabled.
- `async`: runtime-independent `EventStream` support.

Enable at least one Unicode backend. The feature name is `unicode-rs`.

## Where to look next

Read the closest example under `examples/examples/` in the uncurses repository
before you implement a feature. From that checkout, run
`cargo run -p examples --example <name>`.

- **Minimal / interactive:** `counter.rs`, `interactive.rs`, `tour.rs`
- **Inline & modal:** `inline_input.rs`, `modal_inline.rs`, `marquee_hello.rs`
- **Input:** `keylog.rs`, `legacy_keys.rs`, `input_only.rs`, `paste.rs`, `mouse.rs`,
  `cursor_pad.rs`
- **Async / custom events:** `async_screen.rs`, `async_arcade.rs`, `async_input.rs`,
  `custom_events.rs`
- **Capabilities / colors:** `query.rs`, `query_colors.rs`
- **Output / low level:** `draw_only.rs`, `low_level.rs`, `terminal.rs`, `offscreen.rs`
- **ratatui:** `ratatui_hello_world.rs`, `ratatui_user_input.rs`, `ratatui_popup.rs`
- **Styling:** `styles.rs`, `gradient.rs`

Use public `program`, `screen`, and `event` APIs. The `renderer`,
`event::decode`, and `event::poll` modules are internal. Check the source or
rustdoc before you choose a method name.
