//! Port of ratatui's `hello-world` example using the uncurses backend.
//!
//! Run with `cargo run --example ratatui_hello_world`. Displays a greeting
//! and exits when `q` is pressed.

use std::io::{self, Write};
use std::time::Duration;

use ratatui::Terminal;
use ratatui::widgets::Paragraph;
use uncurses::event::{Event, EventSource, KeyCode};
use uncurses::screen::Screen;
use uncurses::terminal::{get_window_size, make_raw_mode, set_state, stdin, stdout};
use uncurses_ratatui::UncursesBackend;

fn main() -> io::Result<()> {
    let stdin = stdin();
    let stdout = stdout();
    let raw_state = make_raw_mode(stdin, stdout)?;
    let result = run();
    set_state(stdin, stdout, &raw_state)?;
    result
}

fn run() -> io::Result<()> {
    let stdin = stdin();
    let stdout = stdout();
    let size = get_window_size(stdout).unwrap_or_default();
    let mut screen = Screen::new(stdout, (size.col, size.row));
    screen.set_alt_screen(true);
    screen.set_cursor_visible(false);

    let mut terminal = Terminal::new(UncursesBackend::new(screen))?;
    let mut events = EventSource::new(stdin)?;

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
