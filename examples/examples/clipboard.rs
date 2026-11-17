//! Count clipboard reply bytes as they arrive, without retaining their contents.
//!
//! Run with `cargo run --example clipboard`. The terminal must allow clipboard
//! reads. This example applies a 30-second caller deadline and reports wire
//! bytes, which normally contain base64 rather than decoded clipboard text.
//! The source recovers after two idle seconds during a reply. Pass
//! `-- --no-idle-timeout` to wait for framing until the caller deadline.

use std::io;
use std::time::{Duration, Instant};

use uncurses::event::Event;
use uncurses::program::Program;

fn main() -> io::Result<()> {
    let mut program = Program::stdio()?;
    let result = (|| -> io::Result<(usize, usize, bool)> {
        program.init()?;
        if std::env::args().any(|arg| arg == "--no-idle-timeout") {
            program
                .event_source()
                .lock()
                .unwrap()
                .set_clipboard_idle_timeout(None);
        }
        program.request_system_clipboard()?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut bytes = 0;
        let mut chunks = 0;
        while let Some(timeout) = deadline.checked_duration_since(Instant::now()) {
            if !program.poll_event(Some(timeout))? {
                continue;
            }
            while let Some(event) = program.try_read_event()? {
                match event {
                    Event::ClipboardStart { .. } => {
                        bytes = 0;
                        chunks = 0;
                    }
                    Event::ClipboardChunk(chunk) => {
                        bytes += chunk.len();
                        chunks += 1;
                    }
                    Event::ClipboardEnd { complete } => return Ok((bytes, chunks, complete)),
                    _ => {}
                }
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "clipboard reply exceeded the caller deadline",
        ))
    })();
    let finish = program.finish();
    let (bytes, chunks, complete) = result?;
    finish?;
    println!("Clipboard: {bytes} wire bytes in {chunks} chunks; complete: {complete}");
    Ok(())
}
