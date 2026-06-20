//! Keypad application and numeric mode selectors.
//!
//! ## Category
//!
//! These short ESC sequences switch the numeric keypad between application mode
//! and numeric mode.
//!
//! ## Escape format
//!
//! The controls are not CSI sequences: application mode is `ESC =`, and numeric
//! mode is `ESC >`.
//!
//! ## Mode interaction
//!
//! The same state is commonly described as DEC numeric keypad mode
//! ([`Mode::NUMERIC_KEYPAD`](crate::ansi::mode::Mode::NUMERIC_KEYPAD), private
//! mode 66). These constants provide the traditional DECKPAM/DECKPNM byte forms.

/// Keypad Application Mode (DECKPAM): exact bytes `ESC =` (`b"\x1b="`).
///
/// After this, keypad keys normally send application sequences instead of digits/operators.
pub const KEYPAD_APPLICATION_MODE: &[u8] = b"\x1b=";

/// Keypad Numeric Mode (DECKPNM): exact bytes `ESC >` (`b"\x1b>"`).
///
/// After this, keypad keys normally send numeric characters and operators.
pub const KEYPAD_NUMERIC_MODE: &[u8] = b"\x1b>";
