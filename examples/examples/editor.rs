//! Shell out to `$EDITOR` with `Screen::pause` and `Screen::resume`.
//!
//! Sometimes a TUI needs to hand the whole terminal to another program —
//! open `$EDITOR`, run a pager, drop to a shell — and take it back when
//! that program exits. [`Screen::pause`] tears down raw mode and the
//! alternate screen and gives the terminal back; [`Screen::resume`]
//! re-acquires it and repaints. The screen keeps all its state in between,
//! so resuming is seamless.
//!
//! This app holds a scratch buffer. Press `e` to edit it in `$EDITOR`
//! (falling back to `vi`); when the editor exits, the edited text appears
//! back in the app. Press `q` to quit.
//!
//! Run with `cargo run --example editor`.

use std::process::Command;

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::color::Color;
use uncurses::event::{Event, Key, KeyCode};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init()?;
    screen.enter_alt_screen()?;
    screen.hide_cursor()?;

    let result = run(&mut screen);
    screen.finish()?;
    result
}

fn run(screen: &mut Screen<Stdin, Stdout>) -> std::io::Result<()> {
    let mut text = String::from("Edit me in $EDITOR.\n\nLine two.\n");
    let mut status = String::new();
    render(screen, &text, &status);

    loop {
        let ev = screen.read_event()?;
        screen.observe_event(&ev)?;
        match ev {
            Event::KeyPress(Key {
                code: KeyCode::Char('q'),
                ..
            }) => break,
            Event::KeyPress(Key {
                code: KeyCode::Char('e'),
                ..
            }) => {
                status = match edit_in_editor(screen, &text) {
                    Ok(edited) => {
                        text = edited;
                        "edited in $EDITOR".to_string()
                    }
                    Err(e) => format!("editor failed: {e}"),
                };
                render(screen, &text, &status);
            }
            Event::Resize(ws) => {
                screen.resize((ws.col, ws.row));
                render(screen, &text, &status);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Write `text` to a temp file, hand the terminal to `$EDITOR`, then take
/// it back and read the file. The `pause`/`resume` pair brackets the child
/// so the editor sees a normal cooked terminal.
fn edit_in_editor(screen: &mut Screen<Stdin, Stdout>, text: &str) -> std::io::Result<String> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());

    let path = std::env::temp_dir().join("uncurses_editor_example.txt");
    std::fs::write(&path, text)?;

    // Hand the terminal back, run the editor (it inherits our stdio), then
    // re-acquire. `resume` runs even if the editor fails, so the UI always
    // comes back.
    screen.pause()?;
    let spawn = Command::new(&editor).arg(&path).status();
    screen.resume()?;
    spawn?;

    let edited = std::fs::read_to_string(&path)?;
    let _ = std::fs::remove_file(&path);
    Ok(edited)
}

fn render(screen: &mut Screen<Stdin, Stdout>, text: &str, status: &str) {
    screen.clear();
    let dim = Style::default().fg(Color::BrightBlack);
    screen.set_str((0, 0), "e: edit in $EDITOR   q: quit", dim.clone());
    if !status.is_empty() {
        screen.set_str((0, 1), status, Style::default().fg(Color::BrightGreen));
    }

    let height = screen.height();
    for (i, line) in text.lines().enumerate() {
        let row = 3 + i as u16;
        if row >= height {
            break;
        }
        screen.set_str((0, row), line, Style::default());
    }

    let _ = screen.render();
}
