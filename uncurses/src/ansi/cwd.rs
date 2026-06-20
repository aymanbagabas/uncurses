//! Current working directory notifications through OSC 7.
//!
//! ## Category
//!
//! OSC 7 communicates a process working-directory URL to the terminal so shell
//! integration can associate panes, tabs, or windows with a path.
//!
//! ## OSC framing
//!
//! The writer emits `ESC ] 7 ; <url> BEL`. The URL payload is passed through
//! unchanged and should normally be a `file://host/path` URL.
//!
//! ## Mode interaction
//!
//! No terminal mode controls OSC 7 emission. Terminals that do not implement the
//! notification ignore it as an ordinary OSC string.

use std::io::{self, Write};

/// Notify the terminal of the current working directory with `ESC ] 7 ; <url> BEL`.
///
/// `url` is emitted verbatim and should normally be a `file://host/path` URL such as `file://localhost/home/user/project`.
pub fn write_notify_working_directory<W: Write>(w: &mut W, url: &str) -> io::Result<()> {
    write!(w, "\x1b]7;{url}\x07")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cwd() {
        let mut buf = Vec::new();
        write_notify_working_directory(&mut buf, "file://localhost/tmp").unwrap();
        assert_eq!(buf, b"\x1b]7;file://localhost/tmp\x07");
    }
}
