//! Windows Terminal / ConEmu progress-bar sequences (OSC 9 ; 4).
//!
//! See: <https://learn.microsoft.com/en-us/windows/terminal/tutorials/progress-bar-sequences>

use std::io::{self, Write};

/// Reset / hide the progress bar (`OSC 9;4;0 BEL`).
pub const RESET_PROGRESS_BAR: &[u8] = b"\x1b]9;4;0\x07";

/// Set the progress bar to the indeterminate state (`OSC 9;4;3 BEL`).
pub const SET_INDETERMINATE_PROGRESS_BAR: &[u8] = b"\x1b]9;4;3\x07";

fn clamp_percentage(p: i32) -> u8 {
    p.clamp(0, 100) as u8
}

/// Set the progress bar to a specific percentage in the default state.
pub fn write_set_progress_bar<W: Write>(w: &mut W, percentage: i32) -> io::Result<()> {
    write!(w, "\x1b]9;4;1;{}\x07", clamp_percentage(percentage))
}

/// Set the progress bar to a specific percentage in the error state.
pub fn write_set_error_progress_bar<W: Write>(w: &mut W, percentage: i32) -> io::Result<()> {
    write!(w, "\x1b]9;4;2;{}\x07", clamp_percentage(percentage))
}

/// Set the progress bar to a specific percentage in the warning state.
pub fn write_set_warning_progress_bar<W: Write>(w: &mut W, percentage: i32) -> io::Result<()> {
    write!(w, "\x1b]9;4;4;{}\x07", clamp_percentage(percentage))
}
