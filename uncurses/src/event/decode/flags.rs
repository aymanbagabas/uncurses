//! Decoder behavior flags for ambiguous legacy input bytes.
//!
//! ## Purpose
//!
//! [`DecoderFlags`] toggles byte sequences that have more than one historical
//! interpretation when the terminal is not using an unambiguous keyboard
//! protocol. With no flags set, the decoder favors semantic keys such as Tab,
//! Enter, Escape, Backspace, Home, and End.
//!
//! ## Affected paths
//!
//! The flags affect C0 controls, a timed-out lone `ESC`, Delete/Backspace, and
//! VT220 Find/Select tilde codes. They do not change richer keyboard encodings
//! that carry explicit key identity.
//!
//! ## Gotchas
//!
//! These flags select legacy key interpretations. Choose them to match the
//! bindings your application exposes. Backarrow mode supplies the baseline;
//! the flags select explicit overrides.
use bitflags::bitflags;

bitflags! {
    /// Optional disambiguation knobs for the input decoder.
    ///
    /// With no flag set and Backarrow mode reset, the decoder reports:
    ///
    /// * `0x00` → `Ctrl+Space`
    /// * `0x08` → `Ctrl+h`
    /// * `0x09` → `Tab`
    /// * `0x0a` → `Ctrl+j`
    /// * `0x0d` → `Enter`
    /// * `0x7f` → `Backspace`
    /// * `CSI 1 ~` → `Home`
    /// * `CSI 4 ~` → `End`
    ///
    /// Set the corresponding flag to swap each mapping to its alternative
    /// reading.
    ///
    /// In raw mode, terminals normally send LF for Ctrl+J and CR for Enter.
    /// An `ESC` prefix adds Alt to the selected interpretation.
    ///
    /// Backarrow mode (DECBKM) defaults to reset. When set, `0x08` reads as
    /// Backspace and `0x7f` as Ctrl+Backspace. These are legacy byte
    /// interpretations: terminals can send the same bytes for different keys.
    ///
    /// [`CTRL_H`](Self::CTRL_H) overrides `0x08` in either mode and takes
    /// precedence over [`CTRL_BACKSPACE`](Self::CTRL_BACKSPACE).
    /// [`DEL_IS_DELETE`](Self::DEL_IS_DELETE) overrides `0x7f` in either mode.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct DecoderFlags: u16 {
        /// Report `0x00` as `Ctrl+@` instead of `Ctrl+Space`.
        const CTRL_AT             = 1 << 0;
        /// Report `0x09` as `Ctrl+i` instead of `Tab`.
        const CTRL_I              = 1 << 1;
        /// Report `0x0d` as `Ctrl+m` instead of `Enter`.
        const CTRL_M              = 1 << 2;
        /// Report a lone `0x1b` as `Ctrl+[` instead of `Escape`.
        ///
        /// Wherever that byte resolves as a key of its own: an
        /// [`EventSource`](crate::event::EventSource) reaching its escape
        /// deadline, whether the `ESC` was alone or at the head of a
        /// sequence that never finished, and the inner `ESC` of a run of
        /// them, which reads as `Alt+Ctrl+[`.
        const CTRL_OPEN_BRACKET   = 1 << 3;
        /// Report `0x7f` as `Delete` instead of `Backspace`.
        ///
        /// Takes precedence over Backarrow mode, including when `0x7f`
        /// would otherwise read as Ctrl+Backspace.
        const DEL_IS_DELETE       = 1 << 4;
        /// Report `CSI 1 ~` as the VT220 `Find` key instead of `Home`.
        const FIND_KEY            = 1 << 5;
        /// Report `CSI 4 ~` as the VT220 `Select` key instead of `End`.
        const SELECT_KEY          = 1 << 6;
        /// Report `0x0a` (LF) as `Enter` instead of `Ctrl+j`.
        ///
        /// An `ESC` prefix reports `Alt+Enter`. This is useful for input
        /// with CR-to-LF conversion enabled. CR keeps its own interpretation,
        /// selected by [`CTRL_M`](Self::CTRL_M).
        const LF_IS_ENTER         = 1 << 7;
        /// Report `0x08` as `Ctrl+h` in either Backarrow mode.
        ///
        /// Takes precedence over [`CTRL_BACKSPACE`](Self::CTRL_BACKSPACE)
        /// and the mode's Backspace interpretation.
        const CTRL_H              = 1 << 8;
        /// Report `0x08` as Ctrl+Backspace when Backarrow mode is reset.
        ///
        /// When the mode is set, `0x08` reads as Backspace and `0x7f`
        /// already reads as Ctrl+Backspace. [`CTRL_H`](Self::CTRL_H) and
        /// [`DEL_IS_DELETE`](Self::DEL_IS_DELETE) retain their precedence.
        const CTRL_BACKSPACE      = 1 << 9;
    }
}
