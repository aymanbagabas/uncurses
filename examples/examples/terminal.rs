//! High-level `uncurses` demo: a `Screen` facade feeding rendering and events.
//!
//! Run with `cargo run --example terminal`. Opens the controlling
//! terminal in raw mode + alternate screen, then echoes window size and
//! an event counter until you press `q` or Ctrl-C.

use std::io;

use uncurses::buffer::Bounded;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers};
use uncurses::program::Program;
use uncurses::terminal::{TtyInput, TtyOutput};
use uncurses::text::TextSurface;

struct App {
    program: Program<TtyInput, TtyOutput>,
}

impl App {
    fn start() -> io::Result<Self> {
        let mut program = Program::open()?;
        program.init()?;
        program.enter_alt_screen()?;
        program.hide_cursor()?;
        Ok(Self { program })
    }

    fn run(&mut self) -> io::Result<()> {
        let (mut w, mut h) = (
            self.program.screen().width(),
            self.program.screen().height(),
        );
        let mut events = 0u64;
        loop {
            self.program.screen_mut().set_str(
                (0, 0),
                "uncurses compositional demo — press q or Ctrl-C to quit",
                uncurses::style::Style::default(),
            );
            self.program.screen_mut().set_str(
                (0, 1),
                &format!("size: {w}x{h}   events: {events}      "),
                uncurses::style::Style::default(),
            );
            self.program.screen_mut().render()?;

            let ev = self.program.read_event()?;
            self.program.observe_event(&ev)?;
            match ev {
                Event::KeyPress(Key {
                    code: KeyCode::Char('q'),
                    ..
                }) => break,
                Event::KeyPress(Key {
                    code: KeyCode::Char('c'),
                    modifiers,
                    ..
                }) if modifiers.contains(KeyModifiers::CTRL) => break,
                Event::Resize(ws) => {
                    self.program.screen_mut().resize((ws.col, ws.row));
                    (w, h) = (
                        self.program.screen().width(),
                        self.program.screen().height(),
                    );
                }
                _ => {}
            }
            events += 1;
        }
        Ok(())
    }

    fn stop(self) -> io::Result<()> {
        self.program.finish()
    }
}

fn main() -> io::Result<()> {
    let mut app = App::start()?;
    let result = app.run();
    app.stop()?;
    result
}
