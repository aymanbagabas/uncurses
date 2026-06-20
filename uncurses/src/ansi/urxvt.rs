//! OSC 777 extension-message framing.
//!
//! ## Category
//!
//! This module writes a generic OSC 777 extension invocation: extension name plus
//! optional semicolon-separated parameters.
//!
//! ## OSC framing
//!
//! The emitted format is `ESC ] 777 ; <extension> [;param...] BEL`. Parameters
//! are written verbatim in the order supplied.
//!
//! ## Mode interaction
//!
//! OSC 777 messages are not controlled by ANSI or DEC modes. Unsupported
//! terminals ignore the OSC string.

use std::io::{self, Write};

/// Invoke an OSC 777 extension with `ESC ] 777 ; <extension> [;param...] BEL`.
///
/// `extension` and each parameter are emitted verbatim as semicolon-separated fields.
pub fn write_urxvt_ext<W: Write>(w: &mut W, extension: &str, params: &[&str]) -> io::Result<()> {
    write!(w, "\x1b]777;{extension}")?;
    for p in params {
        write!(w, ";{p}")?;
    }
    w.write_all(b"\x07")
}
