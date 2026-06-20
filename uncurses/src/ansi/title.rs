//! Window title and icon-name OSC sequences.
//!
//! ## Category
//!
//! OSC 0, OSC 1, and OSC 2 update the terminal window title and/or icon name.
//!
//! ## OSC framing
//!
//! Writers use the 7-bit OSC introducer and `ST` terminator (`ESC \\`):
//! `ESC ] <code> ; <text> ESC \\`.
//!
//! ## Mode interaction
//!
//! Title controls are not gated by an ANSI/DEC mode. The payload is emitted
//! verbatim, so callers should avoid embedding string terminators in the text.

use std::io::{self, Write};

/// Set the window title and icon name with `ESC ] 0 ; <title> ESC \`.
///
/// OSC 0 sets both the icon name and the window title. `title` is emitted
/// verbatim and the sequence uses `ST` (`ESC \`) termination.
pub fn write_window_title_and_icon<W: Write>(w: &mut W, title: &str) -> io::Result<()> {
    write!(w, "\x1b]0;{title}\x1b\\")
}

/// Set the icon name with `ESC ] 1 ; <name> ESC \`.
///
/// `name` is emitted verbatim and the sequence uses `ST` (`ESC \`) termination.
pub fn write_icon_name<W: Write>(w: &mut W, name: &str) -> io::Result<()> {
    write!(w, "\x1b]1;{name}\x1b\\")
}

/// Set window title (OSC 2).
pub fn write_window_title<W: Write>(w: &mut W, title: &str) -> io::Result<()> {
    write!(w, "\x1b]2;{title}\x1b\\")
}
