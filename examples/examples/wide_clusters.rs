//! Wide clusters, and how a frame keeps the rest of the row in place.
//!
//! A terminal decides for itself how many columns a joined emoji takes. One
//! draws every face in the sequence; another ligates the whole thing into a
//! single double-wide glyph. The grid had to pick a number when the cell was
//! written, and a terminal that counts the cluster differently puts
//! everything after it on that line out of place.
//!
//! [`Screen`] gives up on the diff at such a cluster and repaints the row
//! from there to its right edge. One absolute move reaches the cluster, and
//! everything after it is written in a single run, so the terminal lays the
//! rest of the row out on its own terms and the disagreement never leaves
//! that row.
//!
//! The columns left of the cluster are measured the way the terminal draws
//! them, so they keep the ordinary diff. Only the tail is given up.
//!
//! This example renders into a `Vec<u8>` instead of a terminal so the bytes
//! can be printed and read. Run with `cargo run --example wide_clusters`.

use std::io::{self, Write};

use uncurses::cell::Cell;
use uncurses::screen::Screen;
use uncurses::text::WidthMode;

/// A four-person family: seven code points, four faces joined by three
/// zero-width joiners.
const FAMILY: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";

/// A CJK ideograph. One code point, and every terminal gives it two columns.
const IDEOGRAPH: &str = "\u{4e16}";

/// A heart followed by an emoji variation selector. Summing its code
/// points gives one column, and a terminal that honors the selector draws
/// it in emoji presentation across two. The sum can fall short of what the
/// terminal does as easily as it can overshoot.
const HEART: &str = "\u{2764}\u{fe0f}";

const COLS: u16 = 24;

fn main() -> io::Result<()> {
    let mut out = io::stdout().lock();

    for (label, text) in [
        ("family emoji", FAMILY),
        ("CJK ideograph", IDEOGRAPH),
        ("heart and variation selector", HEART),
    ] {
        let width = WidthMode::Wc.grapheme_width(text, false);
        let mut screen = row_screen(false)?;
        let bytes = place(&mut screen, text, width)?;
        let plural = if width == 1 { "column" } else { "columns" };
        writeln!(out, "{label} {text} claims {width} {plural}")?;
        writeln!(out, "  {}", printable(&bytes))?;
    }

    // Measuring whole clusters means the terminal answered DEC mode 2027 and
    // counts them the same way the grid does, so nothing is in doubt.
    let mut screen = row_screen(true)?;
    let bytes = place(
        &mut screen,
        FAMILY,
        WidthMode::Grapheme.grapheme_width(FAMILY, false),
    )?;
    writeln!(out, "\nthe same family emoji, once the terminal has agreed")?;
    writeln!(out, "  {}", printable(&bytes))?;

    // The cluster does not change here, and that changes nothing: a
    // terminal that ligated it put the `Z` four columns in, not ten, so
    // the row is laid out again from the cluster.
    let mut screen = row_screen(false)?;
    let width = WidthMode::Wc.grapheme_width(FAMILY, false);
    let _ = place(&mut screen, FAMILY, width)?;
    screen.writer_mut().clear();
    screen.set_cell((2 + u16::from(width), 0), &Cell::new("Y", 1));
    screen.render()?;
    let bytes = screen.into_writer();
    writeln!(out, "\nchanging only the cell after the family emoji")?;
    writeln!(out, "  {}", printable(&bytes))?;

    writeln!(
        out,
        "\nThe family emoji and the heart are both repainted from the cluster:\n\
         one claims more columns than a terminal may give it, the other less.\n\
         The ideograph is a single code point, so no terminal can disagree\n\
         about it, and the agreed row was measured the way the terminal\n\
         measures it. Both of those keep the ordinary diff."
    )?;
    out.flush()
}

/// A one-row screen with its opening blank frame already written and
/// discarded, so later frames show only the work being demonstrated.
fn row_screen(whole_clusters: bool) -> io::Result<Screen<Vec<u8>>> {
    let mut screen = Screen::new(Vec::new(), (COLS, 1));
    screen.set_grapheme_clusters(whole_clusters);
    screen.render()?;
    screen.writer_mut().clear();
    Ok(screen)
}

/// Put `text` at column 2 followed by a `Z`, and return that frame's bytes.
fn place(screen: &mut Screen<Vec<u8>>, text: &str, width: u8) -> io::Result<Vec<u8>> {
    screen.set_cell((2, 0), &Cell::new(text, width));
    // A wide cell owns the columns after it. They hold continuations, which
    // are never drawn: the primary cell covers them.
    for x in 3..2 + u16::from(width) {
        screen.set_cell((x, 0), &Cell::new("", 0));
    }
    screen.set_cell((2 + u16::from(width), 0), &Cell::new("Z", 1));
    screen.render()?;
    Ok(std::mem::take(screen.writer_mut()))
}

/// Spell the control bytes in a frame so they can be read on a page.
fn printable(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .map(|c| match c {
            '\x1b' => "ESC ".to_string(),
            '\r' => "CR ".to_string(),
            '\n' => "LF ".to_string(),
            ' ' => "·".to_string(),
            c => c.to_string(),
        })
        .collect()
}
