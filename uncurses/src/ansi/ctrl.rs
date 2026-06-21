//! Terminal reset, device-attribute, and version requests.
//!
//! ## Category
//!
//! This module contains terminal-wide control sequences: RIS (`ESC c`), primary,
//! secondary, and tertiary Device Attributes, and the XTVERSION query.
//!
//! ## CSI conventions
//!
//! Device-attribute requests are 7-bit CSI sequences. Private prefixes (`>` and
//! `=`) distinguish secondary and tertiary forms from primary DA.
//!
//! ## Mode interaction
//!
//! These requests do not require a mode. RIS is destructive: it asks the terminal
//! to reset state such as modes, colors, tabs, and character sets.

use std::io::{self, Write};

/// Reset to Initial State: exact bytes `ESC c` (`b"\x1bc"`).
///
/// This is a terminal-wide hard reset, not just an SGR or screen clear.
pub const RIS: &[u8] = b"\x1bc";

/// Write [`RIS`], the `ESC c` Reset to Initial State control.
///
/// Use only when the terminal should discard broad state such as modes, tabs, character sets, and visual attributes.
pub fn write_ris<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(RIS)
}

/// Primary Device Attributes request: exact bytes `ESC [ c` (`b"\x1b[c"`).
///
/// The omitted parameter is the standard DA1 request.
pub const REQUEST_PRIMARY_DA: &[u8] = b"\x1b[c";

/// Secondary Device Attributes request: exact bytes `ESC [ > c` (`b"\x1b[>c"`).
pub const REQUEST_SECONDARY_DA: &[u8] = b"\x1b[>c";

/// Tertiary Device Attributes request: exact bytes `ESC [ = c` (`b"\x1b[=c"`).
pub const REQUEST_TERTIARY_DA: &[u8] = b"\x1b[=c";

/// Terminal name/version request: exact bytes `ESC [ > q` (`b"\x1b[>q"`).
pub const REQUEST_XTVERSION: &[u8] = b"\x1b[>q";

/// Write the primary Device Attributes request `ESC [ c`.
pub fn write_request_primary_da<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_PRIMARY_DA)
}

/// Write the secondary Device Attributes request `ESC [ > c`.
pub fn write_request_secondary_da<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_SECONDARY_DA)
}

/// Write the tertiary Device Attributes request `ESC [ = c`.
pub fn write_request_tertiary_da<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_TERTIARY_DA)
}

/// Write the terminal version request `ESC [ > q`.
pub fn write_request_xtversion<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_XTVERSION)
}
