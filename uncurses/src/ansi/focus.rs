//! Focus-event report sequences.
//!
//! ## Category
//!
//! When focus tracking is enabled, terminals send CSI `I` on focus gain and CSI
//! `O` on focus loss. The constants expose those inbound byte sequences, and the
//! writers can replay them in tests or recordings.
//!
//! ## CSI conventions
//!
//! Both reports use the 7-bit CSI introducer with no parameters: `ESC [ I` and
//! `ESC [ O`.
//!
//! ## Mode interaction
//!
//! Focus reports are controlled by [`Mode::FOCUS`](crate::ansi::mode::Mode::FOCUS)
//! (DEC private mode 1004). Applications enable the mode with
//! [`crate::ansi::mode::write_set_mode`] and then parse these reports from input.

use std::io::{self, Write};

/// Focus-gained report: exact bytes `ESC [ I` (`b"\x1b[I"`).
///
/// Terminals send this after focus tracking mode is enabled.
pub const FOCUS: &[u8] = b"\x1b[I";

/// Focus-lost report: exact bytes `ESC [ O` (`b"\x1b[O"`).
///
/// Terminals send this after focus tracking mode is enabled.
pub const BLUR: &[u8] = b"\x1b[O";

/// Write the focus-gained report bytes `ESC [ I`.
///
/// Useful for tests or replay; applications normally receive this from the terminal.
pub fn write_focus<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(FOCUS)
}

/// Write the focus-lost report bytes `ESC [ O`.
///
/// Useful for tests or replay; applications normally receive this from the terminal.
pub fn write_blur<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(BLUR)
}
