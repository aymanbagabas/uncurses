//! Desktop notification sequences (iTerm OSC 9 and Kitty OSC 99).

use std::io::{self, Write};

/// Send a simple iTerm-compatible desktop notification (`OSC 9 ; body ST`).
pub fn write_notify<W: Write>(w: &mut W, body: &str) -> io::Result<()> {
    write!(w, "\x1b]9;{body}\x07")
}

/// Send a Kitty-compatible desktop notification (`OSC 99 ; meta ; body ST`).
///
/// `metadata` is a list of key-value strings joined with `:`. See:
/// https://sw.kovidgoyal.net/kitty/desktop-notifications/
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
