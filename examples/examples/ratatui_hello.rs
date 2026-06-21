//! Minimal ratatui app driven by [`uncurses_ratatui::UncursesBackend`].
//!
//! Renders a centered greeting inside a bordered block and exits on any
//! keypress (or after 30 seconds).

use std::io;
use std::time::{Duration, Instant};

use ratatui::layout::Alignment;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use uncurses::event::Event;

fn main() -> io::Result<()> {
    let mut terminal = uncurses_ratatui::try_init()?;
    let result = run(&mut terminal);
    uncurses_ratatui::restore(&mut terminal);
    result
}

fn run(terminal: &mut uncurses_ratatui::DefaultTerminal) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
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

        let events = terminal.backend_mut();
        if events.poll_event(Some(Duration::from_millis(100)))?
            && let Some(Event::KeyPress(_)) = events.try_read_event()
        {
            break;
        }
    }

    Ok(())
}
