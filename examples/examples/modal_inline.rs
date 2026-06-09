//! Modal-with-scrim over inline content.
//!
//! Inline mode (no alternate screen). The screen renders a fixed-size
//! surface anchored at the cursor's current position. Content flows
//! normally; pressing `m` opens a centered modal dialog that dims the
//! surface with a scrim and sits on top. `q`, `Esc`, or `Ctrl-C`
//! exits.

use std::io::Write;

use uncurses::SurfaceMut;
use uncurses::cell::Cell;
use uncurses::color::{BasicColor, Color};
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, Source};
use uncurses::layout::Rect;
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses::text::WrapMode;

const SURFACE_W: u16 = 60;
const SURFACE_H: u16 = 16;
const MODAL_W: u16 = 36;
const MODAL_H: u16 = 7;

fn main() -> std::io::Result<()> {
    let state = enable_raw_mode(stdin(), stdout())?;
    let term = get_window_size(stdout()).unwrap_or_default();
    let w = SURFACE_W.min(term.col.max(1));
    let h = SURFACE_H.min(term.row.max(1));
    let mut screen = Screen::new(stdout()).with_size(w, h);
    screen.set_cursor_visible(false)?;

    let mut events = Source::new(stdin())?;
    let mut modal_open = false;
    let mut quit = false;

    redraw(&mut screen, modal_open);
    screen.render()?;
    screen.flush()?;

    while !quit {
        let ev = events.read()?;
        let mut dirty = false;
        match ev {
            Event::KeyPress(Key {
                code: KeyCode::Char('q'),
                modifiers,
                ..
            }) if modifiers.is_empty() => quit = true,
            Event::KeyPress(Key {
                code: KeyCode::Escape,
                ..
            }) => {
                if modal_open {
                    modal_open = false;
                    dirty = true;
                } else {
                    quit = true;
                }
            }
            Event::KeyPress(Key {
                code: KeyCode::Char('c'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CTRL) => quit = true,
            Event::KeyPress(Key {
                code: KeyCode::Char('m'),
                ..
            }) => {
                modal_open = !modal_open;
                dirty = true;
            }
            Event::Resize(ws) => {
                let nw = SURFACE_W.min(ws.col.max(1));
                let nh = SURFACE_H.min(ws.row.max(1));
                screen.resize(nw, nh);
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
    paint_content(screen);
    if modal_open {
        paint_scrim(screen);
        if let Some(rect) = modal_rect(screen) {
            paint_modal(screen, rect);
        }
    }
}

fn paint_content<W: Write>(screen: &mut Screen<W>) {
    let cyan = Style::EMPTY.with_fg(BasicColor::BrightCyan.into());
    let plain = Style::EMPTY;
    let bullet_color = Style::EMPTY.with_fg(BasicColor::Yellow.into());

    screen.set_str_with(
        (0, 1),
        "Press m to toggle the modal.",
        WrapMode::Truncate,
        cyan,
    );
    screen.set_str_with(
        (0, 2),
        "Behind the modal there's regular flow content:",
        WrapMode::Truncate,
        plain,
    );
    for (i, label) in ["item 1", "item 2", "item 3", "item 4"].iter().enumerate() {
        let y = 3 + i as u16;
        screen.set_str_with(
            (0, y),
            "•",
            WrapMode::Truncate,
            Style::EMPTY.with_fg(BasicColor::Yellow.into()),
        );
        screen.set_str_with((2, y), label, WrapMode::Truncate, bullet_color.clone());
    }
}

fn paint_scrim<W: Write>(screen: &mut Screen<W>) {
    // Dim the surface with a uniform gray fill so the modal stands
    // out. The cells behind keep their content but the scrim's bg
    // wins because we overwrite each cell.
    let scrim = Style::EMPTY.with_bg(Color::Rgb(0x55, 0x55, 0x55));
    let bounds = Rect::new(0, 0, screen.width(), screen.height());
    screen.fill_rect(bounds, &Cell::narrow(" ").with_style(scrim));
}

fn modal_rect<W: Write>(screen: &Screen<W>) -> Option<Rect> {
    let w = screen.width();
    let h = screen.height();
    if w < MODAL_W || h < MODAL_H {
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
        .with_bg(BasicColor::Blue.into());

    screen.fill_rect(rect, &Cell::narrow(" ").with_style(body.clone()));

    let right = rect.x + rect.width - 1;
    let bottom = rect.y + rect.height - 1;
    for x in (rect.x + 1)..right {
        screen.set_str_with((x, rect.y), "─", WrapMode::Truncate, frame.clone());
        screen.set_str_with((x, bottom), "─", WrapMode::Truncate, frame.clone());
    }
    for y in (rect.y + 1)..bottom {
        screen.set_str_with((rect.x, y), "│", WrapMode::Truncate, frame.clone());
        screen.set_str_with((right, y), "│", WrapMode::Truncate, frame.clone());
    }
    // Rounded corners.
    screen.set_str_with((rect.x, rect.y), "╭", WrapMode::Truncate, frame.clone());
    screen.set_str_with((right, rect.y), "╮", WrapMode::Truncate, frame.clone());
    screen.set_str_with((rect.x, bottom), "╰", WrapMode::Truncate, frame.clone());
    screen.set_str_with((right, bottom), "╯", WrapMode::Truncate, frame.clone());

    let inner = Rect::new(
        rect.x + 2,
        rect.y + 1,
        rect.width.saturating_sub(4),
        rect.height.saturating_sub(2),
    );
    let title = Style::EMPTY
        .with_fg(BasicColor::BrightWhite.into())
        .with_bg(BasicColor::Blue.into())
        .with_bold();
    screen.set_str_rect_with(
        Rect::new(inner.x, inner.y, inner.width, 1),
        "Modal Dialog",
        WrapMode::Truncate,
        title,
    );
    let copy = "I sit on top thanks to z-index: 20.";
    screen.set_str_rect_with(
        Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height.saturating_sub(2),
        ),
        copy,
        WrapMode::Wrap,
        body,
    );
    screen.set_str_rect_with(
        Rect::new(inner.x, bottom - 1, inner.width, 1),
        "Press m or Esc to dismiss.",
        WrapMode::Truncate,
        hint,
    );
}
