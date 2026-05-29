//! Port of ratatui's `hello-world` example using the uncurses backend.
//!
//! Run with `cargo run --example ratatui_hello_world`. Displays a greeting
//! and exits when `q` is pressed.

use std::io::{self, Write};
use std::time::Duration;

use uncurses::event::{Event, KeyCode, Source};
use uncurses::screen::Screen;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size};
use uncurses_ratatui::UncursesBackend;
use ratatui::Terminal;
use ratatui::widgets::Paragraph;

fn main() -> io::Result<()> {
    let raw_state = enable_raw_mode(io::stdin(), io::stdout())?;
    let result = run();
    disable_raw_mode(io::stdin(), io::stdout(), &raw_state)?;
    result
}

fn run() -> io::Result<()> {
    let size = get_window_size(io::stdout()).unwrap_or_default();
    let mut screen = Screen::new(io::stdout()).with_size(size.col, size.row);
    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(false)?;

    let mut terminal = Terminal::new(UncursesBackend::new(screen))?;
    let mut events = Source::new(io::stdin())?;

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
