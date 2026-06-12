//! Port of ratatui's `modifiers` example using the uncurses backend.
//!
//! Run with `cargo run --example ratatui_modifiers`. Renders a grid of
//! foreground/background combinations with every text modifier applied
//! to demonstrate which modifiers the host terminal supports.

use std::io::{self, Write};
use std::iter::once;

use ratatui::Frame;
use ratatui::Terminal;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use uncurses::event::{Event, Source};
use uncurses::screen::{Options, Screen};
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
    let mut screen = Screen::with_options(
        stdout(),
        Options {
            size: (size.col, size.row),
            ..Default::default()
        },
    );
    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(false)?;

    let mut terminal = Terminal::new(UncursesBackend::new(screen))?;
    let mut events = Source::new(stdin())?;

    loop {
        terminal.draw(render)?;
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

fn render(frame: &mut Frame) {
    let layout = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]);
    let [text_area, main_area] = frame.area().layout(&layout);
    frame.render_widget(
        Paragraph::new("Note: not all terminals support all modifiers")
            .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        text_area,
    );

    let rows = Layout::vertical([Constraint::Length(1); 50]).split(main_area);
    let cells: Vec<_> = rows
        .iter()
        .flat_map(|row| {
            Layout::horizontal([Constraint::Percentage(20); 5])
                .split(*row)
                .to_vec()
        })
        .collect();

    let colors = [
        Color::Black,
        Color::DarkGray,
        Color::Gray,
        Color::White,
        Color::Red,
    ];
    let all_modifiers: Vec<_> = once(Modifier::empty())
        .chain(Modifier::all().iter())
        .collect();

    let mut index = 0;
    for bg in &colors {
        for fg in &colors {
            for modifier in &all_modifiers {
                if index >= cells.len() {
                    return;
                }
                let modifier_name = format!("{modifier:11?}");
                let padding = " ".repeat(12 - modifier_name.len());
                let paragraph = Paragraph::new(Line::from(vec![
                    modifier_name.fg(*fg).bg(*bg).add_modifier(*modifier),
                    padding.fg(*fg).bg(*bg).add_modifier(*modifier),
                ]));
                frame.render_widget(paragraph, cells[index]);
                index += 1;
            }
        }
    }
}
