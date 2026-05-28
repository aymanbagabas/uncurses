//! Bracketed-paste body handling.
//!
//! Once the decoder has seen `Event::PasteStart` (CSI 200~), all incoming
//! bytes are streamed as [`Event::PasteChunk`] until the terminator
//! `Event::PasteEnd` (CSI 201~) is recognized — the only escape
//! sequence honored inside a paste body.

use super::Decoder;
use super::result::ParseResult;
use crate::event::Event;

impl Decoder {
    /// Stream-friendly paste handler. Emits a `PasteChunk` over the
    /// safe-to-flush prefix of `data` (or a `PasteEnd` once the
    /// terminator is parsed). Bytes that look like the start of an
    /// escape sequence are held back until they can be classified.
    pub(super) fn parse_paste_chunk(&mut self, data: &[u8]) -> (usize, Option<Event>) {
        // Scan for the next ESC (0x1B) introducer. Bytes before it are
        // literal paste content; at the introducer we let `try_parse`
        // decide whether it's the PasteEnd terminator or some other
        // sequence that should be treated as part of the paste body.
        //
        // We deliberately do NOT scan for the 8-bit CSI introducer
        // (0x9B) inside paste content: 0x9B is a valid UTF-8
        // continuation byte (range 0x80..=0xBF), so a paste body
        // containing e.g. `ћ` (U+045B, encoded as `D1 9B`) followed by
        // `201~` would otherwise be misparsed as a paste terminator and
        // the rest of the paste would escape paste mode, surfacing as
        // keypresses. The 8-bit form for paste termination is not used
        // by terminal emulators in practice; every emulator emits the
        // 7-bit `\x1b[201~` form.
        let mut scan = 0;
        while scan < data.len() {
            if data[scan] != 0x1B {
                scan += 1;
                continue;
            }
            match self.try_parse(&data[scan..]) {
                ParseResult::Event(Event::PasteEnd, consumed) => {
                    self.in_paste = false;
                    if scan == 0 {
                        return (consumed, Some(Event::PasteEnd));
                    }
                    self.pending.borrow_mut().push_back(Event::PasteEnd);
                    return (
                        scan + consumed,
                        Some(Event::PasteChunk(data[..scan].to_vec())),
                    );
                }
                ParseResult::Incomplete => {
                    if scan == 0 {
                        return (0, None);
                    }
                    return (scan, Some(Event::PasteChunk(data[..scan].to_vec())));
                }
                ParseResult::Event(_, consumed) => {
                    // Some other complete sequence inside the paste body
                    // — keep it as literal bytes in the chunk.
                    scan += consumed.max(1);
                }
                ParseResult::None(consumed) => {
                    scan += consumed.max(1);
                }
            }
        }

        // No escape sequence anywhere — flush everything as raw bytes.
        if data.is_empty() {
            return (0, None);
        }
        (data.len(), Some(Event::PasteChunk(data.to_vec())))
    }
}
