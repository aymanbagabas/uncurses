//! Change legacy decoder flags during a Program session.
//!
//! Run with `cargo run --example legacy_keys`. Press F2 to switch legacy LF
//! between Ctrl+J and Enter, then press Ctrl+J to see the decoded key.
//! Press q or Ctrl-C to quit.

use std::io;

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
    let cols = program.screen().width();
    program.screen_mut().resize((cols, 1));
    program.screen_mut().set_str(
        (0, 0),
        "F2: toggle LF; Ctrl+J: test it; q/Ctrl-C: quit.",
        Style::default(),
    );
    program
        .screen_mut()
        .insert_above("Legacy LF reads as Ctrl+J")?;
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
            Event::KeyPress(key) => {
                program
                    .screen_mut()
                    .insert_above(&format!("Key: {key:?}"))?;
            }
            Event::Resize(_) => program.autoresize()?,
            _ => {}
        }
        program.screen_mut().render()?;
    }
}
