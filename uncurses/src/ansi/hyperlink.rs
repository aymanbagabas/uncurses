//! OSC 8 hyperlink writer and parser.
//!
//! ## Category
//!
//! OSC 8 annotates following text with a URI until a closing OSC 8 with an empty
//! URI is emitted. This module writes start/end markers and parses the body of an
//! already-framed OSC 8 string.
//!
//! ## OSC framing
//!
//! Writers use the 7-bit OSC introducer and the `ST` terminator (`ESC \\`):
//!
//! ```text
//! ESC ] 8 ; params ; uri ESC \\  text  ESC ] 8 ; ; ESC \\
//! ──┬── ┬   ───┬──   ┬   ──┬──        ───── close ─────
//!  OSC code  attrs target  ST
//! ```
//!
//! ## Mode interaction
//!
//! Hyperlinks are not controlled by an ANSI/DEC mode. They are zero-width string
//! controls and are preserved by width-aware text utilities.

use std::io::{self, Write};

/// Bytes that reset the current OSC 8 hyperlink: `ESC ] 8 ; ; ESC \`.
///
/// Writing this empty-URI OSC 8 sequence closes any hyperlink opened by
/// [`write_hyperlink`]; writing it with no hyperlink open is a harmless no-op.
pub const HYPERLINK_RESET: &[u8] = b"\x1b]8;;\x1b\\";

/// Begin an OSC 8 hyperlink with `ESC ] 8 ; <params> ; <url> ESC \`.
///
/// `params` and `url` are emitted verbatim. The hyperlink applies to following
/// printable text until [`HYPERLINK_RESET`] is written.
pub fn write_hyperlink<W: Write>(w: &mut W, url: &str, params: &str) -> io::Result<()> {
    write!(w, "\x1b]8;{params};{url}\x1b\\")
}

/// Parse an OSC 8 body into `(params, url)`.
///
/// `body` must exclude the OSC introducer and terminator, start with `8;`, and contain the separator between params and URL. The URL may contain additional semicolons; only the first semicolon after `8;` separates params from URL. Returns `None` for malformed or non-UTF-8 bodies.
pub fn parse_hyperlink(body: &[u8]) -> Option<(&str, &str)> {
    let rest = body.strip_prefix(b"8;")?;
    let sep = rest.iter().position(|&b| b == b';')?;
    let params = std::str::from_utf8(&rest[..sep]).ok()?;
    let url = std::str::from_utf8(&rest[sep + 1..]).ok()?;
    Some((params, url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hyperlink() {
        let mut buf = Vec::new();
        write_hyperlink(&mut buf, "https://example.com", "").unwrap();
        assert_eq!(buf, b"\x1b]8;;https://example.com\x1b\\");
    }

    #[test]
    fn parse_basic() {
        let (params, url) = parse_hyperlink(b"8;;https://example.com").unwrap();
        assert_eq!(params, "");
        assert_eq!(url, "https://example.com");
    }

    #[test]
    fn parse_with_params() {
        let (params, url) = parse_hyperlink(b"8;id=abc;https://x").unwrap();
        assert_eq!(params, "id=abc");
        assert_eq!(url, "https://x");
    }

    #[test]
    fn parse_close() {
        // `8;;` is the close-link form (empty url).
        let (params, url) = parse_hyperlink(b"8;;").unwrap();
        assert_eq!(params, "");
        assert_eq!(url, "");
    }

    #[test]
    fn parse_url_with_semicolons() {
        // Only the first `;` after `8;` separates params from url; the url
        // keeps the rest of the body verbatim.
        let (params, url) = parse_hyperlink(b"8;id=1;https://x?a=1;b=2").unwrap();
        assert_eq!(params, "id=1");
        assert_eq!(url, "https://x?a=1;b=2");
    }

    #[test]
    fn parse_rejects_wrong_prefix() {
        assert!(parse_hyperlink(b"0;title").is_none());
        assert!(parse_hyperlink(b"").is_none());
    }

    #[test]
    fn parse_rejects_missing_separator() {
        // `8;url` has only one `;` after the prefix.
        assert!(parse_hyperlink(b"8;noseparator").is_none());
    }

    #[test]
    fn parse_rejects_invalid_utf8() {
        assert!(parse_hyperlink(b"8;\xff;url").is_none());
        assert!(parse_hyperlink(b"8;;\xff").is_none());
    }
}
