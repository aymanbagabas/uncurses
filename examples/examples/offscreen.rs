//! Render off-screen to an in-memory buffer, no terminal required.
//!
//! [`Canvas`] is just a cell grid plus a diffing renderer over any
//! [`Write`] sink. The sink does not have to be a terminal: here it is a
//! `Vec<u8>`. We paint a framed greeting, render it, and then print the
//! exact bytes the renderer produced (escapes shown as their `Debug`
//! escapes). This is the building block for snapshot tests, transcript
//! recorders, sending frames over a socket, or feeding another transport.
//!
//! Run with `cargo run --example offscreen`. It writes to stdout like a
//! normal program and exits; nothing about the terminal is touched.

use std::io::Write;

use uncurses::buffer::{Surface, SurfaceMut};
use uncurses::canvas::Canvas;
use uncurses::cell::Cell;
use uncurses::color::BasicColor;
use uncurses::layout::{Position, Rect};
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    // A Canvas over a growable byte buffer. Nothing reaches a terminal;
    // the "output" is whatever ends up in the Vec.
    let mut canvas: Canvas<Vec<u8>> = Canvas::new(Vec::new(), (24, 5));

    draw_card(&mut canvas);

    // `render` diffs the frame into escape bytes; `flush` drains them into
    // the underlying writer (our Vec).
    canvas.render();
    canvas.flush()?;

    // Read the captured bytes back out of the sink.
    let frame = canvas.writer();
    println!("Rendered {} bytes off-screen.\n", frame.len());

    // Show the raw escape stream the renderer emitted.
    println!("--- raw output (escapes shown) ---");
    println!("{}", String::from_utf8_lossy(frame).escape_debug());

    // And a plain-text view: pull each cell's symbol straight from the grid.
    println!("\n--- text view ---");
    for y in 0..canvas.height() {
        let mut line = String::new();
        for x in 0..canvas.width() {
            line.push_str(canvas.cell(Position::new(x, y)).map_or(" ", Cell::content));
        }
        println!("{}", line.trim_end());
    }
    Ok(())
}

fn draw_card(canvas: &mut Canvas<Vec<u8>>) {
    let w = canvas.width();
    let h = canvas.height();
    let border = Style::default().fg(BasicColor::Cyan.into());
    let edge = |s: &str| Cell::narrow(s).style(border.clone());

    // Top and bottom edges.
    canvas.fill_rect(Rect::new(0, 0, w, 1), &edge("─"));
    canvas.fill_rect(Rect::new(0, h - 1, w, 1), &edge("─"));
    // Left and right edges.
    canvas.fill_rect(Rect::new(0, 0, 1, h), &edge("│"));
    canvas.fill_rect(Rect::new(w - 1, 0, 1, h), &edge("│"));
    // Corners.
    canvas.set_cell((0, 0), &edge("┌"));
    canvas.set_cell((w - 1, 0), &edge("┐"));
    canvas.set_cell((0, h - 1), &edge("└"));
    canvas.set_cell((w - 1, h - 1), &edge("┘"));

    let text = Style::default().bold().fg(BasicColor::BrightWhite.into());
    canvas.set_str((2, 2), "Hello, off-screen!", text);
}
