//! Progressive keyboard enhancement protocol.
//!
//! ## Category
//!
//! This module emits CSI `u` queries and setters for enhanced keyboard reporting:
//! requesting active flags, setting/add/removing flags, and pushing/popping a
//! keyboard flag stack.
//!
//! ## CSI format
//!
//! The protocol uses private CSI prefixes before the final `u`:
//!
//! ```text
//! ESC [ = flags ; mode u     set/add/remove flags
//! ESC [ > flags u            push stack frame
//! ESC [ < count u            pop stack frame(s)
//! ESC [ ? u                  query active flags
//! ```
//!
//! ## Mode interaction
//!
//! These controls manage their own keyboard-reporting state and are independent
//! of modifyOtherKeys controls in [`crate::ansi::xterm`].

use std::io::{self, Write};

bitflags::bitflags! {
    /// Bitflags for progressive keyboard reporting.
    ///
    /// The numeric value of the flag set is written as the `flags` parameter in
    /// CSI `u` requests such as `ESC [ = <flags> ; <mode> u`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct KittyKeyboardFlags: u8 {
        /// Flag bit `1`: request disambiguated escape-coded keys.
        const DISAMBIGUATE_ESCAPE_CODES   = 0b0000_0001;
        /// Flag bit `2`: request press, repeat, and release event-type reporting.
        const REPORT_EVENT_TYPES          = 0b0000_0010;
        /// Flag bit `4`: request shifted and base alternate key values.
        const REPORT_ALTERNATE_KEYS       = 0b0000_0100;
        /// Flag bit `8`: request escape-sequence reports for all keys.
        const REPORT_ALL_KEYS_AS_ESCAPE   = 0b0000_1000;
        /// Flag bit `16`: request associated text for key events.
        const REPORT_ASSOCIATED_TEXT      = 0b0001_0000;
        /// All defined keyboard enhancement flags combined.
        const ALL = Self::DISAMBIGUATE_ESCAPE_CODES.bits()
                  | Self::REPORT_EVENT_TYPES.bits()
                  | Self::REPORT_ALTERNATE_KEYS.bits()
                  | Self::REPORT_ALL_KEYS_AS_ESCAPE.bits()
                  | Self::REPORT_ASSOCIATED_TEXT.bits();
    }
}

/// Request active keyboard enhancement flags: exact bytes `ESC [ ? u` (`b"\x1b[?u"`).
///
/// A compatible terminal replies with CSI `? <flags> u`.
pub const REQUEST_KITTY_KEYBOARD: &[u8] = b"\x1b[?u";

/// Write [`REQUEST_KITTY_KEYBOARD`], the `ESC [ ? u` active-flag query.
pub fn write_request_kitty_keyboard<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_KITTY_KEYBOARD)
}

/// Operation mode used by [`write_set_kitty_keyboard`] for CSI `= flags ; mode u`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum KittyKeyboardMode {
    /// Replace the active keyboard flags with exactly the supplied flag set; parameter value `1`.
    Set = 1,
    /// Add the supplied keyboard flags to the active set; parameter value `2`.
    Add = 2,
    /// Remove the supplied keyboard flags from the active set; parameter value `3`.
    Remove = 3,
}

/// Set, add, or remove keyboard enhancement flags with `ESC [ = <flags> ; <mode> u`.
///
/// `flags.bits()` supplies the decimal flag mask; [`KittyKeyboardMode`] supplies the operation parameter.
pub fn write_set_kitty_keyboard<W: Write>(
    w: &mut W,
    flags: KittyKeyboardFlags,
    mode: KittyKeyboardMode,
) -> io::Result<()> {
    write!(w, "\x1b[={};{}u", flags.bits(), mode as u8)
}

/// Push a keyboard enhancement stack frame with `ESC [ > <flags> u`.
///
/// The decimal flag mask is taken from `flags.bits()`.
pub fn write_push_kitty_keyboard<W: Write>(w: &mut W, flags: KittyKeyboardFlags) -> io::Result<()> {
    write!(w, "\x1b[>{}u", flags.bits())
}

/// Pop keyboard enhancement stack frames with `ESC [ < <count> u`.
///
/// `count <= 1` emits the short form `ESC [ < u`; larger counts include the decimal count.
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
        let flags =
            KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES | KittyKeyboardFlags::REPORT_EVENT_TYPES;
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
        let flags = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES;
        write_set_kitty_keyboard(&mut buf, flags, KittyKeyboardMode::Add).unwrap();
        assert_eq!(buf, b"\x1b[=1;2u");
    }
}
