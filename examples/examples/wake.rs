//! Wake a blocked reader from a worker thread.
//!
//! Run with `cargo run -p examples --example wake`. Type or resize while the
//! worker runs. The loop checks the worker request after each read, including
//! events that arrive alongside a wake.

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use uncurses::event::{EventSource, Input, SourceFlags};
use uncurses::terminal::Terminal;

fn main() -> io::Result<()> {
    let mut terminal = Terminal::stdio();
    terminal.make_raw()?;
    let result = (|| {
        let mut source = EventSource::new_with_flags(terminal.input(), SourceFlags::VT_INPUT)?;
        run(&mut source, &mut terminal.output())
    })();
    let restore = terminal.restore();
    result.and(restore)
}

fn run(source: &mut EventSource<impl Input>, out: &mut impl Write) -> io::Result<()> {
    write!(
        out,
        "Type or resize. The worker will request completion in one second.\r\n"
    )?;
    out.flush()?;
    let requested = Arc::new(AtomicBool::new(false));
    let worker_requested = Arc::clone(&requested);
    let waker = source.waker();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        worker_requested.store(true, Ordering::Release);
        waker.wake()
    });

    let result = read_loop(source, out, &requested);
    let worker_result = worker
        .join()
        .map_err(|_| io::Error::other("wake worker panicked"))
        .and_then(|result| result);
    result.and(worker_result)
}

fn read_loop(
    source: &mut EventSource<impl Input>,
    out: &mut impl Write,
    requested: &AtomicBool,
) -> io::Result<()> {
    loop {
        match source.read() {
            Ok(event) => write!(out, "Event: {event:?}\r\n")?,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                write!(out, "The wake interrupted the wait.\r\n")?;
            }
            Err(error) => return Err(error),
        }
        // Ready events take priority over the wake interruption.
        if requested.load(Ordering::Acquire) {
            write!(out, "Worker request handled.\r\n")?;
            return out.flush();
        }
        out.flush()?;
    }
}
