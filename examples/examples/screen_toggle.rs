//! Inline / alt-screen toggle demo.
//!
//! Press `space` to switch between inline mode and the alternate
//! screen. `q`, `Esc` or `Ctrl-C` exits.

use std::io::Write;

use uncurses::SurfaceMut;
use uncurses::color::BasicColor;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, Source};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses::text::WrapMode;

fn main() -> std::io::Result<()> {
    let state = enable_raw_mode(stdin(), stdout())?;
    let size = get_window_size(stdout()).unwrap_or_default();
    // Inline mode: 4 rows is enough for the message + help.
    let mut screen = Screen::new(stdout()).with_size(size.col, 4);
    screen.set_cursor_visible(false)?;

    let mut events = Source::new(stdin())?;
    let mut alt = false;
    let mut quit = false;
    redraw(&mut screen, alt);
    screen.render()?;
    screen.flush()?;

    while !quit {
        let ev = events.read()?;
        match ev {
            Event::KeyPress(Key {
                code: KeyCode::Char('q') | KeyCode::Escape,
                modifiers,
                ..
            }) if modifiers.is_empty() => quit = true,
            Event::KeyPress(Key {
                code: KeyCode::Char('c'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CTRL) => quit = true,
            Event::KeyPress(Key {
                code: KeyCode::Char(' '),
                ..
            }) => {
                alt = !alt;
                if alt {
                    screen.resize(size.col, size.row.max(4));
                    screen.set_alt_screen(true)?;
                } else {
                    screen.set_alt_screen(false)?;
                    screen.resize(size.col, 4);
                }
                redraw(&mut screen, alt);
                screen.render()?;
                screen.flush()?;
            }
            Event::Resize(ws) => {
                if alt {
                    screen.resize(ws.col, ws.row);
                } else {
                    screen.resize(ws.col, 4);
                }
                redraw(&mut screen, alt);
                screen.render()?;
                screen.flush()?;
            }
            _ => {}
        }
    }

    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state)?;
    Ok(())
}

fn redraw<W: Write>(screen: &mut Screen<W>, alt: bool) {
    screen.clear();
    let mode = if alt {
        " alt-screen mode "
    } else {
        " inline mode "
    };
    let keyword = Style::EMPTY
        .with_fg(BasicColor::BrightCyan.into())
        .with_bg(BasicColor::Black.into())
        .bold();
    let help = Style::EMPTY.with_fg(BasicColor::BrightBlack.into());

    screen.set_str((2, 1), "You're in", WrapMode::Truncate);
    screen.set_str_with((12, 1), mode, WrapMode::Truncate, keyword);
    screen.set_str_with(
        (2, 3),
        "space: switch modes • q: quit",
        WrapMode::Truncate,
        help,
    );
}
