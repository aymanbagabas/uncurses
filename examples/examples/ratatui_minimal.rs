//! Port of ratatui's `minimal` example using the uncurses backend.
//!
//! Run with `cargo run --example ratatui_minimal`. Renders a single string
//! and exits on the next key press.

use std::io::{self, Write};

use ratatui::Terminal;
use uncurses::event::{Event, EventSource};
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
    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(false)?;

    let mut terminal = Terminal::new(UncursesBackend::new(screen))?;
    let mut events = EventSource::new(stdin)?;

    loop {
        terminal.draw(|frame| frame.render_widget("Hello World!", frame.area()))?;
        if events.poll(None)?
            && let Some(Event::KeyPress(_)) = events.try_read()
        {
            break;
        }
    }

    let screen = terminal.backend_mut().screen_mut();
    screen.reset()?;
    screen.flush()?;
    Ok(())
}
