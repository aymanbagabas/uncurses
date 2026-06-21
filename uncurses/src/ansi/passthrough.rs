//! DCS passthrough wrappers for terminal multiplexers.
//!
//! ## Category
//!
//! Passthrough strings tunnel an inner escape sequence through an intermediate
//! terminal layer so the outer terminal receives it.
//!
//! ## DCS framing
//!
//! Both writers emit 7-bit DCS strings terminated by `ST` (`ESC \\`). The tmux
//! form prefixes `tmux;` and doubles literal ESC bytes inside the payload; the
//! screen form can split long payloads into multiple adjacent DCS strings.
//!
//! ## Mode interaction
//!
//! Passthrough is not controlled by an ANSI/DEC mode. It is a framing convention
//! around another sequence, so the inner sequence may have its own mode
//! requirements.

use std::io::{self, Write};

use crate::ansi::c0::ESC;

/// Wrap `seq` in one or more DCS passthrough strings for screen-style forwarding.
///
/// The basic frame is `ESC P <seq> ESC \`. When `limit > 0`, `seq` is split into chunks of at most `limit` bytes separated by `ESC \ ESC P`; `limit == 0` writes one frame.
pub fn write_screen_passthrough<W: Write>(w: &mut W, seq: &[u8], limit: usize) -> io::Result<()> {
    w.write_all(b"\x1bP")?;
    if limit > 0 {
        let mut i = 0;
        while i < seq.len() {
            let end = (i + limit).min(seq.len());
            w.write_all(&seq[i..end])?;
            if end < seq.len() {
                w.write_all(b"\x1b\\\x1bP")?;
            }
            i = end;
        }
    } else {
        w.write_all(seq)?;
    }
    w.write_all(b"\x1b\\")
}

/// Wrap `seq` in a tmux passthrough frame, `ESC P tmux ; <escaped-seq> ESC \`.
///
/// Each literal ESC byte inside `seq` is doubled so the intermediate layer passes it through to the outer terminal.
pub fn write_tmux_passthrough<W: Write>(w: &mut W, seq: &[u8]) -> io::Result<()> {
    w.write_all(b"\x1bPtmux;")?;
    for &b in seq {
        if b == ESC {
            w.write_all(&[ESC])?;
        }
        w.write_all(&[b])?;
    }
    w.write_all(b"\x1b\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tmux_passthrough() {
        let mut buf = Vec::new();
        write_tmux_passthrough(&mut buf, b"\x1b[31m").unwrap();
        assert_eq!(buf, b"\x1bPtmux;\x1b\x1b[31m\x1b\\");
    }

    #[test]
    fn test_screen_passthrough_chunked() {
        let mut buf = Vec::new();
        write_screen_passthrough(&mut buf, b"123456", 2).unwrap();
        assert_eq!(buf, b"\x1bP12\x1b\\\x1bP34\x1b\\\x1bP56\x1b\\");
    }
}
