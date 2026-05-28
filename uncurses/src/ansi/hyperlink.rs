//! OSC 8 hyperlink sequences.

use std::io::{self, Write};

/// Begin a hyperlink. `params` is a (possibly empty) `key=value:…` string
/// (e.g. `"id=abc"`); `url` is the target URI.
pub fn write_hyperlink_start<W: Write>(w: &mut W, url: &str, params: &str) -> io::Result<()> {
    write!(w, "\x1b]8;{params};{url}\x1b\\")
}

/// End the current hyperlink (`OSC 8 ; ; ST`).
pub fn write_hyperlink_end<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b]8;;\x1b\\")
}

/// Parse an OSC 8 body into `(params, url)`.
///
/// `body` is the bytes inside the OSC, *without* the introducer (`\x1b]` or
/// `\x9d`) and *without* the string terminator (`\x07`, `\x1b\\`, or
/// `\x9c`). A valid OSC 8 body starts with `8;`, contains a single
/// separator between `params` and `url`, and is otherwise:
///
/// ```text
/// 8 ; params ; url
/// ```
///
/// Returns `None` for malformed input (missing `8;` prefix, fewer than two
/// `;`, or non-UTF-8 bytes in params/url). An empty `url` signals
/// end-of-link.
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
        write_hyperlink_start(&mut buf, "https://example.com", "").unwrap();
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
