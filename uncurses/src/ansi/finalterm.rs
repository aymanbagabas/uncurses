//! FinalTerm shell-integration marks (OSC 133).
//!
//! See: <https://iterm2.com/documentation-shell-integration.html>

use std::io::{self, Write};

/// Emit a FinalTerm marker (`OSC 133 ; ps... ST`).
pub fn write_finalterm<W: Write>(w: &mut W, params: &[&str]) -> io::Result<()> {
    w.write_all(b"\x1b]133")?;
    for p in params {
        w.write_all(b";")?;
        w.write_all(p.as_bytes())?;
    }
    w.write_all(b"\x07")
}

/// Mark the start of a shell prompt (OSC 133 ; A).
pub fn write_prompt_start<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b]133;A\x07")
}

/// Mark the end of a shell prompt and start of user input (OSC 133 ; B).
pub fn write_command_start<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b]133;B\x07")
}

/// Mark the moment a command starts executing (OSC 133 ; C).
pub fn write_command_executed<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b]133;C\x07")
}

/// Mark the end of a command's output (OSC 133 ; D [;exit-code]).
pub fn write_command_finished<W: Write>(w: &mut W, exit_code: Option<i32>) -> io::Result<()> {
    match exit_code {
        Some(code) => write!(w, "\x1b]133;D;{code}\x07"),
        None => w.write_all(b"\x1b]133;D\x07"),
    }
}
