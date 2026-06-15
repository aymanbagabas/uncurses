//! Inline / alt-screen toggle demo.
//!
//! Press `space` to switch between inline mode and the alternate
//! screen. `q`, `Esc` or `Ctrl-C` exits.

use std::io::Write;

use uncurses::SurfaceMut;
use uncurses::Terminal;
use uncurses::color::BasicColor;
use uncurses::event::{Event, EventSource, Key, KeyCode, KeyModifiers};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::WrapMode;

struct App {
    term: Terminal<Stdin, Stdout>,
    screen: Screen<Stdout>,
    events: EventSource<Stdin>,
    alt: bool,
    size_col: u16,
    size_row: u16,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut term = Terminal::stdio();
        term.make_raw()?;
        let size = term.window_size().unwrap_or_default();
        // Inline mode: 4 rows is enough for the message + help.
        let mut screen = Screen::new(term.output(), (size.col, 4));
        screen.set_cursor_visible(false)?;
        let events = EventSource::new(term.input())?;

        Ok(Self {
            term,
            screen,
            events,
            alt: false,
            size_col: size.col,
            size_row: size.row,
        })
    }

    fn render(&mut self) {
        redraw(&mut self.screen, self.alt);
    }

    fn run(&mut self) -> std::io::Result<()> {
        self.render();
        self.screen.render()?;
        self.screen.flush()?;

        loop {
            let ev = self.events.read()?;
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
                Event::KeyPress(Key {
                    code: KeyCode::Space,
                    ..
                }) => {
                    self.alt = !self.alt;
                    if self.alt {
                        self.screen.resize(self.size_col, self.size_row.max(4));
                        self.screen.set_alt_screen(true)?;
                    } else {
                        self.screen.set_alt_screen(false)?;
                        self.screen.resize(self.size_col, 4);
                    }
                    self.render();
                    self.screen.render()?;
                    self.screen.flush()?;
                }
                Event::Resize(ws) => {
                    if self.alt {
                        self.screen.resize(ws.col, ws.row);
                    } else {
                        self.screen.resize(ws.col, 4);
                    }
                    self.render();
                    self.screen.render()?;
                    self.screen.flush()?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn stop(&mut self) -> std::io::Result<()> {
        self.screen.reset()?;
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

fn redraw<W: Write>(screen: &mut Screen<W>, alt: bool) {
    screen.clear();
    let mode = if alt {
        " alt-screen mode "
    } else {
        " inline mode "
    };
    let keyword = Style::default()
        .fg(BasicColor::BrightCyan.into())
        .bg(BasicColor::Black.into())
        .bold();
    let help = Style::default().fg(BasicColor::BrightBlack.into());

    screen.set_str((2, 1), "You're in", WrapMode::Truncate);
    screen.set_str_with((12, 1), mode, WrapMode::Truncate, keyword);
    screen.set_str_with(
        (2, 3),
        "space: switch modes • q: quit",
        WrapMode::Truncate,
        help,
    );
}
