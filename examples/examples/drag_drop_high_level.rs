//! Receive text copies and offer text drags with uncurses-dragdrop.
//!
//! Run with `cargo run -p examples --example drag_drop_high_level` in Kitty 0.47
//! or newer. Drop text onto the window, or use Kitty's drag gesture to copy
//! text out. Press q or Ctrl-C to quit.

use std::io::{self, Cursor, Write};

use uncurses::ansi::drag_drop::REQUEST_DRAG_DROP;
use uncurses::buffer::SurfaceMut;
use uncurses::event::{Event as TerminalEvent, Key};
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;
use uncurses_dragdrop::{
    DragDrop, DragDropOptions, DropOffer, Error, Event, Operation, Operations,
};

const MIME: &str = "text/plain";
const TEXT: &str = "Hello from uncurses!\n";
const PREVIEW_LIMIT: usize = 256;

#[derive(Default)]
struct Demo {
    received: usize,
    preview: String,
    status: String,
}

fn copy_type(offer: &DropOffer) -> Option<&str> {
    if !offer.operations.contains(Operations::COPY) {
        return None;
    }
    offer
        .mime_types
        .iter()
        .find(|mime| matches!(mime.as_str(), MIME | "text/plain;charset=utf-8"))
        .map(String::as_str)
}

impl Demo {
    fn receive(&mut self, data: &[u8]) {
        self.received += data.len();
        for byte in data {
            let escaped = byte.escape_ascii();
            if self.preview.len() + escaped.len() > PREVIEW_LIMIT {
                break;
            }
            self.preview.extend(escaped.map(char::from));
        }
        self.status = format!("Received {} bytes", self.received);
    }

    fn event(
        &mut self,
        controller: &mut DragDrop,
        out: &mut impl Write,
        event: Event,
    ) -> Result<(), Error> {
        match event {
            Event::Hover(offer) => {
                if let Some(mime) = copy_type(&offer) {
                    controller.accept(out, &[mime], Operation::Copy)?;
                    self.status = "Copy accepted; release to drop text".into();
                } else {
                    controller.reject(out)?;
                    self.status = "Drop rejected: plain text copy is unavailable".into();
                }
            }
            Event::Leave => self.status = "Drag left the window".into(),
            Event::Drop(offer) => {
                if let Some(mime) = copy_type(&offer) {
                    self.received = 0;
                    self.preview.clear();
                    controller.request(out, mime)?;
                    self.status = "Requested dropped text".into();
                } else {
                    controller.cancel_drop(out)?;
                    self.status = "Drop canceled: plain text copy is unavailable".into();
                }
            }
            Event::Data(data) => self.receive(&data),
            Event::DataEnd => {
                controller.complete_drop(out, Operation::Copy)?;
                self.status = format!("Received {} bytes; copy complete", self.received);
            }
            Event::DropFailed(error) => self.status = format!("Drop error: {error}"),
            Event::DragRequested(_) => {
                controller.offer(out, &[MIME], Operations::COPY)?;
                controller.send(out, MIME, TEXT.repeat(512).as_bytes())?;
                controller.start_drag(out)?;
                self.status = "Drag offered with text".into();
            }
            Event::DragStarted => self.status = "Drag started".into(),
            Event::DragAccepted { mime_type } => {
                self.status = format!("Drag accepted: {mime_type}");
            }
            Event::DragActionChanged { operation } => {
                self.status = format!("Drag action: {operation:?}");
            }
            Event::DragDropped => self.status = "Text dropped; waiting for completion".into(),
            Event::DataRequested { mime_type } => {
                let mut text = Cursor::new(TEXT.repeat(512));
                controller.send_from(out, &mime_type, &mut text)?;
            }
            Event::DragFinished { canceled } => {
                self.status = if canceled {
                    "Drag canceled"
                } else {
                    "Drag complete"
                }
                .into();
            }
            Event::DragFailed(error) => self.status = format!("Drag error: {error}"),
            _ => {}
        }
        Ok(())
    }
}

fn run(
    program: &mut Program<Stdin, Stdout>,
    controller: &mut DragDrop,
    demo: &mut Demo,
) -> Result<(), Box<dyn std::error::Error>> {
    program.init()?;
    program.enter_alt_screen()?;
    program.hide_cursor()?;
    program.query_capabilities(REQUEST_DRAG_DROP)?;
    let mut awaiting_capabilities = true;
    demo.status = "Checking drag and drop support".into();
    let quit: [Key; 2] = ["q", "ctrl+c"].map(|key| key.parse().unwrap());
    loop {
        let screen = program.screen_mut();
        screen.clear();
        screen.set_str(
            (0, 0),
            "DragDrop: drop text here, or drag text out. q quits.",
            Style::new(),
        );
        screen.set_str((0, 2), &demo.status, Style::new());
        screen.set_str((0, 4), &demo.preview, Style::new());
        screen.render()?;
        let event = program.read_event()?;
        match &event {
            TerminalEvent::KeyPress(key) if quit.contains(key) => return Ok(()),
            TerminalEvent::Resize(_) => program.autoresize()?,
            TerminalEvent::PrimaryDeviceAttributes(_) if awaiting_capabilities => {
                if !program.capabilities().drag_drop() {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "OSC 72 support was not confirmed",
                    )
                    .into());
                }
                controller
                    .enable_drops(program.screen_mut(), &[MIME, "text/plain;charset=utf-8"])?;
                controller.enable_drags(program.screen_mut())?;
                awaiting_capabilities = false;
                demo.status = "Ready for text drops and drag gestures".into();
            }
            _ => {}
        }
        for event in controller.handle_event(&event)? {
            demo.event(controller, program.screen_mut(), event)?;
        }
        program.screen_mut().flush()?;
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut program = Program::stdio()?;
    let mut controller = DragDrop::new(DragDropOptions::default());
    let mut demo = Demo::default();
    let result = run(&mut program, &mut controller, &mut demo);
    let cleanup = controller.close(program.screen_mut()).map_err(Into::into);
    let flush = program.screen_mut().flush().map_err(Into::into);
    let finish = program.finish().map_err(Into::into);
    result.and(cleanup).and(flush).and(finish)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_stays_bounded_and_counts_all_bytes() {
        let mut demo = Demo::default();
        demo.receive(&[b'a'; PREVIEW_LIMIT - 1]);
        demo.receive(&[0xff; 1024]);
        assert_eq!(demo.preview, "a".repeat(PREVIEW_LIMIT - 1));
        assert_eq!(demo.received, PREVIEW_LIMIT - 1 + 1024);
    }
}
