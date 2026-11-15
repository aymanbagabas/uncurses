//! Type a greeting with a blinking cursor, pause, and repeat. Press any key to quit.
//!
//! Run with `cargo run --example hello_terminal`.

use std::io;
use std::time::{Duration, Instant};

use uncurses::ansi::cursor::CursorShape;
use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::event::Event;
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

fn main() -> io::Result<()> {
    let mut program = Program::stdio()?;
    let result = (|| {
        program.init()?;
        program.set_cursor_style(CursorShape::Block, true)?;
        let width = program.screen().width();
        program.screen_mut().resize((width, 3));

        loop {
            let screen = program.screen_mut();
            screen.clear();
            screen.set_cursor_position((2, 1));
            screen.render()?;
            if wait_for_key(&mut program, Duration::from_millis(300))? {
                return Ok(());
            }

            let mut greeting = String::new();
            for (column, character) in (2u16..).zip("Hello, terminal!".chars()) {
                greeting.push(character);
                let screen = program.screen_mut();
                screen.set_str((2, 1), &greeting, Style::new());
                screen.set_cursor_position((column + 1, 1));
                screen.render()?;
                if wait_for_key(&mut program, Duration::from_millis(45))? {
                    return Ok(());
                }
            }
            if wait_for_key(&mut program, Duration::from_secs(5))? {
                return Ok(());
            }
        }
    })();
    let finish = program.finish();
    result.and(finish)
}

fn wait_for_key(program: &mut Program<Stdin, Stdout>, delay: Duration) -> io::Result<bool> {
    let start = Instant::now();
    while let Some(remaining) = delay.checked_sub(start.elapsed()) {
        if !program.poll_event(Some(remaining))? {
            return Ok(false);
        }
        if matches!(program.read_event()?, Event::KeyPress(_)) {
            return Ok(true);
        }
    }
    Ok(false)
}
