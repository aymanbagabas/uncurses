//! Inline / alt-screen toggle demo.
//!
//! Press `space` to switch between inline mode and the alternate
//! screen. `q`, `Esc` or `Ctrl-C` exits. On Unix, `Ctrl-Z` suspends the
//! process and it resumes cleanly with `fg`.

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::color::Color;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers};
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

// Inline mode: 4 rows is enough for the message + help.
const INLINE_ROWS: u16 = 4;

struct App {
    program: Program<Stdin, Stdout>,
    alt: bool,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut program = Program::stdio()?;
        program.init()?;
        program.hide_cursor()?;
        let cols = program.screen().width();
        program.screen_mut().resize((cols, INLINE_ROWS));

        Ok(Self {
            program,
            alt: false,
        })
    }

    fn render(&mut self) -> std::io::Result<()> {
        redraw(&mut self.program, self.alt);
        self.program.screen_mut().render()
    }

    fn run(&mut self) -> std::io::Result<()> {
        self.render()?;

        loop {
            let ev = self.program.read_event()?;
            self.program.observe_event(&ev)?;
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
                    // refit the managed area to the current window.
                    self.program.suspend()?;
                    self.program.resume()?;
                    self.render()?;
                }
                Event::KeyPress(Key {
                    code: KeyCode::Space,
                    ..
                }) => {
                    self.alt = !self.alt;
                    if self.alt {
                        self.program.enter_alt_screen()?;
                        self.program.autoresize()?;
                    } else {
                        self.program.exit_alt_screen()?;
                        let cols = self.program.screen().width();
                        self.program.screen_mut().resize((cols, INLINE_ROWS));
                    }
                    self.render()?;
                }
                Event::Resize(ws) => {
                    if self.alt {
                        self.program.screen_mut().resize((ws.col, ws.row));
                    } else {
                        self.program.screen_mut().resize((ws.col, INLINE_ROWS));
                    }
                    self.render()?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn stop(self) -> std::io::Result<()> {
        self.program.finish()
    }
}

fn main() -> std::io::Result<()> {
    let mut app = App::start()?;
    let result = app.run();
    app.stop()?;
    result
}

fn redraw(program: &mut Program<Stdin, Stdout>, alt: bool) {
    program.screen_mut().clear();
    let mode = if alt {
        " alt-screen mode "
    } else {
        " inline mode "
    };
    let keyword = Style::default()
        .fg(Color::BrightCyan)
        .bg(Color::Black)
        .bold();
    let help = Style::default().fg(Color::BrightBlack);

    program
        .screen_mut()
        .set_str((2, 1), "You're in", uncurses::style::Style::default());
    program.screen_mut().set_str((12, 1), mode, keyword);
    program
        .screen_mut()
        .set_str((2, 3), "space: switch modes • q: quit", help);
}
