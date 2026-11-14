//! Simulate recoverable source errors and fatal output errors without a terminal.

use std::io::{self, Read, Write};
use uncurses::event::Event as TerminalEvent;
use uncurses_dragdrop::{DragDrop, Error, Operations, ThumbnailFormat};

struct BrokenSource {
    send_chunk: bool,
}

impl Read for BrokenSource {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self.send_chunk {
            self.send_chunk = false;
            bytes.fill(b'x');
            Ok(bytes.len())
        } else {
            Err(io::Error::other("source read failed"))
        }
    }
}

struct BrokenOutput;

impl Write for BrokenOutput {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "terminal write failed",
        ))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn requested() -> Result<DragDrop, Error> {
    let mut controller = DragDrop::default();
    controller.enable_drags(&mut Vec::new())?;
    controller.handle_event(&TerminalEvent::DragDrop {
        options: vec![("t".into(), "o".into())],
        payload: Vec::new(),
    })?;
    Ok(controller)
}

fn main() -> Result<(), Error> {
    let mut controller = requested()?;
    let mut output = Vec::new();
    controller.offer(&mut output, &["text/plain"], Operations::COPY)?;
    match controller.send_from(
        &mut output,
        "text/plain",
        &mut BrokenSource { send_chunk: true },
    ) {
        Err(Error::Io(error)) => {
            println!("Recoverable source error: {error}");
            controller.cancel_drag(&mut output)?;
            controller.close(&mut output)?;
            println!("Canceled the transfer and closed the controller.");
        }
        Err(error) => return Err(error),
        Ok(()) => return Err(Error::InvalidState("expected a source error")),
    }

    let mut controller = requested()?;
    match controller.offer(&mut BrokenOutput, &["text/plain"], Operations::COPY) {
        Err(Error::OutputFailed(cause)) => {
            println!("Fatal offer error: {cause}");
            drop(controller);
            println!("Discarded the controller without further protocol output.");
        }
        Err(error) => return Err(error),
        Ok(()) => return Err(Error::InvalidState("expected a terminal write error")),
    }

    let mut controller = requested()?;
    controller.offer(&mut output, &["text/plain"], Operations::COPY)?;
    match controller.add_thumbnail_from(
        &mut output,
        ThumbnailFormat::Png {
            width: 1,
            height: 1,
        },
        &mut BrokenSource { send_chunk: true },
    ) {
        Err(Error::OutputFailed(cause)) => {
            println!("Fatal thumbnail error: {cause}");
            drop(controller);
            println!("Discarded the controller without a cancellation command.");
        }
        Err(error) => return Err(error),
        Ok(_) => return Err(Error::InvalidState("expected a thumbnail source error")),
    }
    Ok(())
}
