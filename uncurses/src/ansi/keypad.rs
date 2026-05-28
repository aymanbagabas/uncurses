//! Keypad application/numeric mode (DECKPAM / DECKPNM).

/// Keypad Application Mode (DECKPAM, `ESC =`).
pub const KEYPAD_APPLICATION_MODE: &[u8] = b"\x1b=";

/// Keypad Numeric Mode (DECKPNM, `ESC >`).
pub const KEYPAD_NUMERIC_MODE: &[u8] = b"\x1b>";
