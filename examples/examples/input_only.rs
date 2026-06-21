//! Read input only: decode events and print them, no rendering.
//!
//! The smallest possible use of the input layer. There is no `Screen` and
//! no renderer here, just a [`Terminal`] in raw mode and an
//! [`EventSource`] turning raw bytes into typed [`Event`] values. Every
//! event is printed as it arrives, so it doubles as a "what does this key
//! send?" probe.
//!
//! Run with `cargo run --example input_only`. Press keys, paste text, or
//! resize the window to see events; press `q` or `Ctrl-C` to quit.

use std::io::{self, Write};

use uncurses::event::{Event, EventSource, Key, KeyCode, KeyModifiers};
use uncurses::terminal::Terminal;

fn main() -> io::Result<()> {
    // Raw mode delivers keystrokes byte-by-byte instead of line-buffered
    // and echoed, which is exactly what the decoder wants to read.
    let mut term = Terminal::stdio();
    term.make_raw()?;

    let mut events = EventSource::new(term.input())?;
    let mut out = term.output();

    // In raw mode the terminal does not translate `\n` into a carriage
    // return, so every line ends with `\r\n`.
    write!(out, "Reading input. Press q or Ctrl-C to quit.\r\n")?;
    out.flush()?;

    let result = read_loop(&mut events, &mut out);

    // No modes were turned on, so teardown is just dropping raw mode.
    term.restore()?;
    result
}

fn read_loop(
    events: &mut EventSource<impl uncurses::event::Input>,
    out: &mut impl Write,
) -> io::Result<()> {
    loop {
        // `read` blocks until the next decoded event (or a partial escape
        // sequence resolves on its own short timeout).
        let event = events.read()?;

        if is_quit(&event) {
            break;
        }

        // Pretty-print the event. `Event` is a plain enum, so a real app
        // would `match` on the variant it cares about; here we just show
        // the `Debug` form for everything.
        write!(out, "{event:?}\r\n")?;
        out.flush()?;
    }
    Ok(())
}

fn is_quit(event: &Event) -> bool {
    matches!(
        event,
        Event::KeyPress(Key { code: KeyCode::Char('q'), modifiers, .. }) if modifiers.is_empty()
    ) || matches!(
        event,
        Event::KeyPress(Key { code: KeyCode::Char('c'), modifiers, .. }) if modifiers.contains(KeyModifiers::CTRL)
    )
}
