//! Current working directory notification (OSC 7).

use std::io::{self, Write};

/// Notify the terminal of the current working directory (`OSC 7 ; URL ST`).
///
/// `url` should be a `file://[host]/[path]` URL; pass `localhost` for `host`
/// when the path lives on the local machine.
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
