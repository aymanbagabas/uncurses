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

use uncurses::buffer::Bounded;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, MouseButton};
use uncurses::program::{MouseTracking, Program, ProgramOptions};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

const HEADER: &str = "cursor_pad — arrows/mouse to move, type to write, Ctrl-C to quit";
const HEADER_ROWS: u16 = 1;

fn redraw(screen: &mut Screen<Stdout>) -> std::io::Result<()> {
    let w = screen.width();
    let header = if (HEADER.len() as u16) <= w {
        HEADER.to_string()
    } else {
        HEADER.chars().take(w as usize).collect()
    };
    screen.set_str((0, 0), &header, Style::default());
    Ok(())
}

fn clamp_to_screen(screen: &Screen<Stdout>, x: u16, y: u16) -> (u16, u16) {
    let w = screen.width().saturating_sub(1);
    let h = screen.height().saturating_sub(1);
    (x.min(w), y.min(h))
}

struct App {
    program: Program<Stdin, Stdout>,
    cx: u16,
    cy: u16,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut program = Program::stdio()?;
        // Enable plain mouse tracking so left clicks reposition the cursor.
        program.init_with(ProgramOptions {
            mouse: Some(MouseTracking::empty()),
            ..ProgramOptions::default()
        })?;
        program.enter_alt_screen()?;
        program.show_cursor()?;

        Ok(Self {
            program,
            cx: 0,
            cy: HEADER_ROWS,
        })
    }

    fn render(&mut self) -> std::io::Result<()> {
        redraw(self.program.screen_mut())?;
        // Stage the caret position so render() leaves the cursor here, applied
        // atomically inside the frame's hide/sync bracket.
        self.program
            .screen_mut()
            .set_cursor_position((self.cx, self.cy));
        self.program.screen_mut().render()
    }

    fn run(&mut self) -> std::io::Result<()> {
        self.render()?;

        loop {
            let ev = self.program.read_event()?;
            match ev {
                Event::KeyPress(Key {
                    code: KeyCode::Char('c'),
                    modifiers,
                    ..
                }) if modifiers.contains(KeyModifiers::CTRL) => break,
                Event::KeyPress(key) | Event::KeyRepeat(key) => match key.code {
                    KeyCode::Up => {
                        let (nx, ny) = clamp_to_screen(
                            self.program.screen(),
                            self.cx,
                            self.cy.saturating_sub(1),
                        );
                        self.cx = nx;
                        self.cy = ny;
                    }
                    KeyCode::Down => {
                        let (nx, ny) = clamp_to_screen(
                            self.program.screen(),
                            self.cx,
                            self.cy.saturating_add(1),
                        );
                        self.cx = nx;
                        self.cy = ny;
                    }
                    KeyCode::Left => {
                        let (nx, ny) = clamp_to_screen(
                            self.program.screen(),
                            self.cx.saturating_sub(1),
                            self.cy,
                        );
                        self.cx = nx;
                        self.cy = ny;
                    }
                    KeyCode::Right => {
                        let (nx, ny) = clamp_to_screen(
                            self.program.screen(),
                            self.cx.saturating_add(1),
                            self.cy,
                        );
                        self.cx = nx;
                        self.cy = ny;
                    }
                    KeyCode::Enter => {
                        let (nx, ny) =
                            clamp_to_screen(self.program.screen(), 0, self.cy.saturating_add(1));
                        self.cx = nx;
                        self.cy = ny;
                    }
                    KeyCode::Backspace if self.cx > 0 => {
                        self.cx -= 1;
                        self.program.screen_mut().set_str(
                            (self.cx, self.cy),
                            " ",
                            Style::default(),
                        );
                    }
                    _ => {
                        if let Some(text) = key.text.as_deref()
                            && !text.is_empty()
                        {
                            let end = self.program.screen_mut().set_str(
                                (self.cx, self.cy),
                                text,
                                Style::default(),
                            );
                            let (nx, ny) = clamp_to_screen(self.program.screen(), end.x, end.y);
                            self.cx = nx;
                            self.cy = ny;
                        }
                    }
                },
                Event::MouseClick(m) if m.button == MouseButton::Left => {
                    let (nx, ny) = clamp_to_screen(self.program.screen(), m.x, m.y);
                    self.cx = nx;
                    self.cy = ny;
                }
                Event::Resize(ws) => {
                    self.program.screen_mut().resize((ws.col, ws.row));
                    let (nx, ny) = clamp_to_screen(self.program.screen(), self.cx, self.cy);
                    self.cx = nx;
                    self.cy = ny;
                }
                _ => {}
            }

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
