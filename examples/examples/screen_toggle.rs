//! Inline / alt-screen toggle demo.
//!
//! Press `space` to switch between inline mode and the alternate
//! screen. `q`, `Esc` or `Ctrl-C` exits. On Unix, `Ctrl-Z` suspends the
//! process and it resumes cleanly with `fg`.

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::color::BasicColor;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

// Inline mode: 4 rows is enough for the message + help.
const INLINE_ROWS: u16 = 4;

struct App {
    screen: Screen<Stdin, Stdout>,
    alt: bool,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut screen = Screen::stdio()?;
        screen.init()?;
        screen.hide_cursor()?;
        let cols = screen.width();
        screen.resize((cols, INLINE_ROWS));

        Ok(Self { screen, alt: false })
    }

    fn render(&mut self) -> std::io::Result<()> {
        redraw(&mut self.screen, self.alt);
        self.screen.present()
    }

    fn run(&mut self) -> std::io::Result<()> {
        self.render()?;

        loop {
            let ev = self.screen.read_event()?;
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
                    // suspend()/resume() preserve the alt-screen state and
                    // refit the canvas to the current window.
                    self.screen.suspend()?;
                    self.screen.resume()?;
                    self.render()?;
                }
                Event::KeyPress(Key {
                    code: KeyCode::Space,
                    ..
                }) => {
                    self.alt = !self.alt;
                    if self.alt {
                        self.screen.enter_alt_screen()?;
                        self.screen.autoresize()?;
                    } else {
                        self.screen.exit_alt_screen()?;
                        let cols = self.screen.width();
                        self.screen.resize((cols, INLINE_ROWS));
                    }
                    self.render()?;
                }
                Event::Resize(ws) => {
                    if self.alt {
                        self.screen.resize((ws.col, ws.row));
                    } else {
                        self.screen.resize((ws.col, INLINE_ROWS));
                    }
                    self.render()?;
                }
                _ => {}
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

fn redraw(screen: &mut Screen<Stdin, Stdout>, alt: bool) {
    screen.clear();
    let mode = if alt {
        " alt-screen mode "
    } else {
        " inline mode "
    };
    let keyword = Style::default()
        .fg(BasicColor::BrightCyan)
        .bg(BasicColor::Black)
        .bold();
    let help = Style::default().fg(BasicColor::BrightBlack);

    screen.set_str((2, 1), "You're in", uncurses::style::Style::default());
    screen.set_str((12, 1), mode, keyword);
    screen.set_str((2, 3), "space: switch modes • q: quit", help);
}
