//! Key-modifier option controls.
//!
//! ## Category
//!
//! This module emits CSI `m` private controls for modifying or querying keyboard
//! modifier reporting resources, including the common modifyOtherKeys resource.
//!
//! ## CSI format
//!
//! Set/reset uses `ESC [ > resource [;value] m`; query uses `ESC [ ? resource m`.
//! The fixed constants cover resource `4` with values `1`, `2`, reset, and query.
//!
//! ## Mode interaction
//!
//! These controls manage keyboard-reporting behavior independently from the
//! progressive keyboard enhancement stack in [`crate::ansi::kitty`].

use std::io::{self, Write};

/// Enable modifyOtherKeys resource `4` at value `1`: exact bytes `ESC [ > 4 ; 1 m` (`b"\x1b[>4;1m"`).
pub const SET_MODIFY_OTHER_KEYS_1: &[u8] = b"\x1b[>4;1m";

/// Enable modifyOtherKeys resource `4` at value `2`: exact bytes `ESC [ > 4 ; 2 m` (`b"\x1b[>4;2m"`).
pub const SET_MODIFY_OTHER_KEYS_2: &[u8] = b"\x1b[>4;2m";

/// Reset modifyOtherKeys resource `4`: exact bytes `ESC [ > 4 m` (`b"\x1b[>4m"`).
pub const RESET_MODIFY_OTHER_KEYS: &[u8] = b"\x1b[>4m";

/// Query modifyOtherKeys resource `4`: exact bytes `ESC [ ? 4 m` (`b"\x1b[?4m"`).
pub const QUERY_MODIFY_OTHER_KEYS: &[u8] = b"\x1b[?4m";

/// Set or reset a key-modifier resource with `ESC [ > <resource> [;<value>] m`.
///
/// When `value` is `Some`, it is emitted as the decimal resource value. When `None`, only the resource number is emitted, requesting a reset to default.
pub fn write_set_key_modifier_options<W: Write>(
    w: &mut W,
    resource: u16,
    value: Option<u16>,
) -> io::Result<()> {
    match value {
        Some(v) => write!(w, "\x1b[>{resource};{v}m"),
        None => write!(w, "\x1b[>{resource}m"),
    }
}

/// Query a key-modifier resource with `ESC [ ? <resource> m`.
///
/// The terminal response, when supported, is parsed elsewhere; this function only emits the query bytes.
pub fn write_query_key_modifier_options<W: Write>(w: &mut W, resource: u16) -> io::Result<()> {
    write!(w, "\x1b[?{resource}m")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set() {
        let mut buf = Vec::new();
        write_set_key_modifier_options(&mut buf, 4, Some(2)).unwrap();
        assert_eq!(buf, b"\x1b[>4;2m");
    }

    #[test]
    fn test_reset() {
        let mut buf = Vec::new();
        write_set_key_modifier_options(&mut buf, 4, None).unwrap();
        assert_eq!(buf, b"\x1b[>4m");
    }
}
