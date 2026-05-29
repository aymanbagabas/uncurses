//! Port of ratatui's `popup` example using the uncurses backend.
//!
//! Run with `cargo run --example ratatui_popup`. Press `p` to toggle a
//! centered popup over the underlying content, `q` to quit. Demonstrates
//! the `Clear` widget overlay.

use std::io::{self, Write};

use uncurses::event::{Event, KeyCode, Source};
use uncurses::screen::Screen;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size};
use uncurses_ratatui::UncursesBackend;
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};

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
    let mut show_popup = false;

    loop {
        terminal.draw(|frame| render(frame, show_popup))?;
        if events.poll(None)?
            && let Some(Event::KeyPress(key)) = events.try_read()
        {
            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Char('p') => show_popup = !show_popup,
                _ => {}
            }
        }
    }

    let screen = terminal.backend_mut().screen_mut();
    screen.reset()?;
    screen.flush()?;
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
