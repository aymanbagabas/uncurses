//! Minimal ratatui inline-viewport example using the uncurses backend.
//!
//! Run with `cargo run --example ratatui_inline_clock`. Reserves a 3-line
//! inline viewport at the bottom of the current screen (no alternate
//! screen) showing the elapsed time. Every second a log line is committed
//! above the viewport via [`Terminal::insert_before`] (it scrolls into
//! the scrollback). Press `q` to quit.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use uncurses::event::{Event, KeyCode, Source};
use uncurses::screen::Screen;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size};
use uncurses_ratatui::UncursesBackend;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};

fn main() -> io::Result<()> {
    let raw_state = enable_raw_mode(io::stdin(), io::stdout())?;
    let result = run();
    disable_raw_mode(io::stdin(), io::stdout(), &raw_state)?;
    result
}

fn run() -> io::Result<()> {
    const VIEWPORT_HEIGHT: u16 = 3;
    let size = get_window_size(io::stdout()).unwrap_or_default();
    let mut screen = Screen::new(io::stdout()).with_size(size.col, VIEWPORT_HEIGHT);
    screen.set_cursor_visible(false)?;

    let mut terminal = Terminal::with_options(
        UncursesBackend::new(screen),
        TerminalOptions {
            viewport: Viewport::Inline(VIEWPORT_HEIGHT),
        },
    )?;
    let mut events = Source::new(io::stdin())?;

    let start = Instant::now();
    let mut next_tick = start + Duration::from_secs(1);
    let mut ticks = 0u32;

    loop {
        let elapsed = start.elapsed();
        terminal.draw(|frame| {
            let p = Paragraph::new(Line::from(vec![
                Span::raw("Elapsed: "),
                Span::styled(
                    format!("{:>3}.{:03}s", elapsed.as_secs(), elapsed.subsec_millis()),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw("   (press 'q' to quit)"),
            ]))
            .block(Block::default().borders(Borders::ALL).title(" inline "));
            frame.render_widget(p, frame.area());
        })?;

        let now = Instant::now();
        if now >= next_tick {
            ticks += 1;
            let line = format!("tick {ticks} at {:.3}s", elapsed.as_secs_f32());
            terminal.insert_before(1, |buf| {
                Paragraph::new(line).render(buf.area, buf);
            })?;
            next_tick += Duration::from_secs(1);
        }

        let wait = next_tick.saturating_duration_since(Instant::now());
        if events.poll(Some(wait))?
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
