//! Render off-screen to an in-memory buffer, then replay it on your terminal.
//!
//! A [`TextBuffer`] is a width-aware cell grid with no terminal attached.
//! Paint into it, then serialize it with the [`Encode`] trait straight into a
//! `Vec<u8>`: we compose a little glamour card (rounded border, a colored
//! title bar, a true-color gradient, a wide emoji) and the escape bytes land
//! in the vector. No terminal was touched, and there is no diffing — every
//! call produces a complete, standalone frame.
//!
//! To prove those bytes are the real thing, we then write them straight to
//! stdout, so the frame that was composed with no terminal shows up on
//! yours. This is the building block for snapshot tests, transcript
//! recorders, or shipping frames over a socket.
//!
//! Run with `cargo run --example offscreen`.

use std::io::{self, Write};

use uncurses::buffer::{Surface, SurfaceMut, TextBuffer};
use uncurses::cell::Cell;
use uncurses::color::Color;
use uncurses::layout::{Position, Rect};
use uncurses::style::Style;
use uncurses::text::{Encode, TextSurface};

const W: u16 = 46;
const H: u16 = 9;

fn main() -> io::Result<()> {
    // A width-aware grid with no terminal behind it. The "screen" is memory.
    let mut buf = TextBuffer::new(W, H);
    draw_card(&mut buf);

    // `encode` serializes the whole grid to escape bytes in one pass: one row
    // per line, default-styled at each row's edges, trailing blanks trimmed.
    let mut frame = Vec::new();
    buf.encode(&mut frame)?;

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
    for y in 0..buf.height() {
        let mut line = String::new();
        for x in 0..buf.width() {
            line.push_str(buf.cell(Position::new(x, y)).map_or(" ", Cell::content));
        }
        writeln!(out, "{}", line.trim_end())?;
    }
    out.flush()
}

fn draw_card(buf: &mut TextBuffer) {
    let w = buf.width();
    let h = buf.height();

    // Rounded border.
    let border = Style::default().fg(Color::BrightBlack);
    let edge = |s: &str| Cell::narrow(s).style(border);
    buf.fill_rect(Rect::new(1, 0, w - 2, 1), &edge("─"));
    buf.fill_rect(Rect::new(1, h - 1, w - 2, 1), &edge("─"));
    buf.fill_rect(Rect::new(0, 1, 1, h - 2), &edge("│"));
    buf.fill_rect(Rect::new(w - 1, 1, 1, h - 2), &edge("│"));
    buf.set_cell((0, 0).into(), &edge("╭"));
    buf.set_cell((w - 1, 0).into(), &edge("╮"));
    buf.set_cell((0, h - 1).into(), &edge("╰"));
    buf.set_cell((w - 1, h - 1).into(), &edge("╯"));

    // A colored title chip on the top border.
    let title = Style::default()
        .bold()
        .fg(Color::Black)
        .bg(Color::BrightMagenta);
    buf.set_str((3, 0), " uncurses ", title);

    // Headline with a wide emoji to show off-screen wide-cell handling.
    buf.set_str(
        (3, 2),
        "Rendered off-screen ✨",
        Style::default().bold().fg(Color::BrightWhite),
    );
    buf.set_str(
        (3, 3),
        "cells in, escape bytes out",
        Style::default().fg(Color::BrightBlack),
    );

    // A true-color gradient bar. Each cell is a left-half block `▌`: its
    // foreground is the left sub-pixel and its background the right one, so the
    // bar carries twice as many color steps as it has columns.
    let bar = Rect::new(3, 5, w.saturating_sub(6), 1);
    let cols = u32::from(bar.width) * 2;
    for i in 0..bar.width {
        let left = gradient_color(u32::from(i) * 2, cols);
        let right = gradient_color(u32::from(i) * 2 + 1, cols);
        let cell = Cell::narrow("▌").style(Style::default().fg(left).bg(right));
        buf.set_cell((bar.x + i, bar.y).into(), &cell);
    }
    buf.set_str(
        (3, 6),
        "24-bit color, encoded as written",
        Style::default().fg(Color::BrightBlack),
    );
}

/// Color of one gradient sub-pixel at column `step` of `cols` total columns.
fn gradient_color(step: u32, cols: u32) -> Color {
    let hue = step as f32 / cols.max(1) as f32 * 300.0;
    Color::hsl(hue, 0.85, 0.55)
}
