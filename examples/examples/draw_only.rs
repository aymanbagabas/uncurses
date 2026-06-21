//! Draw to the screen only: render with `Canvas`, no `Screen`, no input.
//!
//! A program that only *writes* to the terminal does not need `Screen` —
//! `Screen` is the bundle you reach for when you both read input and draw.
//! This one drives a [`Canvas`] (the renderer) over a raw-mode
//! [`Terminal`] directly: it animates a marquee and never reads an event.
//! Because nothing here queries the terminal, there are no replies to leak
//! on exit; we still flush the input queue at teardown so stray keystrokes
//! do not reach the shell.
//!
//! Run with `cargo run --example draw_only`. It plays for a few seconds
//! and restores the terminal on its own.

use std::io::Write;
use std::thread::sleep;
use std::time::Duration;

use uncurses::buffer::SurfaceMut;
use uncurses::canvas::Canvas;
use uncurses::color::BasicColor;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout, Terminal};
use uncurses::text::TextSurface;

const FRAMES: u32 = 160;
const FRAME_TIME: Duration = Duration::from_millis(40);

fn main() -> std::io::Result<()> {
    let mut term = Terminal::stdio();
    term.make_raw()?;

    // The renderer over the terminal's output half, sized to the window.
    let size = term.get_window_size().unwrap_or_default();
    let mut canvas = Canvas::new(term.output(), (size.col, size.row));
    canvas.set_alt_screen(true);
    canvas.set_cursor_visible(false);

    let result = play(&term, &mut canvas);

    // Teardown: reset the modes the canvas turned on, flush, discard any
    // keys typed while it played, and drop raw mode.
    canvas.reset();
    let _ = canvas.flush();
    flush_input(&term);
    term.restore()?;
    result
}

fn play(term: &Terminal<Stdin, Stdout>, canvas: &mut Canvas<Stdout>) -> std::io::Result<()> {
    let label = "uncurses";
    let label_w = label.len() as u16;

    for frame in 0..FRAMES {
        // Manual autoresize: re-query the window and resize to follow it,
        // since there is no Resize event loop here.
        if let Ok(ws) = term.get_window_size()
            && (ws.col, ws.row) != (canvas.width(), canvas.height())
        {
            canvas.resize(ws.col, ws.row);
        }

        canvas.clear();
        let w = canvas.width();
        let h = canvas.height();

        // Always show, at the top, how long until the demo exits.
        let remaining_ms = u64::from(FRAMES - frame) * FRAME_TIME.as_millis() as u64;
        let header = format!(
            "draw-only demo (Canvas, no input) - exiting in {:.1}s",
            remaining_ms as f32 / 1000.0,
        );
        let dim = Style::default().fg(BasicColor::BrightBlack);
        canvas.set_str((0, 0), &header, dim);

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
            canvas.set_str((x, h / 2), label, Style::default().bold().fg(color));
        }

        // `present` renders the diff and flushes it in one call.
        canvas.present()?;
        sleep(FRAME_TIME);
    }
    Ok(())
}

/// Discard any unread terminal input so keys typed during the animation do
/// not spill into the shell once raw mode is dropped.
#[cfg(unix)]
fn flush_input(term: &Terminal<Stdin, Stdout>) {
    use std::os::fd::{AsFd, AsRawFd};
    // SAFETY: tcflush just clears the input queue for a valid fd.
    unsafe {
        libc::tcflush(term.as_fd().as_raw_fd(), libc::TCIFLUSH);
    }
}

#[cfg(not(unix))]
fn flush_input(_term: &Terminal<Stdin, Stdout>) {}
