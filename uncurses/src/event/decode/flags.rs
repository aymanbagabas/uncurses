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
//! These flags are decoder construction-time policy. Choose them to match the
//! bindings your application wants to expose; they are not terminal mode
//! negotiation flags.
use bitflags::bitflags;

bitflags! {
    /// Optional disambiguation knobs for the input decoder.
    ///
    /// With no flag set, the decoder reports the following mappings:
    ///
    /// * `0x00` → `Ctrl+Space`
    /// * `0x09` → `Tab`
    /// * `0x0d` → `Enter`
    /// * `0x7f` → `Backspace`
    /// * `CSI 1 ~` → `Home`
    /// * `CSI 4 ~` → `End`
    ///
    /// Set the corresponding flag to swap each mapping to its alternative
    /// reading.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct DecoderFlags: u8 {
        /// Report `0x00` as `Ctrl+@` instead of `Ctrl+Space`.
        const CTRL_AT             = 1 << 0;
        /// Report `0x09` as `Ctrl+i` instead of `Tab`.
        const CTRL_I              = 1 << 1;
        /// Report `0x0d` as `Ctrl+m` instead of `Enter`.
        const CTRL_M              = 1 << 2;
        /// Report a source-expired lone `0x1b` as `Ctrl+[` instead of `Escape`.
        ///
        /// This applies to the [`EventSource`](crate::event::EventSource)
        /// timeout path that calls the decoder's leading-byte expiry helper.
        /// The legacy buffered [`Decoder::drain`](super::Decoder::drain)
        /// method still emits Escape for a lone `ESC`.
        const CTRL_OPEN_BRACKET   = 1 << 3;
        /// Report `0x7f` as `Delete` instead of `Backspace`.
        const BACKSPACE_IS_DELETE = 1 << 4;
        /// Report `CSI 1 ~` as the VT220 `Find` key instead of `Home`.
        const FIND_KEY            = 1 << 5;
        /// Report `CSI 4 ~` as the VT220 `Select` key instead of `End`.
        const SELECT_KEY          = 1 << 6;
    }
}
