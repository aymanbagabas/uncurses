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
use uncurses::event::{Event, Source};
use uncurses::screen::Screen;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses_ratatui::UncursesBackend;

fn main() -> io::Result<()> {
    let raw_state = enable_raw_mode(stdin(), stdout())?;
    let result = run();
    disable_raw_mode(stdin(), stdout(), &raw_state)?;
    result
}

fn run() -> io::Result<()> {
    let size = get_window_size(stdout()).unwrap_or_default();
    let mut screen = Screen::new(stdout()).with_size(size.col, size.row);
    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(false)?;

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

    let mut events = Source::new(stdin())?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if events.poll(Some(Duration::from_millis(100)))?
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
