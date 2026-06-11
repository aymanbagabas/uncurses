//! Centered click-counter button.
//!
//! Press `enter` or `space`, or click the button with the mouse, to increment.
//! `q`, `Esc`, or `Ctrl-C` exits.

use std::io::Write;

use uncurses::SurfaceMut;
use uncurses::ansi::mode::{MouseEncoding, MouseMode};
use uncurses::color::BasicColor;
use uncurses::event::{Event, Key, MouseButton, Source};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses::text::WrapMode;

fn main() -> std::io::Result<()> {
    let state = enable_raw_mode(stdin(), stdout())?;
    let size = get_window_size(stdout()).unwrap_or_default();
    let mut screen = Screen::new(stdout()).with_size(size.col, size.row);
    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(false)?;
    screen.set_mouse_mode(MouseMode::Normal, MouseEncoding::Sgr)?;

    let mut events = Source::new(stdin())?;
    let mut count: u32 = 0;
    let mut quit = false;
    let mut button_rect;

    // Parse key bindings once. `Key: FromStr`, and `==` compares on
    // the canonical chord identity — so plain equality is the right
    // operator for keyboard-shortcut matching.
    let quit_keys: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
    let click_keys: [Key; 2] = ["enter", "space"].map(|s| s.parse().unwrap());

    redraw(&mut screen, count);
    button_rect = button_bounds(&screen, count);
    screen.render()?;
    screen.flush()?;

    while !quit {
        let ev = events.read()?;
        let mut dirty = false;
        match ev {
            Event::KeyPress(ref key) if quit_keys.contains(key) => quit = true,
            Event::KeyPress(ref key) if click_keys.contains(key) => {
                count = count.saturating_add(1);
                dirty = true;
            }
            Event::MouseClick(m) if m.button == MouseButton::Left && hit(button_rect, m.x, m.y) => {
                count = count.saturating_add(1);
                dirty = true;
            }
            Event::Resize(ws) => {
                screen.resize(ws.col, ws.row);
                dirty = true;
            }
            _ => {}
        }
        if dirty && !quit {
            redraw(&mut screen, count);
            button_rect = button_bounds(&screen, count);
            screen.render()?;
            screen.flush()?;
        }
    }

    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state)?;
    Ok(())
}

fn button_label(count: u32) -> String {
    let label = if count == 0 {
        "Click me!".to_string()
    } else {
        format!("Clicks: {count}")
    };
    format!("[ {label} ]")
}

fn button_bounds<W: Write>(screen: &Screen<W>, count: u32) -> Option<(u16, u16, u16)> {
    let w = screen.width();
    let h = screen.height();
    if w < 20 || h < 5 {
        return None;
    }
    let inner = button_label(count);
    let inner_w = inner.chars().count() as u16;
    let x = w.saturating_sub(inner_w) / 2;
    let y = h / 2;
    Some((x, y, inner_w))
}

fn hit(rect: Option<(u16, u16, u16)>, mx: u16, my: u16) -> bool {
    let Some((x, y, w)) = rect else {
        return false;
    };
    my == y && mx >= x && mx < x + w
}

fn redraw<W: Write>(screen: &mut Screen<W>, count: u32) {
    screen.clear();
    let w = screen.width();
    let h = screen.height();
    if w < 20 || h < 5 {
        return;
    }

    let inner = button_label(count);
    let inner_w = inner.chars().count() as u16;
    let x = w.saturating_sub(inner_w) / 2;
    let y = h / 2;

    let button = Style::EMPTY
        .fg(BasicColor::BrightWhite.into())
        .bg(BasicColor::Blue.into())
        .bold();
    screen.set_str_with((x, y), &inner, WrapMode::Truncate, button);

    let help = Style::EMPTY.fg(BasicColor::BrightBlack.into());
    let hint = "click / enter / space: increment • q: quit";
    let hint_w = hint.chars().count() as u16;
    let hx = w.saturating_sub(hint_w) / 2;
    screen.set_str_with((hx, h.saturating_sub(2)), hint, WrapMode::Truncate, help);
}
