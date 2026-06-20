//! Shell-integration prompt and command markers using OSC 133.
//!
//! ## Category
//!
//! OSC 133 marks prompt start, command input start, command execution, and
//! command completion. Terminals can use these markers to segment scrollback.
//!
//! ## OSC framing
//!
//! Writers use 7-bit OSC with BEL termination: `ESC ] 133 ; <mark> [;data] BEL`.
//! [`write_finalterm`] is the generic form; the other functions encode the
//! standard `A`, `B`, `C`, and `D` markers.
//!
//! ## Mode interaction
//!
//! These markers are not controlled by a DECSET mode. They are passive metadata
//! embedded in the output stream.

use std::io::{self, Write};

/// Emit a generic OSC 133 marker, `ESC ] 133 [;<param>...] BEL`.
///
/// Each string in `params` is appended as one semicolon-prefixed field. Use this for marker forms not covered by the specialized helpers.
pub fn write_finalterm<W: Write>(w: &mut W, params: &[&str]) -> io::Result<()> {
    w.write_all(b"\x1b]133")?;
    for p in params {
        w.write_all(b";")?;
        w.write_all(p.as_bytes())?;
    }
    w.write_all(b"\x07")
}

/// Mark the start of a prompt with exact bytes `ESC ] 133 ; A BEL`.
pub fn write_prompt_start<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b]133;A\x07")
}

/// Mark the end of the prompt and start of command input with exact bytes `ESC ] 133 ; B BEL`.
pub fn write_command_start<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b]133;B\x07")
}

/// Mark the moment command execution begins with exact bytes `ESC ] 133 ; C BEL`.
pub fn write_command_executed<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b]133;C\x07")
}

/// Mark command completion with `ESC ] 133 ; D [;exit-code] BEL`.
///
/// When `exit_code` is `None`, the exit-code field is omitted; otherwise the decimal code is appended after a semicolon.
pub fn write_command_finished<W: Write>(w: &mut W, exit_code: Option<i32>) -> io::Result<()> {
    match exit_code {
        Some(code) => write!(w, "\x1b]133;D;{code}\x07"),
        None => w.write_all(b"\x1b]133;D\x07"),
    }
}
