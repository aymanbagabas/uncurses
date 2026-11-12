//! Receive text drops and offer text drags through Kitty OSC 72.
//!
//! Run with `cargo run -p examples --example drag_drop` in Kitty 0.47 or newer.
//! Drop text onto the window, or use Kitty's drag gesture to copy text out.
//! Press q or Ctrl-C to quit. The example accepts copies and keeps only a
//! short escaped preview. It performs no filesystem operations.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use uncurses::ansi::drag_drop::{REQUEST_DRAG_DROP, write_drag_drop};
use uncurses::buffer::SurfaceMut;
use uncurses::event::{Event, Key};
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

const MIME: &str = "text/plain";
const TEXT: &str = "Hello from uncurses!\n";
const BASE64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

#[derive(Default)]
struct Demo {
    registered: bool,
    dropping: bool,
    dragging: bool,
    offered: Vec<String>,
    request: String,
    continuation: Option<Vec<(String, String)>>,
    control: Vec<u8>,
    base64: Vec<u8>,
    received: usize,
    preview: String,
    status: String,
}

fn value<'a>(options: &'a [(String, String)], key: &str) -> Option<&'a str> {
    options
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn invalid(message: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn send_data(out: &mut impl Write, options: &[&str], data: &[u8]) -> io::Result<()> {
    // Optional padding can break thumbnail completion in older Kitty versions.
    let encoded = BASE64.encode(data);
    for (index, chunk) in encoded.as_bytes().chunks(4096).enumerate() {
        let mut metadata = if index == 0 {
            options.to_vec()
        } else {
            Vec::new()
        };
        metadata.push("m=1");
        write_drag_drop(out, &metadata, chunk)?;
    }
    write_drag_drop(out, options, b"")
}

impl Demo {
    fn message(
        &mut self,
        out: &mut impl Write,
        options: Vec<(String, String)>,
        payload: Vec<u8>,
    ) -> io::Result<()> {
        if value(&options, "t") == Some("q") {
            return Ok(());
        }
        let more = value(&options, "m") == Some("1");
        let metadata = self.continuation.take().unwrap_or(options);
        if more {
            self.continuation = Some(metadata.clone());
        }
        if value(&metadata, "t") == Some("r") {
            if !self.dropping || value(&metadata, "x") != Some(self.request.as_str()) {
                return Err(invalid("unexpected drop data"));
            }
            self.base64.extend_from_slice(&payload);
            let count = if more {
                self.base64.len() / 4 * 4
            } else {
                self.base64.len()
            };
            let decoded = BASE64.decode(&self.base64[..count]).map_err(invalid)?;
            self.base64.drain(..count);
            self.received += decoded.len();
            for byte in decoded
                .iter()
                .take(256_usize.saturating_sub(self.preview.len()))
            {
                self.preview.extend(byte.escape_ascii().map(char::from));
            }
            self.status = format!("Received {} bytes", self.received);
            if payload.is_empty() && !more {
                write_drag_drop(out, &["t=r", "o=1"], b"")?;
                self.dropping = false;
                self.status.push_str("; copy complete");
            }
            return Ok(());
        }

        // Bound control-message assembly; file data above stays streamed.
        if self.control.len() + payload.len() > 64 * 1024 {
            return Err(invalid(
                "control message exceeds the example's 64 KiB limit",
            ));
        }
        self.control.extend(payload);
        if more {
            return Ok(());
        }
        let payload = std::mem::take(&mut self.control);
        match value(&metadata, "t") {
            Some("m") if value(&metadata, "x") == Some("-1") => {
                self.offered.clear();
                self.status = "Drag left the window".into();
            }
            Some("m" | "M") => {
                let dropped = value(&metadata, "t") == Some("M");
                if dropped || !payload.is_empty() {
                    self.offered = std::str::from_utf8(&payload)
                        .map_err(invalid)?
                        .split_ascii_whitespace()
                        .map(str::to_owned)
                        .collect();
                }
                let selected = self
                    .offered
                    .iter()
                    .position(|mime| mime == MIME || mime == "text/plain;charset=utf-8")
                    .filter(|_| matches!(value(&metadata, "o"), Some("1" | "3")));
                if dropped {
                    if let Some(index) = selected {
                        self.dropping = true;
                        self.request = (index + 1).to_string();
                        self.received = 0;
                        self.preview.clear();
                        self.base64.clear();
                        write_drag_drop(out, &["t=r", &format!("x={}", self.request)], b"")?;
                        self.status = "Requested dropped text".into();
                    } else {
                        write_drag_drop(out, &["t=r", "o=0"], b"")?;
                        self.status = "Drop canceled: plain text is unavailable".into();
                    }
                } else if let Some(index) = selected {
                    write_drag_drop(out, &["t=m", "o=1"], self.offered[index].as_bytes())?;
                    self.status = "Copy accepted; release to drop text".into();
                } else {
                    write_drag_drop(out, &["t=m", "o=0"], b"")?;
                    self.status = "Drop rejected: plain text copy is unavailable".into();
                }
            }
            Some("o") => {
                self.dragging = true;
                write_drag_drop(out, &["t=o", "o=1"], MIME.as_bytes())?;
                send_data(out, &["t=p", "x=0"], TEXT.repeat(512).as_bytes())?;
                let thumbnail = [0x40, 0x80, 0xff, 0xff].repeat(16 * 16);
                send_data(out, &["t=p", "x=-1", "y=32", "X=16", "Y=16"], &thumbnail)?;
                write_drag_drop(out, &["t=P", "x=-1"], b"")?;
                self.status = "Drag offered with text and a thumbnail".into();
            }
            Some("e") if value(&metadata, "x") == Some("5") => {
                let index: u32 = value(&metadata, "y")
                    .unwrap_or("0")
                    .parse()
                    .map_err(invalid)?;
                let index_option = format!("y={index}");
                if index == 0 {
                    send_data(out, &["t=e", &index_option], TEXT.repeat(512).as_bytes())?;
                } else {
                    write_drag_drop(out, &["t=E", &index_option], b"ENOENT")?;
                }
            }
            Some("e") if value(&metadata, "x") == Some("4") => {
                self.dragging = false;
                self.status = if value(&metadata, "y") == Some("1") {
                    "Drag canceled"
                } else {
                    "Drag complete"
                }
                .into();
            }
            Some("E") => {
                if payload == b"OK" {
                    self.status = "Drag started".into();
                } else {
                    self.dragging = false;
                    self.status = format!("Drag error: {}", payload.escape_ascii());
                }
            }
            Some("R") => {
                self.dropping = false;
                self.status = format!("Drop error: {}", payload.escape_ascii());
            }
            _ => {}
        }
        Ok(())
    }

    fn cleanup(&mut self, out: &mut impl Write) -> io::Result<()> {
        if !self.registered {
            return Ok(());
        }
        let drop = if self.dropping {
            write_drag_drop(out, &["t=r", "o=0"], b"")
        } else {
            Ok(())
        };
        let drag = if self.dragging {
            write_drag_drop(out, &["t=E", "y=-1"], b"")
        } else {
            Ok(())
        };
        let receiver = write_drag_drop(out, &["t=A"], b"");
        let sender = write_drag_drop(out, &["t=o", "x=2"], b"");
        let flush = out.flush();
        drop.and(drag).and(receiver).and(sender).and(flush)
    }
}

fn run(program: &mut Program<Stdin, Stdout>, demo: &mut Demo) -> io::Result<()> {
    program.init()?;
    program.query_capabilities(REQUEST_DRAG_DROP)?;
    let deadline = Instant::now() + Duration::from_secs(1);
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        if !program.poll_event(Some(remaining))? {
            break;
        }
        if matches!(program.read_event()?, Event::PrimaryDeviceAttributes(_)) {
            break;
        }
    }
    if !program.capabilities().drag_drop() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "OSC 72 support was not confirmed",
        ));
    }
    program.enter_alt_screen()?;
    program.hide_cursor()?;
    demo.registered = true;
    write_drag_drop(program.screen_mut(), &["t=a"], MIME.as_bytes())?;
    write_drag_drop(program.screen_mut(), &["t=o", "x=1"], b"")?;
    program.screen_mut().flush()?;
    demo.status = "Ready for text drops and drag gestures".into();
    let quit: [Key; 2] = ["q", "ctrl+c"].map(|key| key.parse().unwrap());
    loop {
        let screen = program.screen_mut();
        screen.clear();
        screen.set_str(
            (0, 0),
            "OSC 72: drop text here, or drag text out. q quits.",
            Style::new(),
        );
        screen.set_str((0, 2), &demo.status, Style::new());
        screen.set_str((0, 4), &demo.preview, Style::new());
        screen.render()?;
        match program.read_event()? {
            Event::KeyPress(key) if quit.contains(&key) => return Ok(()),
            Event::Resize(_) => {
                program.autoresize()?;
            }
            Event::DragDrop { options, payload } => {
                demo.message(program.screen_mut(), options, payload)?;
                program.screen_mut().flush()?;
            }
            Event::UnknownOsc(bytes) if bytes.starts_with(b"72;") => {
                return Err(invalid("malformed or incomplete OSC 72 message"));
            }
            _ => {}
        }
    }
}

fn main() -> io::Result<()> {
    let mut program = Program::stdio()?;
    let mut demo = Demo::default();
    let result = run(&mut program, &mut demo);
    let cleanup = demo.cleanup(program.screen_mut());
    let finish = program.finish();
    result.and(cleanup).and(finish)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(demo: &mut Demo, out: &mut Vec<u8>, metadata: &[(&str, &str)], payload: &[u8]) {
        demo.message(
            out,
            metadata
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
            payload.to_vec(),
        )
        .unwrap();
    }

    #[test]
    fn chunks_inherit_metadata_and_finish_only_on_empty_data() {
        let mut demo = Demo {
            dropping: true,
            request: "2".into(),
            ..Demo::default()
        };
        let mut out = Vec::new();
        message(
            &mut demo,
            &mut out,
            &[("t", "r"), ("x", "2"), ("m", "1")],
            b"YW",
        );
        message(&mut demo, &mut out, &[("t", "q")], b"");
        message(&mut demo, &mut out, &[("m", "0")], b"JjZA");
        assert_eq!(demo.preview, "abcd");
        assert!(demo.dropping);
        assert!(out.is_empty());
        message(&mut demo, &mut out, &[("t", "r"), ("x", "2")], b"");
        assert!(!demo.dropping);
        assert_eq!(demo.received, 4);
        assert_eq!(out, b"\x1b]72;t=r:o=1\x1b\\");
    }

    #[test]
    fn omitted_mimes_retain_the_offer_but_move_only_drops_are_rejected() {
        let mut demo = Demo::default();
        let mut out = Vec::new();
        message(
            &mut demo,
            &mut out,
            &[("t", "m"), ("o", "1")],
            b"text/plain",
        );
        out.clear();
        message(&mut demo, &mut out, &[("t", "m"), ("o", "1")], b"");
        assert_eq!(out, b"\x1b]72;t=m:o=1;text/plain\x1b\\");
        out.clear();
        message(
            &mut demo,
            &mut out,
            &[("t", "M"), ("o", "2")],
            b"text/plain",
        );
        assert_eq!(out, b"\x1b]72;t=r:o=0\x1b\\");
        assert!(!demo.dropping);
    }
}
