//! Inline viewport demo using the uncurses backend.
//!
//! Run with `cargo run --example ratatui_inline`. Renders a small inline
//! widget anchored at the cursor (no alternate screen, scrollback
//! preserved) and exits on the next key press. Exercises the backend's
//! inline-viewport support: `get_cursor_position` anchoring,
//! `append_lines` scrolling, and the inline-height screen buffer.

use std::io;

use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{TerminalOptions, Viewport};
use uncurses::event::Event;

const INLINE_HEIGHT: u16 = 3;

fn main() -> io::Result<()> {
    let mut terminal = uncurses_ratatui::try_init_with_options(
        TerminalOptions {
            viewport: Viewport::Inline(INLINE_HEIGHT),
        },
        uncurses_ratatui::ProgramOptions::default(),
    )?;
    let result = run(&mut terminal);
    uncurses_ratatui::restore(&mut terminal);
    result
}

fn run(terminal: &mut uncurses_ratatui::DefaultTerminal) -> io::Result<()> {
    loop {
        terminal.draw(|frame| {
            let area: Rect = frame.area();
            let block = Block::default().title(" inline ").borders(Borders::ALL);
            frame.render_widget(
                Paragraph::new("Inline viewport — press any key to exit.").block(block),
                area,
            );
        })?;
        let events = terminal.backend_mut();
        if events.poll_event(None)?
            && let Some(ev) = events.try_read_event()?
            && let Event::KeyPress(_) = ev
        {
            break;
        }
    }

    Ok(())
}
