//! Draw to the screen only: an output-only program, no input.
//!
//! Something that only *writes* to the terminal still wants a
//! [`Program`](uncurses::program::Program) for its alt-screen and cursor
//! bookkeeping, and a [`Screen`](uncurses::screen::Screen) for the diff
//! renderer, but it has no reason to probe: there is no input loop to consume
//! the replies. Nothing is probed unless you ask, so simply never calling
//! [`query_capabilities`](uncurses::program::Program::query_capabilities) means
//! nothing is sent that could leak a reply on exit; the screen still applies the
//! environment-detected color profile so colors downsample correctly. This demo
//! animates a marquee and never reads an event.
//!
//! Run with `cargo run --example draw_only`. It plays for a few seconds
//! and restores the terminal on its own.

use std::thread::sleep;
use std::time::Duration;

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::color::Color;
use uncurses::program::{Program, ProgramOptions};
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

const FRAMES: u32 = 160;
const FRAME_TIME: Duration = Duration::from_millis(40);

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    // Output-only: no input loop, so skip the paste mode. `init` never probes
    // on its own, which suits this example: nothing would read the replies. The
    // env color profile is still applied so colors downsample correctly.
    program.init_with(ProgramOptions {
        bracketed_paste: false,
        ..ProgramOptions::default()
    })?;
    program.enter_alt_screen()?;
    program.hide_cursor()?;

    let result = play(&mut program);

    // Teardown: stage the staged modes off, flush, and restore the terminal.
    program.finish()?;
    result
}

fn play(program: &mut Program<Stdin, Stdout>) -> std::io::Result<()> {
    let label = "uncurses";
    let label_w = label.len() as u16;

    for frame in 0..FRAMES {
        // Manual autoresize: re-query the window and resize to follow it,
        // since there is no Resize event loop here.
        program.autoresize()?;

        program.screen_mut().clear();
        let w = program.screen().width();
        let h = program.screen().height();

        // Always show, at the top, how long until the demo exits.
        let remaining_ms = u64::from(FRAMES - frame) * FRAME_TIME.as_millis() as u64;
        let header = format!(
            "draw-only demo (Screen, no input) - exiting in {:.1}s",
            remaining_ms as f32 / 1000.0,
        );
        let dim = Style::default().fg(Color::BrightBlack);
        program.screen_mut().set_str((0, 0), &header, dim);

        if w >= label_w && h >= 3 {
            // Bounce the label left and right across the width.
            let travel = (w - label_w).max(1) as u32;
            let phase = frame % (2 * travel);
            let x = if phase < travel {
                phase
            } else {
                2 * travel - phase
            } as u16;

            // Cycle the color so the marquee shimmers.
            let palette = [
                Color::BrightRed,
                Color::BrightYellow,
                Color::BrightGreen,
                Color::BrightCyan,
                Color::BrightBlue,
                Color::BrightMagenta,
            ];
            let color = palette[(frame as usize / 4) % palette.len()];
            program
                .screen_mut()
                .set_str((x, h / 2), label, Style::default().bold().fg(color));
        }

        // `present` renders the diff and flushes it in one call.
        program.screen_mut().render()?;
        sleep(FRAME_TIME);
    }
    Ok(())
}
