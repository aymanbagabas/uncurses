//! Character set selection (SCS) and locking-shift sequences.

use std::io::{self, Write};

/// Select a 94-character or 96-character set for a G-set designator.
///
/// `gset` selects the destination:
/// * `b'('` G0, `b')'` G1, `b'*'` G2, `b'+'` G3 (94-char sets).
/// * `b'-'` G1, `b'.'` G2, `b'/'` G3 (96-char sets).
///
/// `charset` is the identifier (e.g. `b'B'` USASCII, `b'0'` DEC Special
/// Drawing).
pub fn write_select_charset<W: Write>(w: &mut W, gset: u8, charset: u8) -> io::Result<()> {
    w.write_all(&[0x1b, gset, charset])
}

/// Locking Shift 1 Right — shift G1 into GR (`ESC ~`).
pub const LS1R: &[u8] = b"\x1b~";

/// Locking Shift 2 — shift G2 into GL (`ESC n`).
pub const LS2: &[u8] = b"\x1bn";

/// Locking Shift 2 Right — shift G2 into GR (`ESC }`).
pub const LS2R: &[u8] = b"\x1b}";

/// Locking Shift 3 — shift G3 into GL (`ESC o`).
pub const LS3: &[u8] = b"\x1bo";

/// Locking Shift 3 Right — shift G3 into GR (`ESC |`).
pub const LS3R: &[u8] = b"\x1b|";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scs_g0_usascii() {
        let mut buf = Vec::new();
        write_select_charset(&mut buf, b'(', b'B').unwrap();
        assert_eq!(buf, b"\x1b(B");
    }

    #[test]
    fn test_scs_g1_drawing() {
        let mut buf = Vec::new();
        write_select_charset(&mut buf, b')', b'0').unwrap();
        assert_eq!(buf, b"\x1b)0");
    }
}
