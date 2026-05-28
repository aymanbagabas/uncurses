//! Kitty keyboard protocol — progressive enhancement.
//!
//! See: https://sw.kovidgoyal.net/kitty/keyboard-protocol/

use std::io::{self, Write};

bitflags::bitflags! {
    /// Kitty keyboard enhancement flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct KittyFlags: u8 {
        const DISAMBIGUATE_ESCAPE_CODES   = 0b0000_0001;
        const REPORT_EVENT_TYPES          = 0b0000_0010;
        const REPORT_ALTERNATE_KEYS       = 0b0000_0100;
        const REPORT_ALL_KEYS_AS_ESCAPE   = 0b0000_1000;
        const REPORT_ASSOCIATED_TEXT      = 0b0001_0000;
        const ALL = Self::DISAMBIGUATE_ESCAPE_CODES.bits()
                  | Self::REPORT_EVENT_TYPES.bits()
                  | Self::REPORT_ALTERNATE_KEYS.bits()
                  | Self::REPORT_ALL_KEYS_AS_ESCAPE.bits()
                  | Self::REPORT_ASSOCIATED_TEXT.bits();
    }
}

/// Request the terminal's currently-active Kitty keyboard flags
/// (`CSI ? u`). The terminal responds with `CSI ? <flags> u`.
pub const REQUEST_KITTY_KEYBOARD: &[u8] = b"\x1b[?u";

/// Disable the Kitty keyboard protocol (`CSI > u`).
///
/// Equivalent to [`write_push_kitty_keyboard`] with an empty flag set.
pub const DISABLE_KITTY_KEYBOARD: &[u8] = b"\x1b[>u";

pub fn write_request_kitty_keyboard<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_KITTY_KEYBOARD)
}

pub fn write_disable_kitty_keyboard<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(DISABLE_KITTY_KEYBOARD)
}

/// Modes for [`write_set_kitty_keyboard`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum KittyKeyboardMode {
    /// Set given flags, unset all others.
    Set = 1,
    /// Set given flags, keep existing flags.
    Add = 2,
    /// Unset given flags, keep existing flags.
    Remove = 3,
}

/// Set Kitty keyboard flags (`CSI = flags ; mode u`).
pub fn write_set_kitty_keyboard<W: Write>(
    w: &mut W,
    flags: KittyFlags,
    mode: KittyKeyboardMode,
) -> io::Result<()> {
    write!(w, "\x1b[={};{}u", flags.bits(), mode as u8)
}

/// Push Kitty keyboard enhancement flags onto the stack (`CSI > flags u`).
pub fn write_push_kitty_keyboard<W: Write>(w: &mut W, flags: KittyFlags) -> io::Result<()> {
    if flags.is_empty() {
        w.write_all(DISABLE_KITTY_KEYBOARD)
    } else {
        write!(w, "\x1b[>{}u", flags.bits())
    }
}

/// Pop `count` levels off the Kitty keyboard stack (`CSI < count u`).
pub fn write_pop_kitty_keyboard<W: Write>(w: &mut W, count: u16) -> io::Result<()> {
    if count <= 1 {
        w.write_all(b"\x1b[<u")
    } else {
        write!(w, "\x1b[<{count}u")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push() {
        let mut buf = Vec::new();
        let flags = KittyFlags::DISAMBIGUATE_ESCAPE_CODES | KittyFlags::REPORT_EVENT_TYPES;
        write_push_kitty_keyboard(&mut buf, flags).unwrap();
        assert_eq!(buf, b"\x1b[>3u");
    }

    #[test]
    fn test_pop_default() {
        let mut buf = Vec::new();
        write_pop_kitty_keyboard(&mut buf, 1).unwrap();
        assert_eq!(buf, b"\x1b[<u");
    }

    #[test]
    fn test_set() {
        let mut buf = Vec::new();
        let flags = KittyFlags::DISAMBIGUATE_ESCAPE_CODES;
        write_set_kitty_keyboard(&mut buf, flags, KittyKeyboardMode::Add).unwrap();
        assert_eq!(buf, b"\x1b[=1;2u");
    }
}
