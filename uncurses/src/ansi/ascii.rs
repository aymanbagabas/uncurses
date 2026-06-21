//! Printable ASCII boundary constants used by the ANSI scanner.
//!
//! ## Category
//!
//! This module names the non-control byte at the start of the printable ASCII
//! range and the `DEL` byte at the end of the 7-bit control area. These values
//! are useful when classifying bytes before parsing CSI, OSC, DCS, and other
//! escape-sequence families.
//!
//! ## Conventions
//!
//! Constants are raw byte values, not escape sequences. `SP` is the literal
//! space byte (`0x20`); `DEL` is the single-byte delete control (`0x7f`).
//!
//! ## Mode interaction
//!
//! These bytes are independent of terminal modes. Higher-level modules such as
//! [`crate::ansi::text`] decide whether neighboring bytes form 7-bit (`ESC [`)
//! or 8-bit (`0x9b`) control sequences.

/// Space byte `0x20`, the first printable ASCII code point and the separator often used inside CSI parameter syntax as an intermediate byte.
pub const SP: u8 = 0x20;
/// Delete control byte `0x7f`, the last 7-bit ASCII byte. It is treated as a standalone control byte by the tokenizer.
pub const DEL: u8 = 0x7F;
