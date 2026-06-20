//! Select Graphic Rendition (SGR) writer.
//!
//! ## Category
//!
//! SGR is the CSI `m` family used for rendition attributes such as reset,
//! intensity, underline, foreground color, and background color. Higher-level
//! style construction lives outside this module; this module writes the raw
//! parameter grammar.
//!
//! ## Parameter conventions
//!
//! The outer slice in [`write_sgr`] becomes semicolon-separated primary
//! parameters. Each inner slice becomes colon-separated subparameters, allowing
//! both classic SGR and colon-form color parameters.
//!
//! ## Mode interaction
//!
//! SGR attributes are not enabled by a separate mode. They change the terminal's
//! active rendition state until another SGR sequence resets or modifies it.

use std::io::{self, Write};

/// Write an SGR sequence using semicolon-separated groups and colon-separated subparameters.
///
/// The emitted format is `ESC [ <params> m`. The outer slice separates primary parameters with `;`; each non-empty inner slice is joined with `:`. An empty outer slice emits `ESC [ m` (SGR reset), and empty inner slices are skipped.
///
/// # Examples
///
/// ```rust,ignore
/// // ESC [ 1 ; 31 ; 4 m — bold, red, underline
/// write_sgr(w, &[&[1], &[31], &[4]])?;
///
/// // ESC [ 38 : 2 : 0 : 10 : 20 : 30 m — colon-form truecolor foreground
/// write_sgr(w, &[&[38, 2, 0, 10, 20, 30]])?;
///
/// // ESC [ 0 ; 38 : 2 : 10 : 20 : 30 ; 1 m — mixed groups
/// write_sgr(w, &[&[0], &[38, 2, 10, 20, 30], &[1]])?;
/// ```
pub fn write_sgr<W: Write>(w: &mut W, params: &[&[u16]]) -> io::Result<()> {
    w.write_all(b"\x1b[")?;
    let mut first = true;
    for group in params {
        if group.is_empty() {
            continue;
        }
        if !first {
            w.write_all(b";")?;
        }
        first = false;
        for (i, p) in group.iter().enumerate() {
            if i > 0 {
                w.write_all(b":")?;
            }
            write!(w, "{p}")?;
        }
    }
    w.write_all(b"m")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_is_reset() {
        let mut buf = Vec::new();
        write_sgr(&mut buf, &[]).unwrap();
        assert_eq!(buf, b"\x1b[m");
    }

    #[test]
    fn test_semicolon_only() {
        let mut buf = Vec::new();
        write_sgr(&mut buf, &[&[1], &[31], &[4]]).unwrap();
        assert_eq!(buf, b"\x1b[1;31;4m");
    }

    #[test]
    fn test_colon_subparams() {
        let mut buf = Vec::new();
        write_sgr(&mut buf, &[&[38, 2, 0, 10, 20, 30]]).unwrap();
        assert_eq!(buf, b"\x1b[38:2:0:10:20:30m");
    }

    #[test]
    fn test_mixed() {
        let mut buf = Vec::new();
        write_sgr(&mut buf, &[&[0], &[38, 2, 10, 20, 30], &[1]]).unwrap();
        assert_eq!(buf, b"\x1b[0;38:2:10:20:30;1m");
    }

    #[test]
    fn test_skip_empty_groups() {
        let mut buf = Vec::new();
        write_sgr(&mut buf, &[&[1], &[], &[4]]).unwrap();
        assert_eq!(buf, b"\x1b[1;4m");
    }
}
