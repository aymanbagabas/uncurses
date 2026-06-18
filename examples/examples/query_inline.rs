//! Querying a terminal by hand, inside your own event loop.
//!
//! The `query` and `query_blocking` helpers (see the `capabilities`
//! example) do the bookkeeping for you: they register the reply matcher,
//! track a deadline, and hand back a typed answer. This example shows the
//! other approach, where *you* hold the loop:
//!
//!   1. Write the request bytes straight to the output yourself.
//!   2. Watch for the reply inside your ordinary event loop. A `Query`
//!      carries its own matcher, so a single match arm calls
//!      [`matches`](uncurses::event::query::Single::matches) and decodes
//!      the answer when it rides in.
//!
//! The reply arrives as just another event, so keystrokes and resizes keep
//! flowing while you wait, and the whole thing is one extra branch in a
//! loop you already run. A query never swallows input either way; the only
//! difference is who holds the loop.
//!
//! This version uses a blocking `read`, so it waits forever and never gives
//! up on a silent terminal. If you want a deadline, you can add one by
//! hand: track an `Instant` and swap `read` for
//! [`poll`](uncurses::event::EventSource::poll) with the remaining budget,
//! breaking when it runs out. That bookkeeping is yours to get right, and
//! it is per query, so a shared budget across several probes turns into a
//! small state machine.
//!
//! The same inline match works against an `EventStream` too: its reader
//! thread blocks on the terminal and fills a shared queue, while its `read`
//! and `try_read` pop from it, so nothing here changes. The `query` /
//! `query_blocking` helpers add the deadline, a typed result, and batching;
//! under async they also hand back a `Future` that resolves through the
//! observer registry, which you can await or race against a timeout without
//! folding reply detection into your stream loop.
//!
//! Run with `cargo run --example query_inline`. Press `q` or Ctrl-C to
//! quit.

use std::io::{self, Write};

use uncurses::event::{Event, EventSource, Key, KeyCode, KeyModifiers, query};
use uncurses::terminal::Terminal;

fn main() -> io::Result<()> {
    let mut term = Terminal::open()?;
    term.make_raw()?;
    let mut out = term.output();
    let mut source = EventSource::new(term.input())?;

    // Fire the request by hand. The bytes go straight to the output, and
    // the reply will come back through the same terminal as an ordinary
    // event. No helper is registered to catch it; the loop below does.
    out.write_all(query::BACKGROUND_COLOR.request())?;
    out.flush()?;

    report(
        &mut out,
        "asked for the background color; type away...".into(),
    )?;

    loop {
        let ev = source.read()?;
        match ev {
            // The reply is just another event. The query bundles its own
            // matcher, so this arm both recognises and decodes it, and
            // stays in sync with the request we sent above.
            ref ev if query::BACKGROUND_COLOR.matches(ev).is_some() => {
                let color = query::BACKGROUND_COLOR.matches(ev).unwrap();
                report(&mut out, format!("background color: {color:?}"))?;
            }
            Event::KeyPress(Key {
                code: KeyCode::Char('q'),
                modifiers,
                ..
            }) if modifiers.is_empty() => break,
            Event::KeyPress(Key {
                code: KeyCode::Char('c'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CTRL) => break,
            other => report(&mut out, format!("event: {other:?}"))?,
        }
    }

    term.restore()?;
    Ok(())
}

/// Print one line in raw mode, where we must emit the carriage return
/// ourselves because the terminal will not translate `\n` for us.
fn report<W: Write>(out: &mut W, line: String) -> io::Result<()> {
    write!(out, "{line}\r\n")?;
    out.flush()
}
