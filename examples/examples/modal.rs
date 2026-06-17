//! Modal toggle over a static background.
//!
//! The screen is anchored top-left and filled with paragraph text. A
//! fixed-size modal box can be toggled on and off with `space` or `m`.
//! When the modal is hidden, the text underneath becomes visible
//! again. `q`, `Esc`, or `Ctrl-C` exits.

use std::io::Write;

use uncurses::buffer::SurfaceMut;
use uncurses::cell::Cell;
use uncurses::color::BasicColor;
use uncurses::event::{Event, EventSource, Key};
use uncurses::layout::Rect;
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::Terminal;
use uncurses::terminal::{Stdin, Stdout};
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

/// Modal-toggle app. `start` enters raw mode and the alternate screen,
/// `run` drives the event loop, and `stop` restores the terminal.
struct App {
    term: Terminal<Stdin, Stdout>,
    screen: Screen<Stdout>,
    events: EventSource<Stdin>,
    modal_open: bool,
    quit_keys: [Key; 3],
    toggle_keys: [Key; 2],
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut term = Terminal::stdio();
        term.make_raw()?;
        let mut screen = Screen::new(term.output(), term.window_size().unwrap_or_default());
        screen.set_alt_screen(true);
        screen.set_cursor_visible(false);
        let events = EventSource::new(term.input())?;

        // Parse key bindings once. `Key: FromStr`, and `==` compares on
        // the canonical chord identity — so plain equality is the right
        // operator for keyboard-shortcut matching.
        Ok(Self {
            term,
            screen,
            events,
            modal_open: true,
            quit_keys: ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap()),
            toggle_keys: ["space", "m"].map(|s| s.parse().unwrap()),
        })
    }

    fn render(&mut self) -> std::io::Result<()> {
        redraw(&mut self.screen, self.modal_open);
        self.screen.present()
    }

    fn run(&mut self) -> std::io::Result<()> {
        self.render()?;

        loop {
            let ev = self.events.read()?;
            let mut dirty = false;
            match ev {
                Event::KeyPress(ref key) if self.quit_keys.contains(key) => break,
                Event::KeyPress(ref key) if self.toggle_keys.contains(key) => {
                    self.modal_open = !self.modal_open;
                    dirty = true;
                }
                Event::Resize(ws) => {
                    self.screen.resize(ws.col, ws.row);
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

    fn stop(&mut self) -> std::io::Result<()> {
        self.screen.reset();
        self.screen.flush()?;
        self.term.restore()
    }
}

fn main() -> std::io::Result<()> {
    let mut app = App::start()?;
    let result = app.run();
    app.stop()?;
    result
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
    let body = Style::default().fg(BasicColor::BrightBlack.into());
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
    let status = Style::default()
        .fg(BasicColor::Black.into())
        .bg(BasicColor::BrightWhite.into());
    screen.fill_rect(
        Rect::new(0, y, screen.width(), 1),
        &Cell::narrow(" ").style(status.clone()),
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
    let frame = Style::default()
        .fg(BasicColor::BrightWhite.into())
        .bg(BasicColor::Blue.into())
        .bold();
    let body = Style::default()
        .fg(BasicColor::BrightWhite.into())
        .bg(BasicColor::Blue.into());
    let hint = Style::default()
        .fg(BasicColor::BrightYellow.into())
        .bg(BasicColor::Blue.into())
        .italic();

    // Solid fill so background text never bleeds through the modal.
    screen.fill_rect(rect, &Cell::narrow(" ").style(body.clone()));

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
