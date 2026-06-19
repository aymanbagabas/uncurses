//! Multiline inline input prompt.
//!
//! Run with `cargo run --example inline_input`. Runs in *inline* mode:
//! the prompt grows vertically as you type, editing in place at the
//! bottom of the screen. `Enter` inserts a newline at the cursor.
//! `Ctrl-D` commits the whole multiline block above the screen via
//! [`Canvas::insert_above`] (it scrolls into the scrollback) and
//! clears the buffer for the next entry.
//!
//! Navigation: arrow keys move within the buffer. `Backspace` deletes
//! the previous character (or joins lines at column 0). Pasted text
//! is accumulated across `PasteChunk` events into a
//! [`PasteBuffer`](uncurses::event::PasteBuffer), decoded as UTF-8
//! on `PasteEnd`, and inserted at the cursor (embedded newlines
//! split lines).
//!
//! Press `Esc` or `Ctrl-C` to exit.

use std::io::Write;

use uncurses::buffer::SurfaceMut;
use uncurses::canvas::Canvas;
use uncurses::event::{Event, EventSource, Key, KeyCode, KeyModifiers, PasteBuffer};
use uncurses::style::Style;
use uncurses::terminal::Terminal;
use uncurses::terminal::{TtyInput, TtyOutput};
use uncurses::text::char_width;

/// Editable multiline buffer with a single cursor.
struct Buffer {
    lines: Vec<String>,
    row: usize,
    col: usize,
}

impl Buffer {
    fn new() -> Self {
        Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
        }
    }

    fn clear(&mut self) {
        self.lines.clear();
        self.lines.push(String::new());
        self.row = 0;
        self.col = 0;
    }

    fn is_blank(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    fn as_text(&self) -> String {
        self.lines.join("\n")
    }

    fn insert_char(&mut self, c: char) {
        let line = &mut self.lines[self.row];
        let byte = char_index_to_byte(line, self.col);
        line.insert(byte, c);
        self.col += 1;
    }

    fn insert_newline(&mut self) {
        let line = &mut self.lines[self.row];
        let byte = char_index_to_byte(line, self.col);
        let tail = line.split_off(byte);
        self.lines.insert(self.row + 1, tail);
        self.row += 1;
        self.col = 0;
    }

    fn backspace(&mut self) {
        if self.col > 0 {
            let line = &mut self.lines[self.row];
            let end = char_index_to_byte(line, self.col);
            let start = char_index_to_byte(line, self.col - 1);
            line.replace_range(start..end, "");
            self.col -= 1;
        } else if self.row > 0 {
            let cur = self.lines.remove(self.row);
            self.row -= 1;
            let prev = &mut self.lines[self.row];
            self.col = prev.chars().count();
            prev.push_str(&cur);
        }
    }

    fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
        }
    }

    fn move_right(&mut self) {
        let line_len = self.lines[self.row].chars().count();
        if self.col < line_len {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    fn move_up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.lines[self.row].chars().count());
        }
    }

    fn move_down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.lines[self.row].chars().count());
        }
    }

    fn insert_str(&mut self, s: &str) {
        // Normalize CRLF and lone CR to LF so the buffer's line model
        // sees a single delimiter regardless of the terminal's paste
        // line-ending convention.
        let normalized = s.replace("\r\n", "\n").replace('\r', "\n");
        let mut parts = normalized.split('\n');
        if let Some(first) = parts.next() {
            for c in first.chars() {
                if !c.is_control() {
                    self.insert_char(c);
                }
            }
        }
        for rest in parts {
            self.insert_newline();
            for c in rest.chars() {
                if !c.is_control() {
                    self.insert_char(c);
                }
            }
        }
    }
}

fn char_index_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

fn redraw<W: std::io::Write>(screen: &mut Canvas<W>, buf: &Buffer) {
    screen.clear();
    for (row, line) in buf.lines.iter().enumerate() {
        let prefix = if row == 0 { "> " } else { ". " };
        let rendered = format!("{}{}", prefix, line);
        {
            screen.set_str(
                (0, row as u16),
                &rendered,
                uncurses::style::Style::default(),
            );
        };
    }

    // Draw a visible cursor cell by re-writing the character at the
    // cursor position with a reversed style (or a space if past EOL).
    // The cursor's column is the sum of display widths of all chars
    // before `buf.col`, so wide characters (CJK / emoji) and zero-width
    // combining marks line up with the rendered cells.
    let line = &buf.lines[buf.row];
    let line_chars: Vec<char> = line.chars().collect();
    let prefix_w = 2u16;
    let before_width: u16 = line_chars
        .iter()
        .take(buf.col)
        .map(|c| char_width(*c, false) as u16)
        .sum();
    let cursor_x = prefix_w + before_width;
    let cursor_ch = line_chars.get(buf.col).copied().unwrap_or(' ');
    screen.set_str(
        (cursor_x, buf.row as u16),
        &cursor_ch.to_string(),
        Style::default().reverse(),
    );
}

struct App {
    term: Terminal<TtyInput, TtyOutput>,
    screen: Canvas<TtyOutput>,
    events: EventSource<TtyInput>,
    buffer: Buffer,
    paste: Option<PasteBuffer>,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut term = Terminal::open()?;
        term.make_raw()?;
        let mut screen = Canvas::new(
            term.output(),
            (term.window_size().unwrap_or_default().col, 1),
        );

        screen.set_cursor_visible(false);

        let events = EventSource::new(term.input())?;
        let buffer = Buffer::new();
        let paste = None;

        Ok(Self {
            term,
            screen,
            events,
            buffer,
            paste,
        })
    }

    fn render(&mut self) -> std::io::Result<()> {
        redraw(&mut self.screen, &self.buffer);
        self.screen.present()
    }

    fn run(&mut self) -> std::io::Result<()> {
        self.screen
            .resize(self.screen.width(), self.buffer.lines.len() as u16);
        self.render()?;

        while let Ok(ev) = self.events.read() {
            match ev {
                Event::KeyPress(Key {
                    code, modifiers, ..
                }) => match code {
                    KeyCode::Escape => break,
                    KeyCode::Char('c') if modifiers.contains(KeyModifiers::CTRL) => break,
                    KeyCode::Char('d')
                        if modifiers.contains(KeyModifiers::CTRL) && !self.buffer.is_blank() =>
                    {
                        let text = self.buffer.as_text();
                        self.screen.insert_above(&text);
                        self.buffer.clear();
                    }
                    KeyCode::Enter => self.buffer.insert_newline(),
                    KeyCode::Backspace => self.buffer.backspace(),
                    KeyCode::Left => self.buffer.move_left(),
                    KeyCode::Right => self.buffer.move_right(),
                    KeyCode::Up => self.buffer.move_up(),
                    KeyCode::Down => self.buffer.move_down(),
                    KeyCode::Char(c)
                        if !modifiers.intersects(KeyModifiers::CTRL | KeyModifiers::ALT) =>
                    {
                        self.buffer.insert_char(c);
                    }
                    _ => {}
                },
                Event::PasteStart => {
                    self.paste = Some(PasteBuffer::new());
                }
                Event::PasteChunk(bytes) => {
                    if let Some(p) = self.paste.as_mut() {
                        p.push(&bytes);
                    }
                }
                Event::PasteEnd => {
                    if let Some(p) = self.paste.take() {
                        let text = p.into_string_lossy();
                        self.buffer.insert_str(&text);
                    }
                }
                Event::Resize(ws) => {
                    self.screen.resize(ws.col, self.buffer.lines.len() as u16);
                }
                _ => {}
            }

            let w = self.screen.width();
            self.screen.resize(w, self.buffer.lines.len() as u16);
            self.render()?;
        }
        Ok(())
    }

    fn stop(&mut self) -> std::io::Result<()> {
        self.screen.reset();
        self.screen.flush()?;
        self.term.restore()
    }
}

fn main() -> std::io::Result<()> {
    let mut app = App::start()?;
    let result = app.run();
    app.stop()?;
    result
}
