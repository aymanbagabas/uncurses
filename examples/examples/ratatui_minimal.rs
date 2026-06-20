//! Port of ratatui's `minimal` example using the uncurses backend.
//!
//! Run with `cargo run --example ratatui_minimal`. Renders a single string
//! and exits on the next key press.

use std::io;

use uncurses::event::Event;

fn main() -> io::Result<()> {
    let mut terminal = uncurses_ratatui::try_init()?;
    let result = run(&mut terminal);
    uncurses_ratatui::restore(&mut terminal);
    result
}

fn run(terminal: &mut uncurses_ratatui::DefaultTerminal) -> io::Result<()> {
    loop {
        terminal.draw(|frame| frame.render_widget("Hello World!", frame.area()))?;
        let events = terminal.backend_mut();
        if events.poll_event(None)?
            && let Some(Event::KeyPress(_)) = events.try_read_event()
        {
            break;
        }
    }

    Ok(())
}
