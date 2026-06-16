//! Port of ratatui's `user-input` example using the uncurses backend.
//!
//! Run with `cargo run --example ratatui_user_input`. Press `e` to start
//! editing, `Esc` to stop, `Enter` to commit a message into the history,
//! and `q` to quit (from normal mode).

use std::io::{self, Write};

use ratatui::Frame;
use ratatui::Terminal;
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, List, ListItem, Paragraph};
use uncurses::event::{Event, EventSource, KeyCode};
use uncurses::screen::Screen;
use uncurses::terminal::{get_window_size, make_raw_mode, set_state, stdin, stdout};
use uncurses_ratatui::UncursesBackend;

enum InputMode {
    Normal,
    Editing,
}

struct App {
    input: String,
    character_index: usize,
    input_mode: InputMode,
    messages: Vec<String>,
}

impl App {
    fn new() -> Self {
        Self {
            input: String::new(),
            character_index: 0,
            input_mode: InputMode::Normal,
            messages: Vec::new(),
        }
    }

    fn move_cursor_left(&mut self) {
        self.character_index = self.clamp_cursor(self.character_index.saturating_sub(1));
    }

    fn move_cursor_right(&mut self) {
        self.character_index = self.clamp_cursor(self.character_index.saturating_add(1));
    }

    fn enter_char(&mut self, new_char: char) {
        let index = self.byte_index();
        self.input.insert(index, new_char);
        self.move_cursor_right();
    }

    fn byte_index(&self) -> usize {
        self.input
            .char_indices()
            .map(|(i, _)| i)
            .nth(self.character_index)
            .unwrap_or(self.input.len())
    }

    fn delete_char(&mut self) {
        if self.character_index == 0 {
            return;
        }
        let current = self.character_index;
        let before: String = self.input.chars().take(current - 1).collect();
        let after: String = self.input.chars().skip(current).collect();
        self.input = before + &after;
        self.move_cursor_left();
    }

    fn clamp_cursor(&self, new_cursor_pos: usize) -> usize {
        new_cursor_pos.clamp(0, self.input.chars().count())
    }

    fn submit_message(&mut self) {
        self.messages.push(self.input.clone());
        self.input.clear();
        self.character_index = 0;
    }
}

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

    let mut terminal = Terminal::new(UncursesBackend::new(screen))?;
    let mut events = EventSource::new(stdin)?;
    let mut app = App::new();

    'outer: loop {
        terminal.draw(|frame| render(frame, &app))?;
        if !events.poll(None)? {
            continue;
        }
        let Some(Event::KeyPress(key)) = events.try_read() else {
            continue;
        };
        match app.input_mode {
            InputMode::Normal => match key.code {
                KeyCode::Char('e') => app.input_mode = InputMode::Editing,
                KeyCode::Char('q') => break 'outer,
                _ => {}
            },
            InputMode::Editing => match key.code {
                KeyCode::Enter => app.submit_message(),
                KeyCode::Char(c) => app.enter_char(c),
                KeyCode::Backspace => app.delete_char(),
                KeyCode::Left => app.move_cursor_left(),
                KeyCode::Right => app.move_cursor_right(),
                KeyCode::Escape => app.input_mode = InputMode::Normal,
                _ => {}
            },
        }
    }

    let screen = terminal.backend_mut().screen_mut();
    screen.reset();
    screen.flush()?;
    Ok(())
}

fn render(frame: &mut Frame, app: &App) {
    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(1),
    ]);
    let [help_area, input_area, messages_area] = frame.area().layout(&layout);

    let (msg, style) = match app.input_mode {
        InputMode::Normal => (
            vec![
                "Press ".into(),
                "q".bold(),
                " to exit, ".into(),
                "e".bold(),
                " to start editing.".bold(),
            ],
            Style::default().add_modifier(Modifier::RAPID_BLINK),
        ),
        InputMode::Editing => (
            vec![
                "Press ".into(),
                "Esc".bold(),
                " to stop editing, ".into(),
                "Enter".bold(),
                " to record the message".into(),
            ],
            Style::default(),
        ),
    };
    let text = Text::from(Line::from(msg)).patch_style(style);
    frame.render_widget(Paragraph::new(text), help_area);

    let input = Paragraph::new(app.input.as_str())
        .style(match app.input_mode {
            InputMode::Normal => Style::default(),
            InputMode::Editing => Style::default().fg(Color::Yellow),
        })
        .block(Block::bordered().title("Input"));
    frame.render_widget(input, input_area);

    if let InputMode::Editing = app.input_mode {
        frame.set_cursor_position(Position::new(
            input_area.x + app.character_index as u16 + 1,
            input_area.y + 1,
        ));
    }

    let messages: Vec<ListItem> = app
        .messages
        .iter()
        .enumerate()
        .map(|(i, m)| ListItem::new(Line::from(Span::raw(format!("{i}: {m}")))))
        .collect();
    frame.render_widget(
        List::new(messages).block(Block::bordered().title("Messages")),
        messages_area,
    );
}
