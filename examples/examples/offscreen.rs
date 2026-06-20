//! Render off-screen to an in-memory buffer, then replay it on your terminal.
//!
//! [`Canvas`] is the cell grid plus the diffing renderer over any [`Write`]
//! sink — and the sink does not have to be a terminal. Here it is a
//! `Vec<u8>`: we paint a little glamour card (rounded border, a colored
//! title bar, a true-color gradient, a wide emoji), render it, and the
//! renderer's exact bytes land in the vector. No terminal was touched.
//!
//! To prove those bytes are the real thing, we then write them straight to
//! stdout, so the frame that was composed with no terminal shows up on
//! yours. This is the building block for snapshot tests, transcript
//! recorders, or shipping frames over a socket.
//!
//! Run with `cargo run --example offscreen`.

use std::io::{self, Write};

use uncurses::buffer::{Surface, SurfaceMut};
use uncurses::canvas::Canvas;
use uncurses::cell::Cell;
use uncurses::color::{BasicColor, Color};
use uncurses::layout::{Position, Rect};
use uncurses::style::Style;
use uncurses::text::TextSurface;

const W: u16 = 46;
const H: u16 = 9;

fn main() -> io::Result<()> {
    // A Canvas over a growable byte buffer. The "screen" is the Vec.
    let mut canvas: Canvas<Vec<u8>> = Canvas::new(Vec::new(), (W, H));
    draw_card(&mut canvas);

    // `render` diffs the frame into escape bytes; `flush` drains them into
    // the underlying writer (our Vec).
    canvas.render();
    canvas.flush()?;
    let frame = canvas.writer().clone();

    let mut out = io::stdout().lock();
    writeln!(
        out,
        "Composed a {W}x{H} frame in {} bytes with no terminal involved.",
        frame.len()
    )?;
    writeln!(out, "Replaying those exact bytes on your terminal:\n")?;

    // Write the off-screen-rendered bytes to the real terminal. The frame
    // is inline (no alternate screen), so it paints right here.
    out.write_all(&frame)?;
    writeln!(out, "\n")?;

    // And the same grid read back as plain text, straight from the cells.
    writeln!(out, "The grid as plain text:")?;
    for y in 0..canvas.height() {
        let mut line = String::new();
        for x in 0..canvas.width() {
            line.push_str(canvas.cell(Position::new(x, y)).map_or(" ", Cell::content));
        }
        writeln!(out, "{}", line.trim_end())?;
    }
    out.flush()
}

fn draw_card(canvas: &mut Canvas<Vec<u8>>) {
    let w = canvas.width();
    let h = canvas.height();

    // Rounded border.
    let border = Style::default().fg(BasicColor::BrightBlack.into());
    let edge = |s: &str| Cell::narrow(s).style(border.clone());
    canvas.fill_rect(Rect::new(1, 0, w - 2, 1), &edge("─"));
    canvas.fill_rect(Rect::new(1, h - 1, w - 2, 1), &edge("─"));
    canvas.fill_rect(Rect::new(0, 1, 1, h - 2), &edge("│"));
    canvas.fill_rect(Rect::new(w - 1, 1, 1, h - 2), &edge("│"));
    canvas.set_cell((0, 0), &edge("╭"));
    canvas.set_cell((w - 1, 0), &edge("╮"));
    canvas.set_cell((0, h - 1), &edge("╰"));
    canvas.set_cell((w - 1, h - 1), &edge("╯"));

    // A colored title chip on the top border.
    let title = Style::default()
        .bold()
        .fg(BasicColor::Black.into())
        .bg(BasicColor::BrightMagenta.into());
    canvas.set_str((3, 0), " uncurses ", title);

    // Headline with a wide emoji to show off-screen wide-cell handling.
    canvas.set_str(
        (3, 2),
        "Rendered off-screen ✨",
        Style::default().bold().fg(BasicColor::BrightWhite.into()),
    );
    canvas.set_str(
        (3, 3),
        "cells in, escape bytes out",
        Style::default().fg(BasicColor::BrightBlack.into()),
    );

    // A true-color gradient bar, one background per column.
    let bar = Rect::new(3, 5, w.saturating_sub(6), 1);
    for i in 0..bar.width {
        let hue = f32::from(i) / f32::from(bar.width.max(1)) * 300.0;
        let (r, g, b) = hsv_to_rgb(hue, 0.85, 1.0);
        let cell = Cell::narrow(" ").style(Style::default().bg(Color::Rgb(r, g, b)));
        canvas.set_cell((bar.x + i, bar.y), &cell);
    }
    canvas.set_str(
        (3, 6),
        "24-bit color, downsampled to fit",
        Style::default().fg(BasicColor::BrightBlack.into()),
    );
}

/// Minimal HSV-to-RGB for `s`, `v` in `0.0..=1.0` and `h` in degrees.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let hp = (h / 60.0).rem_euclid(6.0);
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to = |f: f32| ((f + m) * 255.0).round() as u8;
    (to(r), to(g), to(b))
}
