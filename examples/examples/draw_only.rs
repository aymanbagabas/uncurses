//! Draw to the screen only: an output-only `Screen`, no input.
//!
//! A program that only *writes* to the terminal still uses `Screen` for its
//! diff renderer and alt-screen/cursor bookkeeping, but it has no reason to
//! probe the terminal: there is no input loop to consume the replies. Passing
//! [`query_capabilities: false`](uncurses::screen::ScreenOptions::query_capabilities)
//! makes [`init_with`](uncurses::screen::Screen::init_with) skip the capability
//! queries entirely, so nothing is sent that could leak a reply on exit; the
//! screen still applies the environment-detected color profile so colors
//! downsample correctly. This demo animates a marquee and never reads an event.
//!
//! Run with `cargo run --example draw_only`. It plays for a few seconds
//! and restores the terminal on its own.

use std::thread::sleep;
use std::time::Duration;

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::color::BasicColor;
use uncurses::screen::{Screen, ScreenOptions};
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

const FRAMES: u32 = 160;
const FRAME_TIME: Duration = Duration::from_millis(40);

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    // Output-only: skip the capability queries (no input loop reads the
    // replies) and the paste mode (no input at all). The env color profile is
    // still applied so colors downsample correctly.
    screen.init_with(ScreenOptions {
        query_capabilities: false,
        bracketed_paste: false,
        ..ScreenOptions::default()
    })?;
    screen.enter_alt_screen()?;
    screen.hide_cursor()?;

    let result = play(&mut screen);

    // Teardown: stage the staged modes off, flush, and restore the terminal.
    screen.finish()?;
    result
}

fn play(screen: &mut Screen<Stdin, Stdout>) -> std::io::Result<()> {
    let label = "uncurses";
    let label_w = label.len() as u16;

    for frame in 0..FRAMES {
        // Manual autoresize: re-query the window and resize to follow it,
        // since there is no Resize event loop here.
        screen.autoresize()?;

        screen.clear();
        let w = screen.width();
        let h = screen.height();

        // Always show, at the top, how long until the demo exits.
        let remaining_ms = u64::from(FRAMES - frame) * FRAME_TIME.as_millis() as u64;
        let header = format!(
            "draw-only demo (Screen, no input) - exiting in {:.1}s",
            remaining_ms as f32 / 1000.0,
        );
        let dim = Style::default().fg(BasicColor::BrightBlack);
        screen.set_str((0, 0), &header, dim);

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
                BasicColor::BrightRed,
                BasicColor::BrightYellow,
                BasicColor::BrightGreen,
                BasicColor::BrightCyan,
                BasicColor::BrightBlue,
                BasicColor::BrightMagenta,
            ];
            let color = palette[(frame as usize / 4) % palette.len()];
            screen.set_str((x, h / 2), label, Style::default().bold().fg(color));
        }

        // `present` renders the diff and flushes it in one call.
        screen.present()?;
        sleep(FRAME_TIME);
    }
    Ok(())
}
