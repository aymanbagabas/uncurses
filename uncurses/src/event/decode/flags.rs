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
    /// * `0x08` → `Ctrl+h`
    /// * `0x09` → `Tab`
    /// * `0x0d` → `Enter`
    /// * `0x7f` → `Backspace`
    /// * `CSI 1 ~` → `Home`
    /// * `CSI 4 ~` → `End`
    ///
    /// Set the corresponding flag to swap each mapping to its alternative
    /// reading.
    ///
    /// `0x08` is the one byte with three readings rather than two, so two
    /// flags choose between them:
    ///
    /// | flags | `0x08` reads as |
    /// | --- | --- |
    /// | neither | `Ctrl+h` |
    /// | [`BS_IS_BACKSPACE`](Self::BS_IS_BACKSPACE) | `Backspace` |
    /// | [`BS_IS_CTRL_BACKSPACE`](Self::BS_IS_CTRL_BACKSPACE) | `Ctrl+Backspace` |
    ///
    /// The second of those holds the first, so the two cannot disagree: it
    /// is the Backspace reading with a modifier on it.
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
        /// Report `0x08` as `Backspace` instead of `Ctrl+h`.
        ///
        /// A terminal whose erase character is `^H` sends this for the
        /// Backspace key, which is the VT100 reading and what `stty erase
        /// ^H` asks for. Such a terminal usually sends `0x7f` for Delete,
        /// which is [`BACKSPACE_IS_DELETE`](Self::BACKSPACE_IS_DELETE).
        const BS_IS_BACKSPACE     = 1 << 7;
        /// Report `0x08` as `Ctrl+Backspace` instead of `Ctrl+h`.
        ///
        /// A terminal that sends `0x7f` for Backspace has this byte spare,
        /// and several spend it on `Ctrl+Backspace`, which has no encoding
        /// of its own otherwise.
        ///
        /// This holds [`BS_IS_BACKSPACE`](Self::BS_IS_BACKSPACE) as well,
        /// because it is that reading with a modifier on it. Asking for both
        /// is therefore asking for this one, rather than for two readings of
        /// a byte that can only have one.
        const BS_IS_CTRL_BACKSPACE = (1 << 8) | Self::BS_IS_BACKSPACE.bits();
        /// Report `0x7f` as `Delete` instead of `Backspace`.
        const BACKSPACE_IS_DELETE = 1 << 4;
        /// Report `CSI 1 ~` as the VT220 `Find` key instead of `Home`.
        const FIND_KEY            = 1 << 5;
        /// Report `CSI 4 ~` as the VT220 `Select` key instead of `End`.
        const SELECT_KEY          = 1 << 6;
    }
}
