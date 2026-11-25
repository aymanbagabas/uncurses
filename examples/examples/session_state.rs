//! Inspect owned session choices separately from terminal reports.
//!
//! Run with `cargo run -p examples --example session_state`.
//! Press `e` to enable keyboard enhancements and paste, `d` to disable them,
//! `b` to switch buffers, `r` to request reports, and `q` to exit.
//! Reports appear as ordinary events. Unsupported queries need no reply.

use std::io;

use uncurses::ansi::kitty::KittyKeyboardFlags;
use uncurses::ansi::mode::Mode;
use uncurses::buffer::SurfaceMut;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers};
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::terminal::{TtyInput, TtyOutput};
use uncurses::text::TextSurface;

fn run(program: &mut Program<TtyInput, TtyOutput>) -> io::Result<()> {
    program.init()?;
    program.enter_alt_screen()?;
    program.autoresize()?;
    loop {
        let selected = program.tracked_state();
        let reports = program.capabilities().clone();
        let keyboard = match selected.kitty_keyboard() {
            None => "undecided",
            Some(flags) if flags.is_empty() => "disabled",
            Some(_) => "enabled",
        };
        let lines = [
            "Session state: e enable, d disable, b buffer, r reports, q quit".to_owned(),
            format!(
                "buffer: {}",
                if selected.alt_screen() {
                    "alternate"
                } else {
                    "normal"
                }
            ),
            format!("keyboard: {keyboard}"),
            format!("paste choice: {:?}", selected.mode(Mode::BRACKETED_PASTE)),
            format!("reported keyboard: {:?}", reports.kitty_keyboard()),
            format!("reported resize: {:?}", reports.mode(Mode::IN_BAND_RESIZE)),
        ];
        let screen = program.screen_mut();
        screen.clear();
        for (row, line) in lines.iter().enumerate() {
            screen.set_str((0, row as u16), line, Style::default());
        }
        screen.render()?;
        match program.read_event()? {
            Event::KeyPress(Key {
                code: KeyCode::Char('q'),
                ..
            }) => return Ok(()),
            Event::KeyPress(Key {
                code: KeyCode::Char('c'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CTRL) => return Ok(()),
            Event::KeyPress(Key {
                code: KeyCode::Char('e'),
                ..
            }) => {
                program.set_kitty_keyboard(Some(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES))?;
                program.enable_bracketed_paste()?;
            }
            Event::KeyPress(Key {
                code: KeyCode::Char('d'),
                ..
            }) => {
                program.set_kitty_keyboard(None)?;
                program.disable_bracketed_paste()?;
            }
            Event::KeyPress(Key {
                code: KeyCode::Char('b'),
                ..
            }) => {
                if selected.alt_screen() {
                    program.exit_alt_screen()?;
                    let width = program.screen().size().width;
                    program.screen_mut().resize((width, 6));
                } else {
                    program.enter_alt_screen()?;
                    program.autoresize()?;
                }
            }
            Event::KeyPress(Key {
                code: KeyCode::Char('r'),
                ..
            }) => {
                program.request_kitty_keyboard()?;
                program.request_mode(Mode::IN_BAND_RESIZE)?;
            }
            Event::Resize(_) => program.autoresize()?,
            _ => {}
        }
    }
}

fn main() -> io::Result<()> {
    let mut program = Program::open()?;
    let result = run(&mut program);
    let cleanup = program.finish();
    result.and(cleanup)?;
    println!("Session closed");
    Ok(())
}
