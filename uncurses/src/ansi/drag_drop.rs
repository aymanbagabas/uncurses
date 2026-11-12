//! Kitty drag and drop messages through OSC 72.
//!
//! [`write_drag_drop`] frames one message. Callers supply metadata and a
//! wire-ready payload, manage registration, and complete or cancel transfers.
//! Unregister before a shell handoff or exit.
//!
//! Binary payloads use standard base64. Each encoded payload is at most 4096
//! bytes. For a chunked message, use `m=1` until the last chunk (`m=0`).
//! Only the first chunk must contain the full metadata. An empty data response
//! with `m=0` ends a transfer; a nonempty response with `m=0` only ends a message.
//!
//! Replies arrive as [`Event::DragDrop`](crate::event::Event::DragDrop).
//! See the [protocol specification](https://sw.kovidgoyal.net/kitty/dnd-protocol/).

use std::io::{self, Write};

/// Query OSC 72 support without a routing ID.
///
/// Pass this to [`Program::query_capabilities`](crate::program::Program::query_capabilities)
/// to follow it with the usual primary device attributes query.
pub const REQUEST_DRAG_DROP: &[u8] = b"\x1b]72;t=q\x1b\\";

/// Write one OSC 72 message without flushing.
///
/// Options are joined with `:`. The payload separator is omitted when the
/// payload is empty. Options and payload are written verbatim: callers must
/// supply escape-safe metadata and encode binary data as base64.
///
/// A payload larger than 4096 bytes returns [`io::ErrorKind::InvalidInput`]
/// before output. This function performs framing, not transfer assembly or
/// base64 conversion. It propagates output errors.
///
/// ```
/// use uncurses::ansi::drag_drop::write_drag_drop;
///
/// let mut bytes = Vec::new();
/// write_drag_drop(&mut bytes, &["t=m", "o=1"], b"text/plain")?;
/// assert_eq!(bytes, b"\x1b]72;t=m:o=1;text/plain\x1b\\");
/// # Ok::<(), std::io::Error>(())
/// ```
pub fn write_drag_drop<W: Write>(w: &mut W, options: &[&str], payload: &[u8]) -> io::Result<()> {
    if payload.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "OSC 72 payload exceeds 4096 bytes",
        ));
    }
    w.write_all(b"\x1b]72;")?;
    for (index, option) in options.iter().enumerate() {
        if index != 0 {
            w.write_all(b":")?;
        }
        w.write_all(option.as_bytes())?;
    }
    if !payload.is_empty() {
        w.write_all(b";")?;
        w.write_all(payload)?;
    }
    w.write_all(b"\x1b\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_and_raw_chunks() {
        let mut bytes = Vec::new();
        write_drag_drop(&mut bytes, &["t=q"], b"").unwrap();
        assert_eq!(bytes, REQUEST_DRAG_DROP);
        bytes.clear();
        write_drag_drop(&mut bytes, &["t=r", "x=1", "m=1"], b"YW").unwrap();
        write_drag_drop(&mut bytes, &["m=0"], b"Jj").unwrap();
        write_drag_drop(&mut bytes, &["t=r", "x=1"], b"").unwrap();
        assert_eq!(
            bytes,
            b"\x1b]72;t=r:x=1:m=1;YW\x1b\\\x1b]72;m=0;Jj\x1b\\\x1b]72;t=r:x=1\x1b\\"
        );
    }

    #[test]
    fn payload_limit_and_output_errors() {
        let mut bytes = Vec::new();
        write_drag_drop(&mut bytes, &["t=p", "x=0"], &[b'A'; 4096]).unwrap();
        let before = bytes.clone();
        assert_eq!(
            write_drag_drop(&mut bytes, &["t=p"], &[b'A'; 4097])
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(bytes, before);
        assert_eq!(
            write_drag_drop(&mut [].as_mut_slice(), &["t=q"], b"")
                .unwrap_err()
                .kind(),
            io::ErrorKind::WriteZero
        );
    }
}
