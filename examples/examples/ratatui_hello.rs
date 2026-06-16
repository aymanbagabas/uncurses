//! Minimal ratatui app driven by [`uncurses_ratatui::UncursesBackend`].
//!
//! Renders a centered greeting inside a bordered block and exits on any
//! keypress (or after 30 seconds).

use std::io::{self, Write};
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::layout::Alignment;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
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
    screen.set_alt_screen(true);
    screen.set_cursor_visible(false);

    let backend = UncursesBackend::new(screen);
    let mut terminal = Terminal::new(backend)?;

    terminal.draw(|f| {
        let block = Block::default()
            .title(" uncurses-ratatui ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let para = Paragraph::new(
            "Hello from a ratatui app rendered through uncurses.\n\nPress any key to exit.",
        )
        .alignment(Alignment::Center)
        .block(block);
        f.render_widget(para, f.area());
    })?;

    let mut events = EventSource::new(stdin)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if events.poll(Some(Duration::from_millis(100)))?
            && let Some(Event::KeyPress(_)) = events.try_read()
        {
            break;
        }
    }

    let screen = terminal.backend_mut().screen_mut();
    screen.reset();
    screen.flush()?;

    Ok(())
}
