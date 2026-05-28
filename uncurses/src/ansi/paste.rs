//! Bracketed paste mode delimiter constants.
//!
//! When DECSET 2004 (bracketed paste) is enabled, the terminal wraps pasted
//! text between these two sequences. Enabling/disabling the mode itself is
//! done with [`crate::ansi::mode::Mode::BRACKETED_PASTE`].

/// Start of a bracketed-paste payload (`CSI 200 ~`).
pub const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";

/// End of a bracketed-paste payload (`CSI 201 ~`).
pub const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";
