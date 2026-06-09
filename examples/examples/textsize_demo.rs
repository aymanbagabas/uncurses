//! Interactive text sizing protocol demo.
//!
//! Paints a column of OSC 66 labels at varied scales and lets the
//! host nudge them around. Arrow keys move the whole label group;
//! mouse clicks place a cursor; typing writes characters at the
//! cursor (free-form text written through the cell grid lives
//! alongside the OSC 66 regions). `Ctrl-C` quits.
//!
//! Terminals that don't implement the protocol render blank
//! cells where the labels would be — that's the deliberate
//! trade-off for keeping the library probe-free.

use std::io::Write;

use uncurses::ansi::mode::{MouseEncoding, MouseMode};
use uncurses::color::BasicColor;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, MouseButton, Source};
use uncurses::screen::{RegionId, Screen};
use uncurses::style::Style;
use uncurses::terminal::size::Winsize;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses::text::WrapMode;
use uncurses_textsize::{HAlign, TextSizing, VAlign};

const HEADER_ROWS: u16 = 2;

/// One labeled paint: a region id plus its offset from the group
/// anchor and the run that paints it.
struct Sample {
    id: RegionId,
    dx: u16,
    dy: u16,
    sizing: TextSizing,
}

fn samples() -> Vec<Sample> {
    vec![
        Sample {
            id: RegionId(1),
            dx: 0,
            dy: 0,
            sizing: TextSizing::new("scale 1: hello world"),
        },
        Sample {
            id: RegionId(2),
            dx: 0,
            dy: 2,
            sizing: TextSizing::new("scale 2").scale(2),
        },
        Sample {
            id: RegionId(3),
            dx: 0,
            dy: 5,
            sizing: TextSizing::new("scale 3").scale(3),
        },
        Sample {
            id: RegionId(4),
            dx: 0,
            dy: 9,
            sizing: TextSizing::new("declared width=2: 🐈").scale(2).width(2),
        },
        Sample {
            id: RegionId(5),
            dx: 0,
            dy: 12,
            sizing: TextSizing::new("half-height, centered")
                .fraction(1, 2)
                .align(HAlign::Center, VAlign::Center),
        },
    ]
}

/// Bounding box of the entire group, in cells, given a list of
/// samples whose offsets are relative to a (0, 0) anchor.
fn group_bbox(samples: &[Sample]) -> (u16, u16) {
    let mut w = 0u16;
    let mut h = 0u16;
    for s in samples {
        let (fw, fh) = s.sizing.footprint();
        w = w.max(s.dx.saturating_add(fw));
        h = h.max(s.dy.saturating_add(fh));
    }
    (w, h)
}

fn clamp_anchor(screen_w: u16, screen_h: u16, bbox: (u16, u16), x: u16, y: u16) -> (u16, u16) {
    let max_x = screen_w.saturating_sub(bbox.0);
    let max_y = screen_h.saturating_sub(bbox.1).max(HEADER_ROWS);
    let min_y = HEADER_ROWS;
    (x.min(max_x), y.clamp(min_y, max_y))
}

fn clamp_cursor(screen_w: u16, screen_h: u16, x: u16, y: u16) -> (u16, u16) {
    let max_x = screen_w.saturating_sub(1);
    let max_y = screen_h.saturating_sub(1);
    (x.min(max_x), y.min(max_y).max(HEADER_ROWS))
}

fn main() -> std::io::Result<()> {
    let state = enable_raw_mode(stdin(), stdout())?;
    let mut ws: Winsize = get_window_size(stdout()).unwrap_or_default();

    let mut screen = Screen::new(stdout()).with_size(ws.col, ws.row);
    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(true)?;
    screen.set_mouse_mode(MouseMode::Normal, MouseEncoding::Sgr)?;
    screen.flush()?;

    let samples = samples();
    let bbox = group_bbox(&samples);
    let (mut ax, mut ay) = clamp_anchor(screen.width(), screen.height(), bbox, 2, HEADER_ROWS);
    let mut cx: u16 = 0;
    let mut cy: u16 = HEADER_ROWS;
    let mut quit = false;

    redraw(&mut screen, &samples, ax, ay)?;
    screen.set_cursor_position(cx, cy)?;
    screen.render()?;
    screen.flush()?;

    let mut events = Source::new(stdin())?;
    while !quit {
        let mut dirty = false;
        match events.read()? {
            Event::KeyPress(Key {
                code: KeyCode::Char('c'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CTRL) => quit = true,
            Event::KeyPress(key) | Event::KeyRepeat(key) => match key.code {
                KeyCode::Up => {
                    let (nx, ny) = clamp_anchor(
                        screen.width(),
                        screen.height(),
                        bbox,
                        ax,
                        ay.saturating_sub(1),
                    );
                    if (nx, ny) != (ax, ay) {
                        ax = nx;
                        ay = ny;
                        dirty = true;
                    }
                }
                KeyCode::Down => {
                    let (nx, ny) = clamp_anchor(
                        screen.width(),
                        screen.height(),
                        bbox,
                        ax,
                        ay.saturating_add(1),
                    );
                    if (nx, ny) != (ax, ay) {
                        ax = nx;
                        ay = ny;
                        dirty = true;
                    }
                }
                KeyCode::Left => {
                    let (nx, ny) = clamp_anchor(
                        screen.width(),
                        screen.height(),
                        bbox,
                        ax.saturating_sub(1),
                        ay,
                    );
                    if (nx, ny) != (ax, ay) {
                        ax = nx;
                        ay = ny;
                        dirty = true;
                    }
                }
                KeyCode::Right => {
                    let (nx, ny) = clamp_anchor(
                        screen.width(),
                        screen.height(),
                        bbox,
                        ax.saturating_add(1),
                        ay,
                    );
                    if (nx, ny) != (ax, ay) {
                        ax = nx;
                        ay = ny;
                        dirty = true;
                    }
                }
                KeyCode::Enter => {
                    let (nx, ny) =
                        clamp_cursor(screen.width(), screen.height(), 0, cy.saturating_add(1));
                    cx = nx;
                    cy = ny;
                }
                KeyCode::Backspace if cx > 0 => {
                    cx -= 1;
                    screen.set_str_with((cx, cy), " ", WrapMode::Truncate, Style::EMPTY);
                }
                _ => {
                    if let Some(text) = key.text.as_deref()
                        && !text.is_empty()
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CTRL | KeyModifiers::ALT)
                    {
                        let end =
                            screen.set_str_with((cx, cy), text, WrapMode::Truncate, Style::EMPTY);
                        let (nx, ny) = clamp_cursor(screen.width(), screen.height(), end.x, end.y);
                        cx = nx;
                        cy = ny;
                    }
                }
            },
            Event::MouseClick(m) if m.button == MouseButton::Left => {
                let (nx, ny) = clamp_cursor(screen.width(), screen.height(), m.x, m.y);
                cx = nx;
                cy = ny;
            }
            Event::Resize(new_ws) => {
                ws = new_ws;
                screen.resize(ws.col, ws.row);
                let (nx, ny) = clamp_anchor(ws.col, ws.row, bbox, ax, ay);
                ax = nx;
                ay = ny;
                let (cnx, cny) = clamp_cursor(ws.col, ws.row, cx, cy);
                cx = cnx;
                cy = cny;
                dirty = true;
            }
            _ => {}
        }

        if dirty && !quit {
            redraw(&mut screen, &samples, ax, ay)?;
        }
        if !quit {
            screen.render()?;
            screen.set_cursor_position(cx, cy)?;
            screen.flush()?;
        }
    }

    for s in &samples {
        screen.clear_region(s.id);
    }
    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state)?;
    Ok(())
}

fn redraw<W: Write>(
    screen: &mut Screen<W>,
    samples: &[Sample],
    ax: u16,
    ay: u16,
) -> std::io::Result<()> {
    let header_style = Style::EMPTY.with_fg(BasicColor::BrightBlack.into());
    let blank = " ".repeat(screen.width() as usize);
    screen.set_str_with((0, 0), &blank, WrapMode::Truncate, header_style.clone());
    screen.set_str_with((0, 1), &blank, WrapMode::Truncate, header_style.clone());
    screen.set_str_with(
        (0, 0),
        "OSC 66 text-sizing demo  |  arrows: move group  click: cursor  type: write  C-c: quit",
        WrapMode::Truncate,
        header_style.clone(),
    );
    screen.set_str_with(
        (0, 1),
        &format!("anchor ({ax}, {ay})"),
        WrapMode::Truncate,
        header_style,
    );

    for s in samples {
        s.sizing.paint(screen, s.id, (ax + s.dx, ay + s.dy))?;
    }
    Ok(())
}
