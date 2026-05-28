//! xterm-specific key modifier options (XTMODKEYS / modifyOtherKeys).

use std::io::{self, Write};

/// Enable modifyOtherKeys mode 1 (`CSI > 4 ; 1 m`).
pub const SET_MODIFY_OTHER_KEYS_1: &[u8] = b"\x1b[>4;1m";

/// Enable modifyOtherKeys mode 2 (`CSI > 4 ; 2 m`).
pub const SET_MODIFY_OTHER_KEYS_2: &[u8] = b"\x1b[>4;2m";

/// Reset modifyOtherKeys to its default (`CSI > 4 m`).
pub const RESET_MODIFY_OTHER_KEYS: &[u8] = b"\x1b[>4m";

/// Query modifyOtherKeys state (`CSI ? 4 m`).
pub const QUERY_MODIFY_OTHER_KEYS: &[u8] = b"\x1b[?4m";

/// Set an xterm key-modifier option (XTMODKEYS, `CSI > Pp ; Pv m`).
///
/// If `value` is `None`, the parameter is reset (`CSI > Pp m`).
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

/// Query an xterm key-modifier option (XTQMODKEYS, `CSI ? Pp m`).
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
