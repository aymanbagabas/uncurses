//! Draw to the screen only: animate a frame loop without acting on input.
//!
//! Some programs render without reacting to the keyboard: a dashboard, a
//! progress display, a splash screen. This one drives a [`Screen`] purely
//! as an output device. It paints a little bouncing marquee for a fixed
//! number of frames, sleeping between them. It still *drains* input each
//! frame (so terminal replies and stray keystrokes never leak to the
//! shell on exit), but it never reacts to it.
//!
//! Run with `cargo run --example draw_only`. It plays for a few seconds
//! and then restores the terminal on its own.

use std::thread::sleep;
use std::time::Duration;

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::color::BasicColor;
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

const FRAMES: u32 = 160;
const FRAME_TIME: Duration = Duration::from_millis(40);

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init()?;
    screen.enter_alt_screen()?;
    screen.hide_cursor()?;

    let result = play(&mut screen);

    // One teardown call restores every mode and the prior terminal state.
    screen.finish()?;
    result
}

fn play(screen: &mut Screen<Stdin, Stdout>) -> std::io::Result<()> {
    let label = "uncurses";
    let label_w = label.len() as u16;
    for frame in 0..FRAMES {
        // Drain and discard any pending input. We never act on it, but the
        // terminal still sends bytes (capability-query replies from init,
        // stray keystrokes); reading them keeps them out of the shell's
        // input once we exit.
        while screen.poll_event(Some(Duration::ZERO))? {
            let _ = screen.try_read_event();
        }

        // Refit to the current window each frame so the animation keeps up
        // with resizes even though we never read a Resize event.
        screen.autoresize()?;
        screen.clear();

        let w = screen.width();
        let h = screen.height();
        if w >= label_w && h >= 3 {
            // Bounce the label left and right across the width.
            let travel = (w - label_w).max(1) as u32;
            let phase = frame % (2 * travel);
            let x = if phase < travel { phase } else { 2 * travel - phase } as u16;

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
            let style = Style::default().bold().fg(color.into());
            screen.set_str((x, h / 2), label, style);

            let dim = Style::default().fg(BasicColor::BrightBlack.into());
            screen.set_str((0, 0), "draw-only demo (no input)", dim);
        }

        // `present` renders the diff and flushes it in one call.
        screen.present()?;
        sleep(FRAME_TIME);
    }
    Ok(())
}
