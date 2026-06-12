//! Port of ratatui's `hello-world` example using the uncurses backend.
//!
//! Run with `cargo run --example ratatui_hello_world`. Displays a greeting
//! and exits when `q` is pressed.

use std::io::{self, Write};
use std::time::Duration;

use ratatui::Terminal;
use ratatui::widgets::Paragraph;
use uncurses::event::{Event, KeyCode, Source};
use uncurses::screen::{Options, Screen};
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses_ratatui::UncursesBackend;

fn main() -> io::Result<()> {
    let stdin = stdin();
    let stdout = stdout();
    let raw_state = enable_raw_mode(stdin, stdout)?;
    let result = run();
    disable_raw_mode(stdin, stdout, &raw_state)?;
    result
}

fn run() -> io::Result<()> {
    let stdin = stdin();
    let stdout = stdout();
    let size = get_window_size(stdout).unwrap_or_default();
    let mut screen = Screen::with_options(
        stdout,
        Options {
            size: (size.col, size.row),
            ..Default::default()
        },
    );
    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(false)?;

    let mut terminal = Terminal::new(UncursesBackend::new(screen))?;
    let mut events = Source::new(stdin)?;

    loop {
        terminal.draw(|frame| {
            frame.render_widget(
                Paragraph::new("Hello World! (press 'q' to quit)"),
                frame.area(),
            );
        })?;
        if events.poll(Some(Duration::from_millis(250)))?
            && let Some(Event::KeyPress(k)) = events.try_read()
            && k.code == KeyCode::Char('q')
        {
            break;
        }
    }

    let screen = terminal.backend_mut().screen_mut();
    screen.reset()?;
    screen.flush()?;
    Ok(())
}
