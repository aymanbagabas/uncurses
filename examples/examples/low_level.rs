//! The low-level building blocks, without the `Screen` facade.
//!
//! The high-level [`Screen`](uncurses::screen::Screen) wraps a `Terminal`,
//! a diff renderer, and an `EventSource` and manages the lifecycle for you.
//! This example wires the pieces by hand to show what the layers below
//! `Screen` look like: enter raw mode and the alternate screen with the
//! [`ansi`](uncurses::ansi) helpers, paint a frame into a
//! [`TextBuffer`](uncurses::buffer::TextBuffer), serialize it with the
//! [`Encode`](uncurses::text::Encode) trait, read input from the
//! `EventSource`, and tear it all back down on exit.
//!
//! Unlike `Screen`, a `TextBuffer` does no cell diffing: each frame is a
//! full repaint. We erase the screen and re-encode the whole buffer every
//! time, which is exactly the right tool for simple or one-shot output.
//!
//! Run with `cargo run --example low_level`. Press any key to quit.

use std::io::Write;

use uncurses::ansi::cursor::write_cup;
use uncurses::ansi::mode::{Mode, write_reset_mode, write_set_mode};
use uncurses::ansi::screen::write_erase_screen;
use uncurses::buffer::{SurfaceMut, TextBuffer};
use uncurses::color::Color;
use uncurses::event::{Event, EventSource};
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout, Terminal};
use uncurses::text::{Encode, TextSurface};

fn main() -> std::io::Result<()> {
    // 1. The device handle. make_raw() stashes the prior tty state so
    //    restore() can put it back with no arguments.
    let mut term = Terminal::stdio();
    term.make_raw()?;

    // 2. The cell grid, sized to the window. TextBuffer carries the width
    //    policy used to measure strings; it does no diffing.
    let size = term.get_window_size().unwrap_or_default();
    let mut buf = TextBuffer::new(size.col, size.row);

    // 3. The input decoder over the terminal's input half, and an owned
    //    handle to the output half to write escapes through.
    let mut events = EventSource::new(term.input())?;
    let mut out = term.output();

    // Switch to the alternate screen and hide the cursor by writing the mode
    // escapes straight to the output half.
    write_set_mode(&mut out, &[Mode::ALT_SCREEN_SAVE_CURSOR])?;
    write_reset_mode(&mut out, &[Mode::CURSOR_VISIBLE])?;
    out.flush()?;

    let result = run(&mut out, &mut buf, &mut events);

    // Teardown: show the cursor, leave the alternate screen, flush, and drop
    // raw mode. Run it even if the loop erred so a crash never wrecks the
    // terminal.
    write_set_mode(&mut out, &[Mode::CURSOR_VISIBLE])?;
    write_reset_mode(&mut out, &[Mode::ALT_SCREEN_SAVE_CURSOR])?;
    out.flush()?;
    term.restore()?;
    result
}

fn run(
    out: &mut Stdout,
    buf: &mut TextBuffer,
    events: &mut EventSource<Stdin>,
) -> std::io::Result<()> {
    redraw(buf);
    present(out, buf)?;

    loop {
        match events.read()? {
            // Any key quits.
            Event::KeyPress(_) => break,
            Event::Resize(ws) => {
                buf.resize(ws.col, ws.row);
                redraw(buf);
                present(out, buf)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn redraw(buf: &mut TextBuffer) {
    buf.clear();
    let w = buf.width();
    let h = buf.height();

    let msg = "Low-level TextBuffer. Press any key to quit.";
    let style = Style::default().bold().fg(Color::BrightCyan);
    let x = w.saturating_sub(msg.len() as u16) / 2;
    buf.set_str((x, h / 2), msg, style);
}

/// Paint a full frame: erase the screen, home the cursor, and serialize the
/// whole buffer with [`Encode`]. There is no diffing here, so every frame is
/// a complete repaint.
fn present(out: &mut Stdout, buf: &TextBuffer) -> std::io::Result<()> {
    write_erase_screen(out)?;
    write_cup(out, 0, 0)?;
    buf.encode(out)?;
    out.flush()
}
