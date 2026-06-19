//! Two layered cards. Press any key to swap their stacking order;
//! `q`, `Esc`, or `Ctrl-C` exits.

use std::io::Write;

use uncurses::buffer::SurfaceMut;
use uncurses::canvas::Canvas;
use uncurses::color::BasicColor;
use uncurses::event::{Event, EventSource, Key, KeyCode, KeyModifiers};
use uncurses::style::Style;
use uncurses::terminal::Terminal;
use uncurses::terminal::{Stdin, Stdout};

const CARD_W: u16 = 20;
const CARD_H: u16 = 10;
// Fits two stacked cards (rows 1..13) + footer row.
const VIEW_H: u16 = 15;

struct App {
    term: Terminal<Stdin, Stdout>,
    screen: Canvas<Stdout>,
    events: EventSource<Stdin>,
    flip: bool,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut term = Terminal::stdio();
        term.make_raw()?;
        let mut screen = Canvas::new(
            term.output(),
            (term.window_size().unwrap_or_default().col, VIEW_H),
        );
        screen.set_cursor_visible(false);
        let events = EventSource::new(term.input())?;

        Ok(Self {
            term,
            screen,
            events,
            flip: false,
        })
    }

    fn render(&mut self) -> std::io::Result<()> {
        redraw(&mut self.screen, self.flip);
        self.screen.present()
    }

    fn run(&mut self) -> std::io::Result<()> {
        self.render()?;

        loop {
            let ev = self.events.read()?;
            let mut dirty = false;
            match ev {
                Event::KeyPress(Key {
                    code: KeyCode::Char('q') | KeyCode::Escape,
                    modifiers,
                    ..
                }) if modifiers.is_empty() => break,
                Event::KeyPress(Key {
                    code: KeyCode::Char('c'),
                    modifiers,
                    ..
                }) if modifiers.contains(KeyModifiers::CTRL) => break,
                Event::KeyPress(_) => {
                    self.flip = !self.flip;
                    dirty = true;
                }
                Event::Resize(ws) => {
                    self.screen.resize(ws.col, VIEW_H);
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

fn redraw<W: Write>(screen: &mut Canvas<W>, flip: bool) {
    screen.clear();
    let w = screen.width();
    let h = screen.height();

    let footer = Style::default().fg(BasicColor::BrightBlack.into());
    let footer_text = "Press any key to swap the cards, or q to quit.";
    if h >= 2 {
        screen.set_str((2, h.saturating_sub(2)), footer_text, footer);
    }

    if w < CARD_W + 14 || h < CARD_H + 4 {
        return;
    }

    let border_a = Style::default().fg(BasicColor::BrightYellow.into()).bold();
    let border_b = Style::default().fg(BasicColor::BrightMagenta.into()).bold();

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

fn draw_card<W: Write>(screen: &mut Canvas<W>, x: u16, y: u16, label: &str, border: Style) {
    let w = CARD_W;
    let h = CARD_H;

    let blank = " ".repeat(w as usize - 2);
    // Erase interior with default bg so the lower card doesn't bleed through.
    for row in 1..h - 1 {
        screen.set_str((x + 1, y + row), &blank, uncurses::style::Style::default());
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
    screen.set_str((x, y), &top, border.clone());
    screen.set_str((x, y + h - 1), &bot, border.clone());
    for row in 1..h - 1 {
        screen.set_str((x, y + row), "│", border.clone());
        screen.set_str((x + w - 1, y + row), "│", border.clone());
    }

    // Centered label.
    let lw = label.chars().count() as u16;
    let lx = x + (w.saturating_sub(lw)) / 2;
    let ly = y + h / 2;
    screen.set_str((lx, ly), label, uncurses::style::Style::default());
}
