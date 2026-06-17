//! Compositional `uncurses` demo: a `Terminal` handle feeding a `Screen`
//! and a `EventSource`, with a startup `query`.
//!
//! Run with `cargo run --example terminal`. Opens the controlling
//! terminal in raw mode + alternate screen, queries the background color
//! once at startup, then echoes window size and an event counter until
//! you press `q` or Ctrl-C.
//!
//! Nothing is hidden behind a facade: the `Terminal` owns the device and
//! its raw-mode lifecycle, while its `Copy` halves drive `Screen`
//! (output) and `EventSource` (input). You own the loop.

use std::io;
use std::io::Write;
use std::time::Duration;

use uncurses::event::{Event, EventSource, Key, KeyCode, KeyModifiers, query};
use uncurses::screen::Screen;
use uncurses::terminal::Terminal;
use uncurses::text::WrapMode;

fn main() -> io::Result<()> {
    let mut term = Terminal::open()?;
    term.make_raw()?;

    let mut screen = Screen::new(term.output(), term.window_size().unwrap_or_default());
    let mut source = EventSource::new(term.input())?;

    screen.set_alt_screen(true);
    screen.set_cursor_visible(false);

    // One-shot capability query at startup (100ms budget): the request is
    // written through the screen's output and the reply is plucked off
    // the source; any user input meanwhile stays queued.
    let bg = source.query(
        &mut screen,
        &query::BACKGROUND_COLOR,
        Duration::from_millis(100),
    )?;

    // Drive the loop in a closure so teardown always runs, even on error.
    let result = (|| -> io::Result<()> {
        let (mut w, mut h) = (screen.width(), screen.height());
        let mut events = 0u64;
        loop {
            screen.set_str(
                (0, 0),
                "uncurses compositional demo — press q or Ctrl-C to quit",
                WrapMode::Truncate,
            );
            screen.set_str(
                (0, 1),
                &format!("size: {w}x{h}   background: {bg:?}   events: {events}      "),
                WrapMode::Truncate,
            );
            screen.present()?;

            match source.read()? {
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
                    screen.resize(ws.col, ws.row);
                    (w, h) = (screen.width(), screen.height());
                }
                _ => {}
            }
            events += 1;
        }
        Ok(())
    })();

    screen.reset();
    screen.flush()?;
    term.restore()?;
    result
}
