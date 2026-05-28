//! Decoder behavior flags.
//!
//! These flags toggle ambiguous mappings in the legacy (non-kitty) input
//! parser. With no flags set the decoder reports each C0 byte as its
//! "semantic" key (Tab/Enter/Escape/Backspace) and reports the VT220
//! Find/Select numeric tilde codes as Home/End.
//!
//! When the kitty keyboard protocol is negotiated the terminal sends
//! unambiguous codes in-band, so these flags only affect the legacy parse
//! paths.

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
        /// Report a lone (timed-out) `0x1b` as `Ctrl+[` instead of `Escape`.
        const CTRL_OPEN_BRACKET   = 1 << 3;
        /// Report `0x7f` as `Delete` instead of `Backspace`.
        const BACKSPACE_IS_DELETE = 1 << 4;
        /// Report `CSI 1 ~` as the VT220 `Find` key instead of `Home`.
        const FIND_KEY            = 1 << 5;
        /// Report `CSI 4 ~` as the VT220 `Select` key instead of `End`.
        const SELECT_KEY          = 1 << 6;
    }
}
