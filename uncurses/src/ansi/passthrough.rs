//! DCS passthrough wrappers for tmux and GNU screen.

use std::io::{self, Write};

use crate::ansi::c0::ESC;

/// Wrap a sequence so the outer terminal sees it through GNU screen
/// (`DCS <data> ST`).
///
/// `limit` (>0) chunks the data into `limit`-byte segments separated by
/// `ST DCS` pairs, mirroring the 768-byte string limit imposed by screen
/// since 2014.
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

/// Wrap a sequence so the outer terminal sees it through tmux
/// (`DCS tmux ; <escaped> ST`).
///
/// All ESC bytes inside `seq` are doubled.
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
