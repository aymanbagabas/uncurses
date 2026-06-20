//! OSC 1337 payload framing.
//!
//! ## Category
//!
//! This module emits a generic OSC 1337 string with an application-defined
//! payload. The payload is not parsed or escaped.
//!
//! ## OSC framing
//!
//! The writer emits `ESC ] 1337 ; <payload> BEL` using the 7-bit OSC introducer
//! and BEL terminator.
//!
//! ## Mode interaction
//!
//! OSC 1337 payloads are not gated by an ANSI or DEC mode. Unsupported terminals
//! may ignore the string control.

use std::io::{self, Write};

/// Send a generic OSC 1337 message with `ESC ] 1337 ; <payload> BEL`.
///
/// `payload` is emitted verbatim; callers are responsible for avoiding embedded OSC terminators.
pub fn write_iterm2<W: Write>(w: &mut W, payload: &str) -> io::Result<()> {
    write!(w, "\x1b]1337;{payload}\x07")
}
