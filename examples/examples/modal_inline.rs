//! Modal-with-scrim over inline content.
//!
//! Inline mode (no alternate screen). The screen renders a fixed-size
//! surface anchored at the cursor's current position. Content flows
//! normally; pressing `m` opens a centered modal dialog that dims the
//! surface with a scrim and sits on top. `q`, `Esc`, or `Ctrl-C`
//! exits.

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::cell::Cell;
use uncurses::color::{BasicColor, Color};
use uncurses::event::{Event, Key, KeyCode, KeyModifiers};
use uncurses::layout::Rect;
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::{TextSurface, WrapMode};

const SURFACE_W: u16 = 60;
const SURFACE_H: u16 = 16;
const MODAL_W: u16 = 36;
const MODAL_H: u16 = 7;

struct App {
    screen: Screen<Stdin, Stdout>,
    modal_open: bool,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut screen = Screen::stdio()?;
        screen.init()?;
        let w = SURFACE_W.min(screen.width().max(1));
        let h = SURFACE_H.min(screen.height().max(1));
        screen.resize((w, h));
        screen.hide_cursor()?;

        Ok(Self {
            screen,
            modal_open: false,
        })
    }

    fn render(&mut self) -> std::io::Result<()> {
        redraw(&mut self.screen, self.modal_open);
        self.screen.present()
    }

    fn run(&mut self) -> std::io::Result<()> {
        self.render()?;

        loop {
            let ev = self.screen.read()?;
            let mut dirty = false;
            match ev {
                Event::KeyPress(Key {
                    code: KeyCode::Char('q'),
                    modifiers,
                    ..
                }) if modifiers.is_empty() => break,
                Event::KeyPress(Key {
                    code: KeyCode::Escape,
                    ..
                }) => {
                    if self.modal_open {
                        self.modal_open = false;
                        dirty = true;
                    } else {
                        break;
                    }
                }
                Event::KeyPress(Key {
                    code: KeyCode::Char('c'),
                    modifiers,
                    ..
                }) if modifiers.contains(KeyModifiers::CTRL) => break,
                Event::KeyPress(Key {
                    code: KeyCode::Char('m'),
                    ..
                }) => {
                    self.modal_open = !self.modal_open;
                    dirty = true;
                }
                Event::Resize(ws) => {
                    let nw = SURFACE_W.min(ws.col.max(1));
                    let nh = SURFACE_H.min(ws.row.max(1));
                    self.screen.resize((nw, nh));
                    dirty = true;
                }
                _ => {}
            }
            if dirty {
                self.render()?;
            }
        }
        Ok(())
    }

    fn stop(self) -> std::io::Result<()> {
        self.screen.finish()
    }
}

fn main() -> std::io::Result<()> {
    let mut app = App::start()?;
    let result = app.run();
    app.stop()?;
    result
}

fn redraw(screen: &mut Screen<Stdin, Stdout>, modal_open: bool) {
    screen.clear();
    paint_content(screen);
    if modal_open {
        paint_scrim(screen);
        if let Some(rect) = modal_rect(screen) {
            paint_modal(screen, rect);
        }
    }
}

fn paint_content(screen: &mut Screen<Stdin, Stdout>) {
    let cyan = Style::default().fg(BasicColor::BrightCyan.into());
    let plain = Style::default();
    let bullet_color = Style::default().fg(BasicColor::Yellow.into());

    screen.set_str((0, 1), "Press m to toggle the modal.", cyan);
    screen.set_str(
        (0, 2),
        "Behind the modal there's regular flow content:",
        plain,
    );
    for (i, label) in ["item 1", "item 2", "item 3", "item 4"].iter().enumerate() {
        let y = 3 + i as u16;
        screen.set_str((0, y), "•", Style::default().fg(BasicColor::Yellow.into()));
        screen.set_str((2, y), label, bullet_color.clone());
    }
}

fn paint_scrim(screen: &mut Screen<Stdin, Stdout>) {
    // Dim the surface with a uniform gray fill so the modal stands
    // out. The cells behind keep their content but the scrim's bg
    // wins because we overwrite each cell.
    let scrim = Style::default().bg(Color::Rgb(0x55, 0x55, 0x55));
    let bounds = Rect::new(0, 0, screen.width(), screen.height());
    screen.fill_rect(bounds, &Cell::narrow(" ").style(scrim));
}

fn modal_rect(screen: &Screen<Stdin, Stdout>) -> Option<Rect> {
    let w = screen.width();
    let h = screen.height();
    if w < MODAL_W || h < MODAL_H {
        return None;
    }
    let x = (w - MODAL_W) / 2;
    let y = (h - MODAL_H) / 2;
    Some(Rect::new(x, y, MODAL_W, MODAL_H))
}

fn paint_modal(screen: &mut Screen<Stdin, Stdout>, rect: Rect) {
    let frame = Style::default()
        .fg(BasicColor::BrightWhite.into())
        .bg(BasicColor::Blue.into())
        .bold();
    let body = Style::default()
        .fg(BasicColor::BrightWhite.into())
        .bg(BasicColor::Blue.into());
    let hint = Style::default()
        .fg(BasicColor::BrightYellow.into())
        .bg(BasicColor::Blue.into());

    screen.fill_rect(rect, &Cell::narrow(" ").style(body.clone()));

    let right = rect.x + rect.width - 1;
    let bottom = rect.y + rect.height - 1;
    for x in (rect.x + 1)..right {
        screen.set_str((x, rect.y), "─", frame.clone());
        screen.set_str((x, bottom), "─", frame.clone());
    }
    for y in (rect.y + 1)..bottom {
        screen.set_str((rect.x, y), "│", frame.clone());
        screen.set_str((right, y), "│", frame.clone());
    }
    // Rounded corners.
    screen.set_str((rect.x, rect.y), "╭", frame.clone());
    screen.set_str((right, rect.y), "╮", frame.clone());
    screen.set_str((rect.x, bottom), "╰", frame.clone());
    screen.set_str((right, bottom), "╯", frame.clone());

    let inner = Rect::new(
        rect.x + 2,
        rect.y + 1,
        rect.width.saturating_sub(4),
        rect.height.saturating_sub(2),
    );
    let title = Style::default()
        .fg(BasicColor::BrightWhite.into())
        .bg(BasicColor::Blue.into())
        .bold();
    screen.set_str_rect(
        Rect::new(inner.x, inner.y, inner.width, 1),
        "Modal Dialog",
        title,
    );
    let copy = "I sit on top thanks to z-index: 20.";
    screen.set_str_rect_wrap(
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
    screen.set_str_rect(
        Rect::new(inner.x, bottom - 1, inner.width, 1),
        "Press m or Esc to dismiss.",
        hint,
    );
}
