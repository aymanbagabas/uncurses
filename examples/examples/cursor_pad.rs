//! Tiny scratch pad: move the cursor with the arrow keys and type
//! anything to write at the current cursor position.
//!
//! Run with `cargo run --example cursor_pad`.
//!
//! Controls:
//! - Arrow keys: move the cursor (clamped to the screen).
//! - Left mouse click: jump the cursor to the clicked cell.
//! - Printable characters: write the character at the cursor and
//!   advance one column.
//! - Backspace: move one column left and erase that cell.
//! - Enter: move to column 0 of the next row.
//! - Ctrl-C: quit.

use std::io::Write;

use uncurses::Terminal;
use uncurses::ansi::mode::{MouseEncoding, MouseMode};
use uncurses::event::{Event, EventSource, Key, KeyCode, KeyModifiers, MouseButton};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::WrapMode;

const HEADER: &str = "cursor_pad — arrows/mouse to move, type to write, Ctrl-C to quit";
const HEADER_ROWS: u16 = 1;

fn redraw<W: Write>(screen: &mut Screen<W>) -> std::io::Result<()> {
    let w = screen.width();
    let header = if (HEADER.len() as u16) <= w {
        HEADER.to_string()
    } else {
        HEADER.chars().take(w as usize).collect()
    };
    screen.set_str_with((0, 0), &header, WrapMode::Truncate, Style::default());
    Ok(())
}

fn clamp_to_screen<W: Write>(screen: &Screen<W>, x: u16, y: u16) -> (u16, u16) {
    let w = screen.width().saturating_sub(1);
    let h = screen.height().saturating_sub(1);
    (x.min(w), y.min(h))
}

struct App {
    term: Terminal<Stdin, Stdout>,
    screen: Screen<Stdout>,
    events: EventSource<Stdin>,
    cx: u16,
    cy: u16,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut term = Terminal::stdio();
        term.make_raw()?;
        let mut screen = Screen::new(term.output(), term.window_size().unwrap_or_default());

        screen.set_alt_screen(true)?;
        screen.set_cursor_visible(true)?;
        screen.set_mouse_mode(MouseMode::Normal, MouseEncoding::Sgr)?;

        let events = EventSource::new(term.input())?;

        Ok(Self {
            term,
            screen,
            events,
            cx: 0,
            cy: HEADER_ROWS,
        })
    }

    fn render(&mut self) -> std::io::Result<()> {
        redraw(&mut self.screen)?;
        self.screen.render()?;
        self.screen.set_cursor_position(self.cx, self.cy)?;
        self.screen.flush()
    }

    fn run(&mut self) -> std::io::Result<()> {
        self.render()?;

        loop {
            let ev = self.events.read()?;
            match ev {
                Event::KeyPress(Key {
                    code: KeyCode::Char('c'),
                    modifiers,
                    ..
                }) if modifiers.contains(KeyModifiers::CTRL) => break,
                Event::KeyPress(key) | Event::KeyRepeat(key) => match key.code {
                    KeyCode::Up => {
                        let (nx, ny) =
                            clamp_to_screen(&self.screen, self.cx, self.cy.saturating_sub(1));
                        self.cx = nx;
                        self.cy = ny;
                    }
                    KeyCode::Down => {
                        let (nx, ny) =
                            clamp_to_screen(&self.screen, self.cx, self.cy.saturating_add(1));
                        self.cx = nx;
                        self.cy = ny;
                    }
                    KeyCode::Left => {
                        let (nx, ny) =
                            clamp_to_screen(&self.screen, self.cx.saturating_sub(1), self.cy);
                        self.cx = nx;
                        self.cy = ny;
                    }
                    KeyCode::Right => {
                        let (nx, ny) =
                            clamp_to_screen(&self.screen, self.cx.saturating_add(1), self.cy);
                        self.cx = nx;
                        self.cy = ny;
                    }
                    KeyCode::Enter => {
                        let (nx, ny) = clamp_to_screen(&self.screen, 0, self.cy.saturating_add(1));
                        self.cx = nx;
                        self.cy = ny;
                    }
                    KeyCode::Backspace if self.cx > 0 => {
                        self.cx -= 1;
                        self.screen.set_str_with(
                            (self.cx, self.cy),
                            " ",
                            WrapMode::Truncate,
                            Style::default(),
                        );
                    }
                    _ => {
                        if let Some(text) = key.text.as_deref()
                            && !text.is_empty()
                        {
                            let end = self.screen.set_str_with(
                                (self.cx, self.cy),
                                text,
                                WrapMode::Truncate,
                                Style::default(),
                            );
                            let (nx, ny) = clamp_to_screen(&self.screen, end.x, end.y);
                            self.cx = nx;
                            self.cy = ny;
                        }
                    }
                },
                Event::MouseClick(m) if m.button == MouseButton::Left => {
                    let (nx, ny) = clamp_to_screen(&self.screen, m.x, m.y);
                    self.cx = nx;
                    self.cy = ny;
                }
                Event::Resize(ws) => {
                    self.screen.resize(ws.col, ws.row);
                    let (nx, ny) = clamp_to_screen(&self.screen, self.cx, self.cy);
                    self.cx = nx;
                    self.cy = ny;
                }
                _ => {}
            }

            self.render()?;
        }
        Ok(())
    }

    fn stop(&mut self) -> std::io::Result<()> {
        self.screen.reset()?;
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
