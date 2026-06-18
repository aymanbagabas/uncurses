//! Terminal capability probing with the `query` API, end to end.
//!
//! Terminals answer feature queries (OSC / CSI requests) only when they
//! support them, and stay silent otherwise — so a probe is really two
//! questions: *did the terminal reply?* and, if not, *is it unsupported
//! or merely slow?* This example demonstrates every shape of the query
//! API answering both:
//!
//!   * `query_blocking` — fire a request and park until the reply lands
//!     (or the budget elapses).
//!   * `query` — fire a request and get back a handle you drive yourself
//!     (a sync poll loop, or `.await` on a thread-backed stream).
//!   * threadless [`EventSource`] (you drive the source) vs. thread-backed
//!     [`EventStream`](uncurses::event::EventStream) (a reader thread
//!     drives it for you).
//!   * the **DA1 sentinel** pattern: batch the real query with a Primary
//!     Device Attributes request in a single flush. Every terminal answers
//!     DA1, so if DA1 comes back but the real reply does not, the feature
//!     is *definitively* unsupported rather than just slow.
//!
//! Every reply also stays visible to ordinary [`read`](EventSource::read):
//! a query never swallows input. Run with
//! `cargo run --example capabilities`.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use uncurses::color::Color;
use uncurses::event::source::Input;
use uncurses::event::{EventSource, query};
use uncurses::terminal::Terminal;

/// Per-query budget. Real terminals usually reply within a few
/// milliseconds; the DA1 sentinel removes the need to pick this
/// conservatively.
const BUDGET: Duration = Duration::from_millis(150);

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let mut term = Terminal::open()?;
    term.make_raw()?;
    // Collect results while raw mode rewrites newline handling, then print
    // them once the terminal is restored.
    let report = run(&mut term).await;
    term.restore()?;

    println!("terminal capability probe (background color via OSC 11)\n");
    match report {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

async fn run<I, O>(term: &mut Terminal<I, O>) -> io::Result<Vec<String>>
where
    I: Input + Copy + 'static,
    O: Write + Copy,
{
    let mut out = term.output();
    let mut lines = Vec::new();

    // ---- Threadless EventSource: you own the source and drive it -------
    {
        let mut source = EventSource::new(term.input())?;

        // 1. query_blocking, no sentinel: simplest form. `None` could mean
        //    "unsupported" or "slower than the budget" — we cannot tell.
        let bg = source.query_blocking(&mut out, query::BACKGROUND_COLOR, BUDGET)?;
        lines.push(plain("sync  query_blocking            ", bg));

        // 2. query_blocking, DA1 sentinel: one flush, two replies, each
        //    resolved independently. DA1 disambiguates silence.
        let (bg, da1) = source.query_blocking(
            &mut out,
            (query::BACKGROUND_COLOR, query::PRIMARY_DEVICE_ATTRIBUTES),
            BUDGET,
        )?;
        lines.push(sentinel(
            "sync  query_blocking + DA1      ",
            bg,
            da1.is_some(),
        ));

        // 3. query, no sentinel: `query` stages the request and hands back
        //    a handle that borrows nothing. You drive the source in your
        //    own loop and collect the handle when it resolves.
        let mut handle = source.query(&mut out, query::BACKGROUND_COLOR, BUDGET)?;
        let deadline = Instant::now() + BUDGET;
        while !handle.is_ready() && Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            source.poll(Some(remaining))?;
            // A real loop would dispatch these events; the reply is among
            // them and *also* fills the handle. We discard them here.
            while source.try_read().is_some() {}
        }
        lines.push(plain("sync  query + poll loop         ", handle.try_take()));

        // 4. query, DA1 sentinel: same manual drive over a batch. Each
        //    member fills its own slot; collect them separately.
        let (mut hbg, mut hda1) = source.query(
            &mut out,
            (query::BACKGROUND_COLOR, query::PRIMARY_DEVICE_ATTRIBUTES),
            BUDGET,
        )?;
        let deadline = Instant::now() + BUDGET;
        while !(hbg.is_ready() && hda1.is_ready()) && Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            source.poll(Some(remaining))?;
            while source.try_read().is_some() {}
        }
        let (bg, da1) = (hbg.try_take(), hda1.try_take());
        lines.push(sentinel(
            "sync  query + poll loop + DA1   ",
            bg,
            da1.is_some(),
        ));
    }

    // ---- Thread-backed EventStream: a reader thread drives the source --
    let stream = EventSource::new(term.input())?.into_stream();

    // 5. query_blocking on the stream, no sentinel. Blocks the calling
    //    thread, but the reader thread does the I/O — so other threads
    //    sharing this stream keep working.
    let bg = stream.query_blocking(&mut out, query::BACKGROUND_COLOR, BUDGET)?;
    lines.push(plain("async query_blocking            ", bg));

    // 6. query_blocking on the stream, DA1 sentinel.
    let (bg, da1) = stream.query_blocking(
        &mut out,
        (query::BACKGROUND_COLOR, query::PRIMARY_DEVICE_ATTRIBUTES),
        BUDGET,
    )?;
    lines.push(sentinel(
        "async query_blocking + DA1      ",
        bg,
        da1.is_some(),
    ));

    // 7. query + `.await`: the handle is a Future. Awaiting it parks the
    //    task (not the OS thread) until the reader thread resolves it.
    let bg = stream
        .query(&mut out, query::BACKGROUND_COLOR, BUDGET)?
        .await;
    lines.push(plain("async query + .await            ", bg));

    // 8. query + DA1 sentinel awaited concurrently with `tokio::join!`:
    //    one flush, both futures in flight at once, resolved together.
    let (hbg, hda1) = stream.query(
        &mut out,
        (query::BACKGROUND_COLOR, query::PRIMARY_DEVICE_ATTRIBUTES),
        BUDGET,
    )?;
    let (bg, da1) = tokio::join!(hbg, hda1);
    lines.push(sentinel(
        "async query + join! + DA1       ",
        bg,
        da1.is_some(),
    ));

    // 9. Many independent queries in flight at once: issue several
    //    (each its own flush), then await them all concurrently. This is
    //    the payoff of non-destructive, per-handle replies — every query
    //    resolves on its own as its reply decodes, in any order.
    let fg = stream.query(&mut out, query::FOREGROUND_COLOR, BUDGET)?;
    let bg = stream.query(&mut out, query::BACKGROUND_COLOR, BUDGET)?;
    let cursor = stream.query(&mut out, query::CURSOR_COLOR, BUDGET)?;
    let da1 = stream.query(&mut out, query::PRIMARY_DEVICE_ATTRIBUTES, BUDGET)?;
    let (fg, bg, cursor, da1) = tokio::join!(fg, bg, cursor, da1);
    lines.push(format!(
        "async join! of 4 queries        : fg={} bg={} cursor={} (DA1 sentinel: {})",
        opt(fg),
        opt(bg),
        opt(cursor),
        if da1.is_some() { "answered" } else { "silent" },
    ));

    Ok(lines)
}

/// Render a no-sentinel result: a reply means supported; silence is
/// ambiguous (unsupported or just slow).
fn plain(name: &str, bg: Option<Color>) -> String {
    match bg {
        Some(c) => format!("{name}: supported — {c:?}"),
        None => format!("{name}: no reply within {BUDGET:?} (unsupported or slow — can't tell)"),
    }
}

/// Render a DA1-sentinel result: silence with a DA1 answer is a definitive
/// "unsupported"; silence on both means the terminal is unresponsive.
fn sentinel(name: &str, bg: Option<Color>, da1_answered: bool) -> String {
    match (bg, da1_answered) {
        (Some(c), _) => format!("{name}: supported — {c:?}"),
        (None, true) => format!("{name}: UNSUPPORTED (DA1 sentinel answered, color query did not)"),
        (None, false) => {
            format!("{name}: no reply at all (DA1 sentinel silent too — unresponsive)")
        }
    }
}

fn opt(c: Option<Color>) -> String {
    match c {
        Some(c) => format!("{c:?}"),
        None => "—".to_string(),
    }
}
