//! Truly async terminal input via [`EventStream`], no [`Screen`] in sight.
//!
//! This is the low-level async path: build an [`EventStream`] over stdin and
//! await events on a tokio runtime, concurrently with a timer via
//! `tokio::select!`. `poll_next` never blocks the reactor, a helper thread
//! does the blocking readiness wait and wakes the task, so the stream is
//! genuinely async on any executor. We skip `Screen` entirely and just print
//! each decoded event (and each timer tick) to stderr.
//!
//! Raw mode is handled directly with [`Terminal::make_raw`] / [`restore`] so
//! keys arrive unbuffered and `Ctrl-C` reaches us as an event instead of
//! killing the process.
//!
//! Requires the `async` feature (on by default for the examples crate):
//! `cargo run --example async_input`. Press `q`, `Esc`, or `Ctrl-C` to quit.
//!
//! [`restore`]: uncurses::terminal::Terminal::restore

use tokio_stream::StreamExt;

use uncurses::event::{Event, EventSource, Key, KeyCode, KeyModifiers};
use uncurses::terminal::{Stdin, Terminal};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut term = Terminal::stdio();
    term.make_raw()?;

    // Raw mode is on; make sure we always put the terminal back.
    let result = run(term.input()).await;

    term.restore()?;
    result
}

async fn run(input: Stdin) -> std::io::Result<()> {
    let mut stream = EventSource::new(input)?.into_stream();
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
    let mut ticks = 0u64;
    eprint!("async input via EventStream + a 1s timer. press q, Esc, or Ctrl-C to quit.\r\n");

    loop {
        tokio::select! {
            // Branch 1: the terminal. `next()` is truly async, no reactor block.
            maybe = stream.next() => {
                let Some(ev) = maybe else { break };
                let ev = ev?;
                eprint!("event: {ev:?}\r\n");
                if let Event::KeyPress(ref k) = ev
                    && is_quit(k)
                {
                    break;
                }
            }
            // Branch 2: a timer, ticking concurrently with input.
            _ = ticker.tick() => {
                ticks += 1;
                eprint!("tick #{ticks}\r\n");
            }
        }
    }
    Ok(())
}

fn is_quit(k: &Key) -> bool {
    matches!(k.code, KeyCode::Char('q') | KeyCode::Escape)
        || (matches!(k.code, KeyCode::Char('c')) && k.modifiers.contains(KeyModifiers::CTRL))
}
