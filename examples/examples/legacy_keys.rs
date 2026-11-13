//! Change legacy decoder flags during a Program session.
//!
//! Run with `cargo run --example legacy_keys`. Press F2 to switch legacy LF
//! between Ctrl+J and Enter, then press Ctrl+J to see the decoded key.
//! Press Alt+[ or Alt+] to see an ambiguous legacy prefix resolve after idle.
//! An unterminated control string appears as unknown input after two seconds idle.
//! Press F3 to enable or disable that timeout through the shared event source.
//! Press q or Ctrl-C to quit.

use std::io;
use std::time::Duration;

use uncurses::buffer::Bounded;
use uncurses::event::{DecoderFlags, Event};
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

fn main() -> io::Result<()> {
    let mut program = Program::stdio()?;
    program.init()?;
    let result = run(&mut program);
    program.finish()?;
    result
}

fn run(program: &mut Program<Stdin, Stdout>) -> io::Result<()> {
    let mut string_timeout_enabled = true;
    let cols = program.screen().width();
    program.screen_mut().resize((cols, 1));
    program.screen_mut().set_str(
        (0, 0),
        "F2: LF mapping; F3: string timeout; Ctrl+J: test LF; q/Ctrl-C: quit.",
        Style::default(),
    );
    program
        .screen_mut()
        .insert_above("Legacy LF reads as Ctrl+J")?;
    program
        .screen_mut()
        .insert_above("Try Alt+[ and Alt+]: legacy prefixes resolve after idle.")?;
    program
        .screen_mut()
        .insert_above("Unterminated strings appear as unknown input after 2 seconds idle.")?;
    program.screen_mut().render()?;

    loop {
        match program.read_event()? {
            Event::KeyPress(key) if key.matches_any(["q", "ctrl+c"]) => return Ok(()),
            Event::KeyPress(key) if key.matches("f2") => {
                let mut flags = program.decoder_flags();
                flags.toggle(DecoderFlags::LF_IS_ENTER);
                program.set_decoder_flags(flags);
                let meaning = if flags.contains(DecoderFlags::LF_IS_ENTER) {
                    "Enter"
                } else {
                    "Ctrl+J"
                };
                program
                    .screen_mut()
                    .insert_above(&format!("Legacy LF reads as {meaning}"))?;
            }
            Event::KeyPress(key) if key.matches("f3") => {
                string_timeout_enabled = !string_timeout_enabled;
                let timeout = string_timeout_enabled.then_some(Duration::from_secs(2));
                program
                    .event_source()
                    .lock()
                    .unwrap()
                    .set_string_idle_timeout(timeout);
                let setting = if string_timeout_enabled {
                    "2 seconds"
                } else {
                    "disabled"
                };
                program
                    .screen_mut()
                    .insert_above(&format!("String idle timeout: {setting}"))?;
            }
            Event::KeyPress(key) => {
                program
                    .screen_mut()
                    .insert_above(&format!("Key: {key:?}"))?;
            }
            Event::Unknown(bytes) => {
                program
                    .screen_mut()
                    .insert_above(&format!("Unknown input: {bytes:?}"))?;
            }
            Event::Resize(_) => program.autoresize()?,
            _ => {}
        }
        program.screen_mut().render()?;
    }
}
