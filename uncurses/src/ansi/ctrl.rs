//! Terminal-wide control sequences: RIS reset and Device Attributes (DA1/2/3).

use std::io::{self, Write};

/// Reset to Initial State (RIS, `ESC c`). Hard-resets the terminal.
pub const RIS: &[u8] = b"\x1bc";

/// Reset the terminal to its initial state (RIS).
pub fn write_ris<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(RIS)
}

/// Request Primary Device Attributes (DA1, `CSI c`).
pub const REQUEST_PRIMARY_DA: &[u8] = b"\x1b[c";

/// Request Secondary Device Attributes (DA2, `CSI > c`).
pub const REQUEST_SECONDARY_DA: &[u8] = b"\x1b[>c";

/// Request Tertiary Device Attributes (DA3, `CSI = c`).
pub const REQUEST_TERTIARY_DA: &[u8] = b"\x1b[=c";

/// Request the terminal's name and version (XTVERSION, `CSI > q`).
pub const REQUEST_XTVERSION: &[u8] = b"\x1b[>q";

/// Write the Primary Device Attributes request (`CSI c`).
pub fn write_request_primary_da<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_PRIMARY_DA)
}

/// Write the Secondary Device Attributes request (`CSI > c`).
pub fn write_request_secondary_da<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_SECONDARY_DA)
}

/// Write the Tertiary Device Attributes request (`CSI = c`).
pub fn write_request_tertiary_da<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_TERTIARY_DA)
}

/// Write the terminal name and version request (`CSI > q`).
pub fn write_request_xtversion<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_XTVERSION)
}
