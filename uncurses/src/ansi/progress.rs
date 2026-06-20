//! Progress-bar OSC sequences.
//!
//! ## Category
//!
//! OSC 9;4 communicates taskbar or tab progress state: reset, indeterminate,
//! normal percentage, error percentage, and warning percentage.
//!
//! ## OSC framing
//!
//! All sequences use `ESC ] 9 ; 4 ; state [; percentage] BEL`. Percentages passed
//! to writers are clamped into `0..=100` before emission.
//!
//! ## Mode interaction
//!
//! These notifications are not controlled by terminal modes. Unsupported
//! terminals ignore the OSC string.

use std::io::{self, Write};

/// Reset or hide progress: exact bytes `ESC ] 9 ; 4 ; 0 BEL` (`b"\x1b]9;4;0\x07"`).
pub const RESET_PROGRESS_BAR: &[u8] = b"\x1b]9;4;0\x07";

/// Set indeterminate progress: exact bytes `ESC ] 9 ; 4 ; 3 BEL` (`b"\x1b]9;4;3\x07"`).
pub const SET_INDETERMINATE_PROGRESS_BAR: &[u8] = b"\x1b]9;4;3\x07";

fn clamp_percentage(p: i32) -> u8 {
    p.clamp(0, 100) as u8
}

/// Set normal progress with `ESC ] 9 ; 4 ; 1 ; <percentage> BEL`.
///
/// `percentage` is clamped to `0..=100` before it is emitted.
pub fn write_set_progress_bar<W: Write>(w: &mut W, percentage: i32) -> io::Result<()> {
    write!(w, "\x1b]9;4;1;{}\x07", clamp_percentage(percentage))
}

/// Set error progress with `ESC ] 9 ; 4 ; 2 ; <percentage> BEL`.
///
/// `percentage` is clamped to `0..=100` before it is emitted.
pub fn write_set_error_progress_bar<W: Write>(w: &mut W, percentage: i32) -> io::Result<()> {
    write!(w, "\x1b]9;4;2;{}\x07", clamp_percentage(percentage))
}

/// Set warning progress with `ESC ] 9 ; 4 ; 4 ; <percentage> BEL`.
///
/// `percentage` is clamped to `0..=100` before it is emitted.
pub fn write_set_warning_progress_bar<W: Write>(w: &mut W, percentage: i32) -> io::Result<()> {
    write!(w, "\x1b]9;4;4;{}\x07", clamp_percentage(percentage))
}
