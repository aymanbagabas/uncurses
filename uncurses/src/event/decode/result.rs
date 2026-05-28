//! Outcome of one parse step in [`Decoder::try_parse`].

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
