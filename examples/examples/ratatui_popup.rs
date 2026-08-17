//! Port of ratatui's `popup` example using the uncurses backend.
//!
//! Run with `cargo run --example ratatui_popup`. Press `p` to toggle a
//! centered popup over the underlying content, `q` to quit. Demonstrates
//! the `Clear` widget overlay.

use std::io;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};
use uncurses::event::{Event, KeyCode};

fn main() -> io::Result<()> {
    let mut terminal = uncurses_ratatui::try_init()?;
    let result = run(&mut terminal);
    uncurses_ratatui::restore(&mut terminal);
    result
}

fn run(terminal: &mut uncurses_ratatui::DefaultTerminal) -> io::Result<()> {
    let mut show_popup = false;

    loop {
        terminal.draw(|frame| render(frame, show_popup))?;
        let events = terminal.backend_mut();
        if events.poll_event(None)?
            && let Some(ev) = events.try_read_event()?
        {
            events.observe_event(&ev)?;
            if let Event::KeyPress(key) = ev {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('p') => show_popup = !show_popup,
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

fn render(frame: &mut Frame, show_popup: bool) {
    let area = frame.area();
    let layout = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]);
    let [instructions, content] = area.layout(&layout);

    frame.render_widget(
        Line::from("Press 'p' to toggle popup, 'q' to quit").centered(),
        instructions,
    );
    frame.render_widget(Block::bordered().title("Content").on_blue(), content);

    if show_popup {
        let popup_block = Block::bordered().title("Popup");
        let centered_area = area.centered(Constraint::Percentage(60), Constraint::Percentage(20));
        frame.render_widget(Clear, centered_area);
        frame.render_widget(
            Paragraph::new("Lorem ipsum").block(popup_block),
            centered_area,
        );
    }
}
