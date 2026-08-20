//! Port of ratatui's `hello-world` example using the uncurses backend.
//!
//! Run with `cargo run --example ratatui_hello_world`. Displays a greeting
//! and exits when `q` is pressed.

use std::io;
use std::time::Duration;

use ratatui::widgets::Paragraph;
use uncurses::event::{Event, KeyCode};

fn main() -> io::Result<()> {
    let mut terminal = uncurses_ratatui::try_init()?;
    let result = run(&mut terminal);
    uncurses_ratatui::restore(&mut terminal);
    result
}

fn run(terminal: &mut uncurses_ratatui::DefaultTerminal) -> io::Result<()> {
    loop {
        terminal.draw(|frame| {
            frame.render_widget(
                Paragraph::new("Hello World! (press 'q' to quit)"),
                frame.area(),
            );
        })?;
        let events = terminal.backend_mut();
        if events.poll_event(Some(Duration::from_millis(250)))?
            && let Some(ev) = events.try_read_event()?
            && let Event::KeyPress(k) = ev
            && k.code == KeyCode::Char('q')
        {
            break;
        }
    }

    Ok(())
}
