//! Bracketed-paste delimiter sequences.
//!
//! ## Category
//!
//! These constants are the byte markers terminals send around pasted text when
//! bracketed paste is enabled.
//!
//! ## CSI format
//!
//! The start marker is `ESC [ 200 ~`; the end marker is `ESC [ 201 ~`. Bytes
//! between them are pasted data, not terminal control input.
//!
//! ## Mode interaction
//!
//! Enable or disable the reports with
//! [`Mode::BRACKETED_PASTE`](crate::ansi::mode::Mode::BRACKETED_PASTE), DEC
//! private mode 2004.

/// Start delimiter for bracketed paste: exact bytes `ESC [ 200 ~` (`b"\x1b[200~"`).
///
/// Terminals send this before pasted payload when DEC private mode 2004 is enabled.
pub const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";

/// End delimiter for bracketed paste: exact bytes `ESC [ 201 ~` (`b"\x1b[201~"`).
///
/// Terminals send this after pasted payload when DEC private mode 2004 is enabled.
pub const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";
