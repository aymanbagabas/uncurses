//! Desktop notification OSC encoders.
//!
//! ## Category
//!
//! This module writes simple OSC 9 notifications and metadata-bearing OSC 99
//! notifications. Both forms are zero-width terminal string controls.
//!
//! ## OSC framing
//!
//! Writers use the 7-bit OSC introducer and BEL terminator. OSC 99 joins
//! metadata fields with `:` before the final body field.
//!
//! ## Mode interaction
//!
//! Notification support is not advertised through a mode here. Terminals that do
//! not implement the OSC numbers ignore the strings.

use std::io::{self, Write};

/// Send a simple notification with `ESC ] 9 ; <body> BEL`.
///
/// `body` is emitted verbatim as the notification text.
pub fn write_notify<W: Write>(w: &mut W, body: &str) -> io::Result<()> {
    write!(w, "\x1b]9;{body}\x07")
}

/// Send a metadata-bearing notification with `ESC ] 99 ; <metadata> ; <body> BEL`.
///
/// `metadata` entries are joined with `:` and emitted before the body field; `body` is emitted verbatim.
pub fn write_desktop_notification<W: Write>(
    w: &mut W,
    body: &str,
    metadata: &[&str],
) -> io::Result<()> {
    w.write_all(b"\x1b]99;")?;
    for (i, m) in metadata.iter().enumerate() {
        if i > 0 {
            w.write_all(b":")?;
        }
        w.write_all(m.as_bytes())?;
    }
    write!(w, ";{body}\x07")
}
