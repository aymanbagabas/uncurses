//! Modal toggle over a static background.
//!
//! The screen is anchored top-left and filled with paragraph text. A
//! fixed-size modal box can be toggled on and off with `space` or `m`.
//! When the modal is hidden, the text underneath becomes visible
//! again. `q`, `Esc`, or `Ctrl-C` exits.

use std::io::Write;

use uncurses::SurfaceMut;
use uncurses::cell::Cell;
use uncurses::color::BasicColor;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, Source};
use uncurses::layout::Rect;
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses::text::WrapMode;

const MODAL_W: u16 = 44;
const MODAL_H: u16 = 9;

const BACKGROUND: &[&str] = &[
    "Lorem ipsum dolor sit amet, consectetur adipiscing elit.",
    "Sed do eiusmod tempor incididunt ut labore et dolore",
    "magna aliqua. Ut enim ad minim veniam, quis nostrud",
    "exercitation ullamco laboris nisi ut aliquip ex ea",
    "commodo consequat. Duis aute irure dolor in reprehenderit",
    "in voluptate velit esse cillum dolore eu fugiat nulla",
    "pariatur. Excepteur sint occaecat cupidatat non proident,",
    "sunt in culpa qui officia deserunt mollit anim id est",
    "laborum. Curabitur pretium tincidunt lacus. Nulla gravida",
    "orci a odio. Nullam varius, turpis et commodo pharetra,",
    "est eros bibendum elit, nec luctus magna felis sollicitudin",
    "mauris. Integer in mauris eu nibh euismod gravida.",
    "Duis ac tellus et risus vulputate vehicula. Donec lobortis",
    "risus a elit. Etiam tempor. Ut ullamcorper, ligula eu",
    "tempor congue, eros est euismod turpis, id tincidunt sapien",
    "risus a quam. Maecenas fermentum consequat mi.",
];

fn main() -> std::io::Result<()> {
    let state = enable_raw_mode(stdin(), stdout())?;
    let size = get_window_size(stdout()).unwrap_or_default();
    let mut screen = Screen::new(stdout()).with_size(size.col, size.row);
    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(false)?;

    let mut events = Source::new(stdin())?;
    let mut modal_open = true;
    let mut quit = false;

    redraw(&mut screen, modal_open);
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
            Event::KeyPress(Key {
                code: KeyCode::Char(' ' | 'm'),
                ..
            }) => {
                modal_open = !modal_open;
                dirty = true;
            }
            Event::Resize(ws) => {
                screen.resize(ws.col, ws.row);
                dirty = true;
            }
            _ => {}
        }
        if dirty && !quit {
            redraw(&mut screen, modal_open);
            screen.render()?;
            screen.flush()?;
        }
    }

    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state)?;
    Ok(())
}

fn redraw<W: Write>(screen: &mut Screen<W>, modal_open: bool) {
    screen.clear();
    paint_background(screen);
    paint_status(screen, modal_open);
    if modal_open && let Some(rect) = modal_rect(screen) {
        paint_modal(screen, rect);
    }
}

fn paint_background<W: Write>(screen: &mut Screen<W>) {
    let w = screen.width();
    let h = screen.height();
    if w == 0 || h == 0 {
        return;
    }
    let body = Style::EMPTY.with_fg(BasicColor::BrightBlack.into());
    // Reserve the bottom row for the status line.
    let body_rows = h.saturating_sub(1);
    for y in 0..body_rows {
        let line = BACKGROUND[(y as usize) % BACKGROUND.len()];
        screen.set_str_with((0, y), line, WrapMode::Truncate, body.clone());
    }
}

fn paint_status<W: Write>(screen: &mut Screen<W>, modal_open: bool) {
    let h = screen.height();
    if h == 0 {
        return;
    }
    let y = h - 1;
    let status = Style::EMPTY
        .with_fg(BasicColor::Black.into())
        .with_bg(BasicColor::BrightWhite.into());
    screen.fill_rect(
        Rect::new(0, y, screen.width(), 1),
        &Cell::new(" ", 1).with_style(status.clone()),
    );
    let label = if modal_open {
        " modal: open    space/m: toggle    q: quit "
    } else {
        " modal: closed  space/m: toggle    q: quit "
    };
    screen.set_str_with((0, y), label, WrapMode::Truncate, status);
}

fn modal_rect<W: Write>(screen: &Screen<W>) -> Option<Rect> {
    let w = screen.width();
    let h = screen.height();
    if w < MODAL_W + 2 || h < MODAL_H + 2 {
        return None;
    }
    let x = (w - MODAL_W) / 2;
    let y = (h - MODAL_H) / 2;
    Some(Rect::new(x, y, MODAL_W, MODAL_H))
}

fn paint_modal<W: Write>(screen: &mut Screen<W>, rect: Rect) {
    let frame = Style::EMPTY
        .with_fg(BasicColor::BrightWhite.into())
        .with_bg(BasicColor::Blue.into())
        .with_bold();
    let body = Style::EMPTY
        .with_fg(BasicColor::BrightWhite.into())
        .with_bg(BasicColor::Blue.into());
    let hint = Style::EMPTY
        .with_fg(BasicColor::BrightYellow.into())
        .with_bg(BasicColor::Blue.into())
        .with_italic();

    // Solid fill so background text never bleeds through the modal.
    screen.fill_rect(rect, &Cell::new(" ", 1).with_style(body.clone()));

    let right = rect.x + rect.width - 1;
    let bottom = rect.y + rect.height - 1;
    // Borders.
    for x in (rect.x + 1)..right {
        screen.set_str_with((x, rect.y), "─", WrapMode::Truncate, frame.clone());
        screen.set_str_with((x, bottom), "─", WrapMode::Truncate, frame.clone());
    }
    for y in (rect.y + 1)..bottom {
        screen.set_str_with((rect.x, y), "│", WrapMode::Truncate, frame.clone());
        screen.set_str_with((right, y), "│", WrapMode::Truncate, frame.clone());
    }
    screen.set_str_with((rect.x, rect.y), "┌", WrapMode::Truncate, frame.clone());
    screen.set_str_with((right, rect.y), "┐", WrapMode::Truncate, frame.clone());
    screen.set_str_with((rect.x, bottom), "└", WrapMode::Truncate, frame.clone());
    screen.set_str_with((right, bottom), "┘", WrapMode::Truncate, frame.clone());

    // Title bar.
    let title = " Modal ";
    let title_x = rect.x + (rect.width - title.chars().count() as u16) / 2;
    screen.set_str_with((title_x, rect.y), title, WrapMode::Truncate, frame.clone());

    // Body wraps inside the modal's inner area.
    let inner = Rect::new(
        rect.x + 2,
        rect.y + 1,
        rect.width.saturating_sub(4),
        rect.height.saturating_sub(2),
    );
    let body_text = "This panel covers a fixed slice of the screen. \
                     The paragraph text behind it is hidden until the \
                     modal is dismissed.";
    screen.set_str_rect_with(inner, body_text, WrapMode::Wrap, body.clone());

    // Footer hint inside the modal.
    let footer = "press space / m to close";
    let fx = rect.x + (rect.width - footer.chars().count() as u16) / 2;
    screen.set_str_with((fx, bottom - 1), footer, WrapMode::Truncate, hint);
}
