//! Render off-screen to an in-memory buffer, then replay it on your terminal.
//!
//! A [`Buffer`] is a width-aware cell grid stored in memory.
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

use uncurses::buffer::{Buffer, Surface, SurfaceMut};
use uncurses::cell::Cell;
use uncurses::color::Color;
use uncurses::layout::{Position, Rect};
use uncurses::style::Style;
use uncurses::text::{Encode, TextSurface};

const W: u16 = 46;
const H: u16 = 9;

fn main() -> io::Result<()> {
    // A width-aware grid with no terminal behind it. The "screen" is memory.
    let mut buf = Buffer::new(W, H);
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
        for cell in buf.line(y).unwrap() {
            line.push_str(cell.content());
        }
        writeln!(out, "{}", line.trim_end())?;
    }

    // A cell is not limited to one or two columns. Under the default
    // `WidthMode::Wc` a joined emoji sequence measures the sum of its code
    // points, because a terminal that does not group graphemes draws each
    // face in turn. The grid credits the primary with every column it takes
    // and fills the rest with continuations, so whatever follows lands where
    // the terminal actually leaves the cursor.
    let mut wide = Buffer::new(16, 1);
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
    wide.set_str((0, 0), family, Style::default());
    wide.set_str(
        (wide.str_width(family), 0),
        "<- ends here",
        Style::default(),
    );
    let primary = wide
        .cell(Position::new(0, 0))
        .map_or(0, |cell| cell.width());
    writeln!(
        out,
        "\nA family emoji reserves {primary} columns, so the text after it starts at column {}.",
        wide.str_width(family)
    )?;

    // Width reserves the columns, not content. A cell that stores nothing
    // and claims three of them is drawn as three blanks, which makes it a
    // spacer carrying its own background.
    let mut spacer = Buffer::new(16, 1);
    spacer.set_str((0, 0), "A", Style::default());
    spacer.set_cell(
        Position::new(1, 0),
        Cell::new("", 3)
            .style(Style::default().bg(Color::Blue))
            .as_ref(),
    );
    spacer.set_str((4, 0), "B", Style::default());
    let mut gap = Vec::new();
    spacer.encode(&mut gap)?;
    write!(out, "An empty cell of width 3 is a three-column spacer: ")?;
    out.write_all(&gap)?;
    writeln!(out)?;

    let mut truncated = Buffer::new(12, 1);
    truncated.set_str_truncate(
        (0, 0),
        "A long status message",
        Style::new().fg(Color::Green),
        "...",
        Style::new().fg(Color::Red).bold(),
    );
    write!(out, "Green text with a red truncation tail: ")?;
    truncated.encode(&mut out)?;
    writeln!(out)?;

    let mut copy = Buffer::new(4, 1);
    let snapshot = {
        let mut source = Buffer::new(4, 1);
        let style = Style::new()
            .fg(Color::Green)
            .link("https://example.test", "");
        let cell = source
            .new_cell("e\u{301}", style)
            .map_err(io::Error::other)?;
        source.set_cell(Position::new(0, 0), cell.as_ref());
        source.blit(&mut copy, Position::new(0, 0));
        source.cell(Position::new(0, 0)).unwrap().to_owned()
    };
    copy.set_cell(Position::new(2, 0), snapshot.as_ref());
    copy.set_style(Position::new(2, 0), Style::new().bold());
    let borrowed = copy.cell(Position::new(0, 0)).unwrap();
    writeln!(
        out,
        "Borrowed glyph: {:?}, width={}; its copy and snapshot survive the source.",
        borrowed.content(),
        borrowed.width()
    )?;
    assert_eq!(borrowed.content(), snapshot.content());
    assert_eq!(borrowed.width(), snapshot.width());

    out.flush()
}

fn draw_card(buf: &mut Buffer) {
    let w = buf.width();
    let h = buf.height();

    // Rounded border.
    let border = Style::default().fg(Color::BrightBlack);
    let edge = |s: &str| Cell::new(s, 1).style(border.clone());
    buf.fill_rect(Rect::new(1, 0, w - 2, 1), edge("─").as_ref());
    buf.fill_rect(Rect::new(1, h - 1, w - 2, 1), edge("─").as_ref());
    buf.fill_rect(Rect::new(0, 1, 1, h - 2), edge("│").as_ref());
    buf.fill_rect(Rect::new(w - 1, 1, 1, h - 2), edge("│").as_ref());
    buf.set_cell((0, 0).into(), edge("╭").as_ref());
    buf.set_cell((w - 1, 0).into(), edge("╮").as_ref());
    buf.set_cell((0, h - 1).into(), edge("╰").as_ref());
    buf.set_cell((w - 1, h - 1).into(), edge("╯").as_ref());

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
        let cell = Cell::new("▌", 1).style(Style::default().fg(left).bg(right));
        buf.set_cell((bar.x + i, bar.y).into(), cell.as_ref());
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
