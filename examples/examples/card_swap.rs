//! Two layered cards. Press any key to swap their stacking order;
//! `q`, `Esc`, or `Ctrl-C` exits.

use std::io::Write;

use uncurses::SurfaceMut;
use uncurses::color::BasicColor;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, Source};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses::text::WrapMode;

const CARD_W: u16 = 20;
const CARD_H: u16 = 10;
// Fits two stacked cards (rows 1..13) + footer row.
const VIEW_H: u16 = 15;

fn main() -> std::io::Result<()> {
    let state = enable_raw_mode(stdin(), stdout())?;
    let size = get_window_size(stdout()).unwrap_or_default();
    let mut screen = Screen::new(stdout()).with_size(size.col, VIEW_H);
    screen.set_cursor_visible(false)?;

    let mut events = Source::new(stdin())?;
    let mut flip = false;
    let mut quit = false;

    redraw(&mut screen, flip);
    screen.render()?;
    screen.flush()?;

    while !quit {
        let ev = events.read()?;
        let mut dirty = false;
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
            Event::KeyPress(_) => {
                flip = !flip;
                dirty = true;
            }
            Event::Resize(ws) => {
                screen.resize(ws.col, VIEW_H);
                dirty = true;
            }
            _ => {}
        }
        if dirty && !quit {
            redraw(&mut screen, flip);
            screen.render()?;
            screen.flush()?;
        }
    }

    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state)?;
    Ok(())
}

fn redraw<W: Write>(screen: &mut Screen<W>, flip: bool) {
    screen.clear();
    let w = screen.width();
    let h = screen.height();

    let footer = Style::EMPTY.with_fg(BasicColor::BrightBlack.into());
    let footer_text = "Press any key to swap the cards, or q to quit.";
    if h >= 2 {
        screen.set_str_with(
            (2, h.saturating_sub(2)),
            footer_text,
            WrapMode::Truncate,
            footer,
        );
    }

    if w < CARD_W + 14 || h < CARD_H + 4 {
        return;
    }

    let border_a = Style::EMPTY
        .with_fg(BasicColor::BrightYellow.into())
        .with_bold();
    let border_b = Style::EMPTY
        .with_fg(BasicColor::BrightMagenta.into())
        .with_bold();

    // Card A at (3, 1); Card B offset by (10, 2) from A.
    let ax = 3u16;
    let ay = 1u16;
    let bx = ax + 10;
    let by = ay + 2;

    if flip {
        draw_card(screen, ax, ay, "Hello", border_a);
        draw_card(screen, bx, by, "Goodbye", border_b);
    } else {
        draw_card(screen, bx, by, "Goodbye", border_b);
        draw_card(screen, ax, ay, "Hello", border_a);
    }
}

fn draw_card<W: Write>(screen: &mut Screen<W>, x: u16, y: u16, label: &str, border: Style) {
    let w = CARD_W;
    let h = CARD_H;

    let blank = " ".repeat(w as usize - 2);
    // Erase interior with default bg so the lower card doesn't bleed through.
    for row in 1..h - 1 {
        screen.set_str((x + 1, y + row), &blank, WrapMode::Truncate);
    }

    // Rounded corners + horizontals + verticals.
    let top: String = std::iter::once('╭')
        .chain(std::iter::repeat_n('─', w as usize - 2))
        .chain(std::iter::once('╮'))
        .collect();
    let bot: String = std::iter::once('╰')
        .chain(std::iter::repeat_n('─', w as usize - 2))
        .chain(std::iter::once('╯'))
        .collect();
    screen.set_str_with((x, y), &top, WrapMode::Truncate, border.clone());
    screen.set_str_with((x, y + h - 1), &bot, WrapMode::Truncate, border.clone());
    for row in 1..h - 1 {
        screen.set_str_with((x, y + row), "│", WrapMode::Truncate, border.clone());
        screen.set_str_with(
            (x + w - 1, y + row),
            "│",
            WrapMode::Truncate,
            border.clone(),
        );
    }

    // Centered label.
    let lw = label.chars().count() as u16;
    let lx = x + (w.saturating_sub(lw)) / 2;
    let ly = y + h / 2;
    screen.set_str((lx, ly), label, WrapMode::Truncate);
}
