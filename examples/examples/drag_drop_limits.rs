//! Simulate terminal requests to demonstrate an outgoing queue limit.

use uncurses::event::Event as TerminalEvent;
use uncurses_dragdrop::{DragDrop, DragDropOptions, Error, Operations};

fn message(fields: &[(&str, &str)], payload: &[u8]) -> TerminalEvent {
    TerminalEvent::DragDrop {
        options: fields
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
        payload: payload.into(),
    }
}

fn main() -> Result<(), Error> {
    let mut controller = DragDrop::new(DragDropOptions {
        max_pending_requests: Some(1),
        ..DragDropOptions::default()
    });
    let mut output = Vec::new();
    controller.enable_drags(&mut output)?;
    controller.handle_event(&message(&[("t", "o")], b""))?;
    controller.offer(&mut output, &["text/plain"], Operations::COPY)?;
    controller.start_drag(&mut output)?;
    controller.handle_event(&message(&[("t", "E")], b"OK"))?;

    let request = message(&[("t", "e"), ("x", "5"), ("y", "0")], b"");
    controller.handle_event(&request)?;
    match controller.handle_event(&request) {
        Err(Error::LimitExceeded(reason)) => {
            println!("Second request rejected: {reason}");
            controller.cancel_drag(&mut output)?;
            println!("Drag canceled; the controller remains available.");
        }
        Err(error) => return Err(error),
        Ok(_) => return Err(Error::InvalidState("the second request exceeded the limit")),
    }
    Ok(())
}
