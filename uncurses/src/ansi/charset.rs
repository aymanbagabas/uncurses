//! Character-set designation and locking-shift sequences.
//!
//! ## Category
//!
//! This module emits SCS (`ESC` plus a G-set selector and charset designator)
//! and the locking-shift controls that map G1/G2/G3 into GL or GR.
//!
//! ## Escape format
//!
//! SCS is a short 7-bit ESC sequence rather than CSI/OSC/DCS:
//!
//! ```text
//! ESC ( B
//! ─┬─ ┬ ┬
//! intro G0 charset
//! ```
//!
//! The caller supplies the G-set selector byte and charset identifier exactly as
//! they should appear on the wire.
//!
//! ## Mode interaction
//!
//! Charset selection affects how subsequent bytes are rendered by terminals that
//! honor ISO-2022 character sets. It is independent of DECSET/DECRST modes.

use std::io::{self, Write};

/// Designate a character set with `ESC <gset> <charset>`.
///
/// `gset` is the designator selector byte: `b'('`/`b')'`/`b'*'`/`b'+'` for G0-G3 94-character sets, or `b'-'`/`b'.'`/`b'/'` for G1-G3 96-character sets. `charset` is emitted as the final designator byte, for example `b'B'` or `b'0'`.
pub fn write_select_charset<W: Write>(w: &mut W, gset: u8, charset: u8) -> io::Result<()> {
    w.write_all(&[0x1b, gset, charset])
}

/// Lock G1 into GR: exact bytes `ESC ~`.
///
/// Use after designating G-sets with [`write_select_charset`] when a terminal honors ISO-2022 locking shifts.
pub const LS1R: &[u8] = b"\x1b~";

/// Lock G2 into GL: exact bytes `ESC n`.
///
/// Use after designating G-sets with [`write_select_charset`] when a terminal honors ISO-2022 locking shifts.
pub const LS2: &[u8] = b"\x1bn";

/// Lock G2 into GR: exact bytes `ESC }`.
///
/// Use after designating G-sets with [`write_select_charset`] when a terminal honors ISO-2022 locking shifts.
pub const LS2R: &[u8] = b"\x1b}";

/// Lock G3 into GL: exact bytes `ESC o`.
///
/// Use after designating G-sets with [`write_select_charset`] when a terminal honors ISO-2022 locking shifts.
pub const LS3: &[u8] = b"\x1bo";

/// Lock G3 into GR: exact bytes `ESC |`.
///
/// Use after designating G-sets with [`write_select_charset`] when a terminal honors ISO-2022 locking shifts.
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
