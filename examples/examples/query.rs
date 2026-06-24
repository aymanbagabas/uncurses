//! Query the terminal and print its answers.
//!
//! Demonstrates the request/reply model: write request bytes from the
//! `ansi` module to the terminal output, then read the matching reply
//! [`Event`] values back through an `EventSource`. The Primary Device
//! Attributes reply is conventionally sent last, so it marks the end of
//! the answers.
//!
//! Run with `cargo run --example query`. A terminal that does not support
//! a given query simply never answers it, so the program gives up after a
//! short deadline.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use uncurses::ansi::color::REQUEST_BACKGROUND_COLOR;
use uncurses::ansi::ctrl::REQUEST_PRIMARY_DA;
use uncurses::ansi::cursor::write_request_cursor_position;
use uncurses::ansi::winop::REQUEST_CELL_PIXEL_SIZE;
use uncurses::event::{Event, EventSource};
use uncurses::terminal::Terminal;

fn main() -> io::Result<()> {
    // Raw mode so the replies arrive as bytes we can read, not echoed text.
    let mut term = Terminal::stdio();
    term.make_raw()?;
    let mut out = term.output();
    let mut events = EventSource::new(term.input())?;

    // Fire the requests. Primary DA goes last: its reply terminates the
    // batch, so seeing it means every earlier answer has already arrived.
    out.write_all(REQUEST_BACKGROUND_COLOR)?;
    write_request_cursor_position(&mut out)?;
    out.write_all(REQUEST_CELL_PIXEL_SIZE)?;
    out.write_all(REQUEST_PRIMARY_DA)?;
    out.flush()?;

    let mut lines = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(300);
    'wait: loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || !events.poll(Some(remaining))? {
            break;
        }
        while let Some(ev) = events.try_read() {
            match ev {
                Event::BackgroundColor(c) => lines.push(format!("background color: {c:?}")),
                Event::CursorPosition(p) => {
                    lines.push(format!("cursor position: column {}, row {}", p.x, p.y))
                }
                Event::CellPixelSize { width, height } => {
                    lines.push(format!("cell size: {width}x{height} pixels"))
                }
                Event::PrimaryDeviceAttributes(attrs) => {
                    lines.push(format!("device attributes: {attrs:?}"));
                    break 'wait; // the terminating reply: we are done
                }
                // A real app would handle keys, resizes, and so on here; the
                // reply events are just more variants in the same stream.
                _ => {}
            }
        }
    }

    term.restore()?;

    if lines.is_empty() {
        println!("no replies (the terminal may not support these queries)");
    } else {
        for line in lines {
            println!("{line}");
        }
    }
    Ok(())
}
