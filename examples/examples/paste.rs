//! Bracketed paste: capture pasted text as one unit, not keystroke spam.
//!
//! With bracketed paste on (a [`Screen`] default), the terminal wraps
//! pasted text in [`Event::PasteStart`] / [`Event::PasteEnd`] and streams
//! the body as [`Event::PasteChunk`] payloads. That lets you tell a paste
//! apart from someone typing fast, and reassemble it by accumulating the
//! chunks into a `Vec<u8>`. This app shows the last thing you pasted.
//!
//! Run with `cargo run --example paste`. Paste some text (try a few
//! lines); press `q` or `Ctrl-C` to quit.

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::color::Color;
use uncurses::event::{Event, Key};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init()?; // bracketed paste is enabled by default
    screen.enter_alt_screen()?;
    screen.hide_cursor()?;

    let result = run(&mut screen);
    screen.finish()?;
    result
}

fn run(screen: &mut Screen<Stdin, Stdout>) -> std::io::Result<()> {
    let quit: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
    let mut last: Option<String> = None;
    // Holds chunk bytes between PasteStart and PasteEnd.
    let mut pending: Option<Vec<u8>> = None;
    render(screen, last.as_deref());

    loop {
        let ev = screen.read_event()?;
        screen.observe_event(&ev)?;
        match ev {
            Event::KeyPress(ref k) if quit.contains(k) => break,
            Event::PasteStart => pending = Some(Vec::new()),
            Event::PasteChunk(bytes) => {
                if let Some(buf) = pending.as_mut() {
                    buf.extend_from_slice(&bytes);
                }
            }
            Event::PasteEnd => {
                if let Some(buf) = pending.take() {
                    last = Some(String::from_utf8_lossy(&buf).into_owned());
                    render(screen, last.as_deref());
                }
            }
            Event::Resize(ws) => {
                screen.resize((ws.col, ws.row));
                render(screen, last.as_deref());
            }
            _ => {}
        }
    }
    Ok(())
}

fn render(screen: &mut Screen<Stdin, Stdout>, last: Option<&str>) {
    screen.clear();
    let dim = Style::default().fg(Color::BrightBlack);
    screen.set_str((0, 0), "Paste some text. q quits.", dim.clone());

    match last {
        None => {
            screen.set_str((0, 2), "(nothing pasted yet)", dim);
        }
        Some(text) => {
            let lines = text.lines().count().max(1);
            let chars = text.chars().count();
            let summary = format!("pasted {chars} chars across {lines} line(s):");
            screen.set_str((0, 2), &summary, Style::default());

            let body = Style::default().fg(Color::BrightGreen);
            let height = screen.height();
            for (i, line) in text.lines().enumerate() {
                let row = 4 + i as u16;
                if row >= height {
                    break;
                }
                screen.set_str((0, row), line, body.clone());
            }
        }
    }

    let _ = screen.render();
}
