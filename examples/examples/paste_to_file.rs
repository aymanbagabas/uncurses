//! Streaming paste with spill-to-file past a size threshold.
//!
//! Bracketed paste arrives as a stream of [`Event::PasteChunk`] payloads
//! between [`Event::PasteStart`] and [`Event::PasteEnd`]. Small pastes are
//! fine in memory; a megabyte of pasted log lines is not. This app feeds
//! chunks into a sink that keeps bytes in memory until they cross a
//! threshold, then spills the buffer to a temp file and streams the rest
//! straight to disk. On `PasteEnd` it reports where the paste landed.
//!
//! Run with `cargo run --example paste_to_file`. Paste something small to
//! see it in memory, then paste a large block to watch it spill to a file.
//! Press `q` or `Ctrl-C` to quit.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::color::Color;
use uncurses::event::{Event, Key};
use uncurses::program::Program;
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

/// Spill to a file once a paste exceeds this many bytes in memory. Kept
/// small so a modest paste demonstrates the spill.
const THRESHOLD: usize = 512;

/// Collects paste chunks, spilling to a temp file past [`THRESHOLD`].
struct PasteSink {
    mem: Vec<u8>,
    total: usize,
    spill: Option<(PathBuf, BufWriter<File>)>,
}

impl PasteSink {
    fn new() -> Self {
        Self {
            mem: Vec::new(),
            total: 0,
            spill: None,
        }
    }

    /// Append one chunk, spilling to disk if the in-memory buffer grows
    /// past the threshold.
    fn push(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        self.total += chunk.len();

        // Already spilling: stream straight to the file.
        if let Some((_, file)) = self.spill.as_mut() {
            return file.write_all(chunk);
        }

        self.mem.extend_from_slice(chunk);
        if self.mem.len() > THRESHOLD {
            // Cross the threshold: open a file, flush what we have, and
            // switch to file mode for the rest of the stream.
            let path =
                std::env::temp_dir().join(format!("uncurses_paste_{}.txt", std::process::id()));
            let mut file = BufWriter::new(File::create(&path)?);
            file.write_all(&self.mem)?;
            self.mem = Vec::new();
            self.spill = Some((path, file));
        }
        Ok(())
    }

    /// Finish the paste, returning a human-readable outcome.
    fn finish(mut self) -> std::io::Result<Outcome> {
        match self.spill.take() {
            Some((path, mut file)) => {
                file.flush()?;
                Ok(Outcome::File {
                    path,
                    bytes: self.total,
                })
            }
            None => Ok(Outcome::Memory {
                preview: String::from_utf8_lossy(&self.mem).into_owned(),
                bytes: self.total,
            }),
        }
    }
}

enum Outcome {
    Memory { preview: String, bytes: usize },
    File { path: PathBuf, bytes: usize },
}

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    program.init()?; // bracketed paste on by default
    program.enter_alt_screen()?;
    program.hide_cursor()?;

    let result = run(&mut program);
    program.finish()?;
    result
}

fn run(program: &mut Program<Stdin, Stdout>) -> std::io::Result<()> {
    let quit: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
    let mut last: Option<Outcome> = None;
    let mut sink: Option<PasteSink> = None;
    render(program.screen_mut(), last.as_ref());

    loop {
        let ev = program.read_event()?;
        program.observe_event(&ev)?;
        match ev {
            Event::KeyPress(ref k) if quit.contains(k) => break,
            Event::PasteStart => sink = Some(PasteSink::new()),
            Event::PasteChunk(bytes) => {
                if let Some(s) = sink.as_mut() {
                    s.push(&bytes)?;
                }
            }
            Event::PasteEnd => {
                if let Some(s) = sink.take() {
                    last = Some(s.finish()?);
                    render(program.screen_mut(), last.as_ref());
                }
            }
            Event::Resize(ws) => {
                program.screen_mut().resize((ws.col, ws.row));
                render(program.screen_mut(), last.as_ref());
            }
            _ => {}
        }
    }
    Ok(())
}

fn render(screen: &mut Screen<Stdout>, last: Option<&Outcome>) {
    screen.clear();
    let dim = Style::default().fg(Color::BrightBlack);
    let hint = format!("Paste text. Pastes over {THRESHOLD} bytes spill to a file. q quits.");
    screen.set_str((0, 0), &hint, dim.clone());

    let height = screen.height();
    match last {
        None => {
            screen.set_str((0, 2), "(nothing pasted yet)", dim);
        }
        Some(Outcome::Memory { preview, bytes }) => {
            let head = format!("kept {bytes} bytes in memory:");
            screen.set_str((0, 2), &head, Style::default());
            let body = Style::default().fg(Color::BrightGreen);
            for (i, line) in preview.lines().enumerate() {
                let row = 4 + i as u16;
                if row >= height {
                    break;
                }
                screen.set_str((0, row), line, body.clone());
            }
        }
        Some(Outcome::File { path, bytes }) => {
            let head = format!("spilled {bytes} bytes to a file:");
            screen.set_str((0, 2), &head, Style::default());
            let body = Style::default().fg(Color::BrightYellow);
            screen.set_str((0, 4), &path.display().to_string(), body);
        }
    }

    let _ = screen.render();
}
