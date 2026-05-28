//! Generic SGR (Select Graphic Rendition) sequence writer.
//!
//! Higher-level style/color helpers live in [`crate::style`] — this module
//! only provides the wire-format primitive.

use std::io::{self, Write};

/// Write an SGR sequence with mixed `;`- and `:`-separated parameters.
///
/// `params` is a slice of slices. The outer slice separates **primary
/// parameters** with `;`; each inner slice represents **sub-parameters**
/// for a single primary parameter, separated with `:` (per ECMA-48 / ITU
/// T.416 sub-parameter syntax).
///
/// An empty outer slice writes `ESC [ m` (reset). Empty inner slices are
/// skipped.
///
/// # Examples
///
/// ```ignore
/// // \x1b[1;31;4m  — bold, red, underline
/// write_sgr(w, &[&[1], &[31], &[4]])?;
///
/// // \x1b[38:2::10:20:30m  — truecolor fg via colon-form (ITU T.416)
/// write_sgr(w, &[&[38, 2, 0, 10, 20, 30]])?;
///
/// // \x1b[0;38:2:10:20:30;1m  — mixed semicolon and colon
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
