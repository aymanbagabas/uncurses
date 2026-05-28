//! Strip ANSI escape sequences from a string.

use super::text::{Token, WidthMode, tokenize};

/// Return a copy of `s` with all ANSI escape sequences removed.
///
/// C0 control bytes other than ESC (e.g. `\n`, `\r`, `\t`) are preserved. Plain
/// text including UTF-8 and grapheme clusters passes through unchanged.
pub fn strip(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for tok in tokenize(s.as_bytes(), WidthMode::default(), false) {
        match tok {
            Token::Text { text, .. } => out.push_str(bs(text)),
            Token::Control(b) => out.push(b as char),
            Token::Escape(_) => {}
        }
    }
    out
}

#[inline]
fn bs(b: &[u8]) -> &str {
    // SAFETY: Tokens emitted for `&str` input are always whole grapheme
    // clusters or escape sequences that fall on valid UTF-8 boundaries.
    unsafe { std::str::from_utf8_unchecked(b) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_sgr() {
        assert_eq!(strip("\x1b[31mhello\x1b[0m"), "hello");
    }

    #[test]
    fn strip_osc() {
        assert_eq!(strip("\x1b]0;title\x07hello"), "hello");
        assert_eq!(strip("\x1b]0;title\x1b\\hello"), "hello");
    }

    #[test]
    fn strip_preserves_newlines() {
        assert_eq!(strip("a\nb\tc"), "a\nb\tc");
    }

    #[test]
    fn strip_unicode() {
        assert_eq!(strip("\x1b[1m中文\x1b[m"), "中文");
    }

    #[test]
    fn strip_empty() {
        assert_eq!(strip(""), "");
    }

    #[test]
    fn strip_only_escapes() {
        assert_eq!(strip("\x1b[31m\x1b[m"), "");
    }

    #[test]
    fn strip_nested_csi() {
        assert_eq!(strip("a\x1b[1;2;3;4mb\x1b[0;1mc"), "abc");
    }

    #[test]
    fn strip_two_byte_esc() {
        // ESC = (DECKPAM)
        assert_eq!(strip("a\x1b=b"), "ab");
    }
}
