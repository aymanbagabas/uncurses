//! iTerm2 proprietary protocol (OSC 1337).
//!
//! See: https://iterm2.com/documentation-escape-codes.html

use std::io::{self, Write};

/// Send an iTerm2 protocol message (`OSC 1337 ; payload ST`).
pub fn write_iterm2<W: Write>(w: &mut W, payload: &str) -> io::Result<()> {
    write!(w, "\x1b]1337;{payload}\x07")
}
