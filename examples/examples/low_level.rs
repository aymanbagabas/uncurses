//! The low-level building blocks, without the `Screen` facade.
//!
//! The high-level [`Screen`](uncurses::screen::Screen) wraps a `Terminal`,
//! a `Canvas`, and an `EventSource` and manages the lifecycle for you.
//! This example wires the same three pieces by hand to show what `Screen`
//! does under the hood: enter raw mode and the alternate screen, draw a
//! frame through the cell-diffing `Canvas`, read input from the
//! `EventSource`, and tear it all back down on exit.
//!
//! Run with `cargo run --example low_level`. Press any key to quit.

use std::io::Write;

use uncurses::buffer::SurfaceMut;
use uncurses::canvas::Canvas;
use uncurses::color::BasicColor;
use uncurses::event::{Event, EventSource};
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout, Terminal};
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    // 1. The device handle. make_raw() stashes the prior tty state so
    //    restore() can put it back with no arguments.
    let mut term = Terminal::stdio();
    term.make_raw()?;

    // 2. The cell grid plus diffing renderer, sized to the window.
    let size = term.get_window_size().unwrap_or_default();
    let mut canvas = Canvas::new(term.output(), (size.col, size.row));

    // 3. The input decoder over the terminal's input half.
    let mut events = EventSource::new(term.input())?;

    // Switch to the alternate screen and hide the cursor. These mode
    // setters only stage bytes into the canvas buffer; the next flush
    // (here, inside present()) sends them.
    canvas.set_alt_screen(true);
    canvas.set_cursor_visible(false);

    let result = run(&mut canvas, &mut events);

    // Teardown: reset() stages the teardown for every mode the canvas
    // turned on, flush sends it, and restore() drops raw mode. Run it even
    // if the loop erred, so a crash never wrecks the terminal.
    canvas.reset();
    canvas.flush()?;
    term.restore()?;
    result
}

fn run(canvas: &mut Canvas<Stdout>, events: &mut EventSource<Stdin>) -> std::io::Result<()> {
    redraw(canvas);
    canvas.present()?;

    loop {
        match events.read()? {
            // Any key quits.
            Event::KeyPress(_) => break,
            Event::Resize(ws) => {
                canvas.resize(ws.col, ws.row);
                redraw(canvas);
                canvas.present()?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn redraw(canvas: &mut Canvas<Stdout>) {
    canvas.clear();
    let w = canvas.width();
    let h = canvas.height();

    let msg = "Low-level Canvas. Press any key to quit.";
    let style = Style::default().bold().fg(BasicColor::BrightCyan.into());
    let x = w.saturating_sub(msg.len() as u16) / 2;
    canvas.set_str((x, h / 2), msg, style);
}
