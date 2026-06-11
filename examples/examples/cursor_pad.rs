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

use uncurses::ansi::mode::{MouseEncoding, MouseMode};
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, MouseButton, Source};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
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

fn main() -> std::io::Result<()> {
    let state = enable_raw_mode(stdin(), stdout())?;
    let size = get_window_size(stdout()).unwrap_or_default();
    let mut screen = Screen::new(stdout()).with_size(size.col, size.row);

    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(true)?;
    screen.set_mouse_mode(MouseMode::Normal, MouseEncoding::Sgr)?;

    let mut cx: u16 = 0;
    let mut cy: u16 = HEADER_ROWS;

    redraw(&mut screen)?;
    screen.set_cursor_position(cx, cy)?;
    screen.render()?;
    screen.flush()?;

    let mut events = Source::new(stdin())?;
    let mut quit = false;
    while !quit {
        let ev = events.read()?;
        match ev {
            Event::KeyPress(Key {
                code: KeyCode::Char('c'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CTRL) => quit = true,
            Event::KeyPress(key) | Event::KeyRepeat(key) => match key.code {
                KeyCode::Up => {
                    let (nx, ny) = clamp_to_screen(&screen, cx, cy.saturating_sub(1));
                    cx = nx;
                    cy = ny;
                }
                KeyCode::Down => {
                    let (nx, ny) = clamp_to_screen(&screen, cx, cy.saturating_add(1));
                    cx = nx;
                    cy = ny;
                }
                KeyCode::Left => {
                    let (nx, ny) = clamp_to_screen(&screen, cx.saturating_sub(1), cy);
                    cx = nx;
                    cy = ny;
                }
                KeyCode::Right => {
                    let (nx, ny) = clamp_to_screen(&screen, cx.saturating_add(1), cy);
                    cx = nx;
                    cy = ny;
                }
                KeyCode::Enter => {
                    let (nx, ny) = clamp_to_screen(&screen, 0, cy.saturating_add(1));
                    cx = nx;
                    cy = ny;
                }
                KeyCode::Backspace if cx > 0 => {
                    cx -= 1;
                    screen.set_str_with((cx, cy), " ", WrapMode::Truncate, Style::default());
                }
                _ => {
                    if let Some(text) = key.text.as_deref()
                        && !text.is_empty()
                    {
                        let end = screen.set_str_with(
                            (cx, cy),
                            text,
                            WrapMode::Truncate,
                            Style::default(),
                        );
                        let (nx, ny) = clamp_to_screen(&screen, end.x, end.y);
                        cx = nx;
                        cy = ny;
                    }
                }
            },
            Event::MouseClick(m) if m.button == MouseButton::Left => {
                let (nx, ny) = clamp_to_screen(&screen, m.x, m.y);
                cx = nx;
                cy = ny;
            }
            Event::Resize(ws) => {
                screen.resize(ws.col, ws.row);
                let (nx, ny) = clamp_to_screen(&screen, cx, cy);
                cx = nx;
                cy = ny;
                redraw(&mut screen)?;
            }
            _ => {}
        }

        screen.render()?;
        screen.set_cursor_position(cx, cy)?;
        screen.flush()?;
    }

    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state)?;
    Ok(())
}
