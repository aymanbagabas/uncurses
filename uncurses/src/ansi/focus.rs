//! Focus tracking constants.
//!
//! Terminals send these sequences to applications when focus mode (DECSET 1004)
//! is enabled. They are normally *received* by the input parser, but exposing
//! the byte sequences is useful for tests and for replaying recordings.

use std::io::{self, Write};

/// Sequence sent when the terminal gains focus (`CSI I`).
pub const FOCUS: &[u8] = b"\x1b[I";

/// Sequence sent when the terminal loses focus (`CSI O`).
pub const BLUR: &[u8] = b"\x1b[O";

pub fn write_focus<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(FOCUS)
}

pub fn write_blur<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(BLUR)
}
