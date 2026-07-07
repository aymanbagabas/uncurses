//! Modal toggle over a static background.
//!
//! The screen is anchored top-left and filled with paragraph text. A
//! fixed-size modal box can be toggled on and off with `space` or `m`.
//! When the modal is hidden, the text underneath becomes visible
//! again. `q`, `Esc`, or `Ctrl-C` exits.

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::cell::Cell;
use uncurses::color::Color;
use uncurses::event::{Event, Key};
use uncurses::layout::Rect;
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::{TextSurface, WrapMode};

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

/// Modal-toggle app. `start` enters raw mode and the alternate screen,
/// and `run` drives the event loop.
struct App {
    screen: Screen<Stdin, Stdout>,
    modal_open: bool,
    quit_keys: [Key; 3],
    toggle_keys: [Key; 2],
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut screen = Screen::stdio()?;
        screen.init()?;
        screen.enter_alt_screen()?;
        screen.hide_cursor()?;

        // Parse key bindings once. `Key: FromStr`, and `==` compares on
        // the canonical chord identity — so plain equality is the right
        // operator for keyboard-shortcut matching.
        Ok(Self {
            screen,
            modal_open: true,
            quit_keys: ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap()),
            toggle_keys: ["space", "m"].map(|s| s.parse().unwrap()),
        })
    }

    fn render(&mut self) -> std::io::Result<()> {
        redraw(&mut self.screen, self.modal_open);
        self.screen.render()
    }

    fn run(&mut self) -> std::io::Result<()> {
        self.render()?;

        loop {
            let ev = self.screen.read_event()?;
            self.screen.observe_event(&ev)?;
            let mut dirty = false;
            match ev {
                Event::KeyPress(ref key) if self.quit_keys.contains(key) => break,
                Event::KeyPress(ref key) if self.toggle_keys.contains(key) => {
                    self.modal_open = !self.modal_open;
                    dirty = true;
                }
                Event::Resize(ws) => {
                    self.screen.resize((ws.col, ws.row));
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
    paint_background(screen);
    paint_status(screen, modal_open);
    if modal_open && let Some(rect) = modal_rect(screen) {
        paint_modal(screen, rect);
    }
}

fn paint_background(screen: &mut Screen<Stdin, Stdout>) {
    let w = screen.width();
    let h = screen.height();
    if w == 0 || h == 0 {
        return;
    }
    let body = Style::default().fg(Color::BrightBlack);
    // Reserve the bottom row for the status line.
    let body_rows = h.saturating_sub(1);
    for y in 0..body_rows {
        let line = BACKGROUND[(y as usize) % BACKGROUND.len()];
        screen.set_str((0, y), line, body.clone());
    }
}

fn paint_status(screen: &mut Screen<Stdin, Stdout>, modal_open: bool) {
    let h = screen.height();
    if h == 0 {
        return;
    }
    let y = h - 1;
    let status = Style::default().fg(Color::Black).bg(Color::BrightWhite);
    screen.fill_rect(
        Rect::new(0, y, screen.width(), 1),
        &Cell::narrow(" ").style(status.clone()),
    );
    let label = if modal_open {
        " modal: open    space/m: toggle    q: quit "
    } else {
        " modal: closed  space/m: toggle    q: quit "
    };
    screen.set_str((0, y), label, status);
}

fn modal_rect(screen: &Screen<Stdin, Stdout>) -> Option<Rect> {
    let w = screen.width();
    let h = screen.height();
    if w < MODAL_W + 2 || h < MODAL_H + 2 {
        return None;
    }
    let x = (w - MODAL_W) / 2;
    let y = (h - MODAL_H) / 2;
    Some(Rect::new(x, y, MODAL_W, MODAL_H))
}

fn paint_modal(screen: &mut Screen<Stdin, Stdout>, rect: Rect) {
    let frame = Style::default()
        .fg(Color::BrightWhite)
        .bg(Color::Blue)
        .bold();
    let body = Style::default().fg(Color::BrightWhite).bg(Color::Blue);
    let hint = Style::default()
        .fg(Color::BrightYellow)
        .bg(Color::Blue)
        .italic();

    // Solid fill so background text never bleeds through the modal.
    screen.fill_rect(rect, &Cell::narrow(" ").style(body.clone()));

    let right = rect.x + rect.width - 1;
    let bottom = rect.y + rect.height - 1;
    // Borders.
    for x in (rect.x + 1)..right {
        screen.set_str((x, rect.y), "─", frame.clone());
        screen.set_str((x, bottom), "─", frame.clone());
    }
    for y in (rect.y + 1)..bottom {
        screen.set_str((rect.x, y), "│", frame.clone());
        screen.set_str((right, y), "│", frame.clone());
    }
    screen.set_str((rect.x, rect.y), "┌", frame.clone());
    screen.set_str((right, rect.y), "┐", frame.clone());
    screen.set_str((rect.x, bottom), "└", frame.clone());
    screen.set_str((right, bottom), "┘", frame.clone());

    // Title bar.
    let title = " Modal ";
    let title_x = rect.x + (rect.width - title.chars().count() as u16) / 2;
    screen.set_str((title_x, rect.y), title, frame.clone());

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
    screen.set_str_rect_wrap(inner, body_text, WrapMode::Wrap, body.clone());

    // Footer hint inside the modal.
    let footer = "press space / m to close";
    let fx = rect.x + (rect.width - footer.chars().count() as u16) / 2;
    screen.set_str((fx, bottom - 1), footer, hint);
}
