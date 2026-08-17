//! Multiline inline input prompt.
//!
//! Run with `cargo run --example inline_input`. Runs in *inline* mode:
//! the prompt grows vertically as you type, editing in place at the
//! bottom of the screen. `Enter` inserts a newline at the cursor.
//! `Ctrl-D` commits the whole multiline block above the screen via
//! [`Screen::insert_above`] (it scrolls into the scrollback) and
//! clears the buffer for the next entry.
//!
//! Navigation: arrow keys move within the buffer. `Backspace` deletes
//! the previous character (or joins lines at column 0). Pasted text
//! is accumulated across `PasteChunk` events into a `Vec<u8>`, decoded
//! as UTF-8 on `PasteEnd`, and inserted at the cursor (embedded newlines
//! split lines).
//!
//! Press `Esc` or `Ctrl-C` to exit.

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::event::{Event, Key, KeyCode, KeyModifiers};
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::terminal::{TtyInput, TtyOutput};
use uncurses::text::{TextSurface, char_width};

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

fn redraw(program: &mut Program<TtyInput, TtyOutput>, buf: &Buffer) {
    program.screen_mut().clear();
    for (row, line) in buf.lines.iter().enumerate() {
        let prefix = if row == 0 { "> " } else { ". " };
        let rendered = format!("{}{}", prefix, line);
        {
            program
                .screen_mut()
                .set_str((0, row as u16), &rendered, Style::default());
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
    program.screen_mut().set_str(
        (cursor_x, buf.row as u16),
        &cursor_ch.to_string(),
        Style::default().reverse(),
    );
}

struct App {
    program: Program<TtyInput, TtyOutput>,
    buffer: Buffer,
    paste: Option<Vec<u8>>,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut program = Program::open()?;
        // Inline mode: no alternate screen. Bracketed paste is enabled by
        // default, so PasteStart/Chunk/End events arrive as the user pastes.
        program.init()?;
        program.hide_cursor()?;
        // Start one row tall; the prompt grows as lines are added.
        let w = program.screen().width();
        program.screen_mut().resize((w, 1));

        let buffer = Buffer::new();
        let paste = None;

        Ok(Self {
            program,
            buffer,
            paste,
        })
    }

    fn render(&mut self) -> std::io::Result<()> {
        redraw(&mut self.program, &self.buffer);
        self.program.screen_mut().render()
    }

    fn run(&mut self) -> std::io::Result<()> {
        let w = self.program.screen().width();
        self.program
            .screen_mut()
            .resize((w, self.buffer.lines.len() as u16));
        self.render()?;

        while let Ok(ev) = self.program.read_event() {
            let _ = self.program.observe_event(&ev);
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
                        self.program.screen_mut().insert_above(&text)?;
                        self.buffer.clear();
                    }
                    KeyCode::Enter => self.buffer.insert_newline(),
                    KeyCode::Backspace => self.buffer.backspace(),
                    KeyCode::Left => self.buffer.move_left(),
                    KeyCode::Right => self.buffer.move_right(),
                    KeyCode::Up => self.buffer.move_up(),
                    KeyCode::Down => self.buffer.move_down(),
                    KeyCode::Space
                        if !modifiers.intersects(KeyModifiers::CTRL | KeyModifiers::ALT) =>
                    {
                        self.buffer.insert_char(' ');
                    }
                    KeyCode::Char(c)
                        if !modifiers.intersects(KeyModifiers::CTRL | KeyModifiers::ALT) =>
                    {
                        self.buffer.insert_char(c);
                    }
                    _ => {}
                },
                Event::PasteStart => {
                    self.paste = Some(Vec::new());
                }
                Event::PasteChunk(bytes) => {
                    if let Some(p) = self.paste.as_mut() {
                        p.extend_from_slice(&bytes);
                    }
                }
                Event::PasteEnd => {
                    if let Some(p) = self.paste.take() {
                        let text = String::from_utf8_lossy(&p).into_owned();
                        self.buffer.insert_str(&text);
                    }
                }
                Event::Resize(ws) => {
                    self.program
                        .screen_mut()
                        .resize((ws.col, self.buffer.lines.len() as u16));
                }
                _ => {}
            }

            let w = self.program.screen().width();
            self.program
                .screen_mut()
                .resize((w, self.buffer.lines.len() as u16));
            self.render()?;
        }
        Ok(())
    }

    fn stop(self) -> std::io::Result<()> {
        self.program.finish()
    }
}

fn main() -> std::io::Result<()> {
    let mut app = App::start()?;
    let result = app.run();
    app.stop()?;
    result
}
