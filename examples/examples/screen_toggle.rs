//! Inline / alt-screen toggle demo.
//!
//! Press `space` to switch between inline mode and the alternate
//! screen. `q`, `Esc` or `Ctrl-C` exits. On Unix, `Ctrl-Z` suspends the
//! process and it resumes cleanly with `fg`.

use std::io::Write;

use uncurses::buffer::SurfaceMut;
use uncurses::color::BasicColor;
use uncurses::event::{Event, EventSource, Key, KeyCode, KeyModifiers};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::Terminal;
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
        screen.set_cursor_visible(false);
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

    fn render(&mut self) -> std::io::Result<()> {
        redraw(&mut self.screen, self.alt);
        self.screen.present()
    }

    fn run(&mut self) -> std::io::Result<()> {
        self.render()?;

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
                #[cfg(unix)]
                Event::KeyPress(Key {
                    code: KeyCode::Char('z'),
                    modifiers,
                    ..
                }) if modifiers.contains(KeyModifiers::CTRL) => {
                    self.suspend()?;
                    self.resume()?;
                    self.render()?;
                }
                Event::KeyPress(Key {
                    code: KeyCode::Space,
                    ..
                }) => {
                    self.alt = !self.alt;
                    if self.alt {
                        self.screen.resize(self.size_col, self.size_row.max(4));
                        self.screen.set_alt_screen(true);
                    } else {
                        self.screen.set_alt_screen(false);
                        self.screen.resize(self.size_col, 4);
                    }
                    self.render()?;
                }
                Event::Resize(ws) => {
                    if self.alt {
                        self.screen.resize(ws.col, ws.row);
                    } else {
                        self.screen.resize(ws.col, 4);
                    }
                    self.render()?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Suspend to job control (Unix-only): tear down the screen, drop
    /// raw mode, then send `SIGTSTP` to ourselves. The kernel pauses the
    /// process until a `SIGCONT` (e.g. `fg`) returns control here.
    #[cfg(unix)]
    fn suspend(&mut self) -> std::io::Result<()> {
        self.screen.reset();
        self.screen.flush()?;
        self.term.restore()?;
        // SAFETY: raise is async-signal-safe.
        unsafe { libc::raise(libc::SIGTSTP) };
        Ok(())
    }

    /// Resume after [`suspend`](Self::suspend): re-acquire raw mode,
    /// refit to the current window size, and reinstate the screen modes
    /// (the alternate screen is re-entered if it was active), then force
    /// a full repaint.
    #[cfg(unix)]
    fn resume(&mut self) -> std::io::Result<()> {
        self.term.make_raw()?;
        if let Ok(size) = self.term.window_size() {
            self.size_col = size.col;
            self.size_row = size.row;
            if self.alt {
                self.screen.resize(size.col, size.row.max(4));
            } else {
                self.screen.resize(size.col, 4);
            }
        }
        self.screen.restore();
        self.screen.invalidate();
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
