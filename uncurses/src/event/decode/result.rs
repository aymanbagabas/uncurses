//! Internal result type for a single decode attempt.
//!
//! ## Purpose
//!
//! [`ParseResult`] lets small parser functions distinguish a complete event, a
//! valid-but-incomplete prefix, and an unrecognized prefix that should be
//! consumed. The outer [`Decoder`](super::Decoder) uses that distinction to
//! buffer partial sequences without dropping bytes.
//!
//! ## Gotchas
//!
//! `Incomplete` never means "invalid"; it means the caller should keep the
//! current prefix and wait for more bytes or an explicit timeout decision.
use crate::event::Event;

/// What `try_parse` learned after looking at the current prefix of the
/// input buffer.
pub(super) enum ParseResult {
    /// A complete sequence was recognized; `usize` is the number of
    /// input bytes consumed.
    Event(Event, usize),
    /// The prefix is the start of a valid sequence but the rest of the
    /// sequence hasn't arrived yet. The driver should hand more bytes
    /// to the decoder before retrying.
    Incomplete,
    /// The prefix didn't match any known sequence; `usize` is the
    /// number of bytes that should be discarded (typically the single
    /// stray byte at the head of the buffer).
    None(usize),
}
