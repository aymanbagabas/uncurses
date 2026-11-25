//! ANSI escape stripping built on the byte tokenizer.
//!
//! ## Category
//!
//! [`strip`] removes ANSI escape/string/control sequences while preserving
//! printable text and non-ESC control bytes such as newlines and tabs.
//!
//! ## Parser conventions
//!
//! Tokenization recognizes CSI, OSC, DCS, SOS, PM, APC, and two-byte ESC
//! sequences in both 7-bit and 8-bit forms. Escape tokens contribute no output.
//!
//! ## Mode interaction
//!
//! Stripping does not emulate terminal modes. It is a byte-stream transformation
//! suitable for display-width and plain-text extraction paths.
//!
//! Sequence boundaries and widths come from [`crate::ansi::text`];
//! which byte ends a control string, and when a byte in `0x80..=0x9F`
//! is a C1 control rather than part of a character, are documented there.

use super::text::{Token, WidthMode, tokenize};

/// Return `s` with ANSI escape sequences removed.
///
/// CSI, OSC, DCS, SOS, PM, APC, and short ESC sequences are dropped. Printable
/// UTF-8 text and non-ESC control bytes such as `\n`, `\r`, and `\t` are
/// preserved.
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
    // The tokenizer never splits a character: text tokens are whole grapheme
    // clusters, and every sequence scanner steps a whole UTF-8 character at a
    // time. Nothing enforced that, and when a scanner did split one - 0x9C is
    // 8-bit ST and also a continuation byte, so an OSC title containing a
    // check mark ended mid-character - the ill-formed bytes arrived here and
    // this was undefined behaviour. Checked where checking is free.
    debug_assert!(
        std::str::from_utf8(b).is_ok(),
        "token split a UTF-8 character: {b:?}"
    );
    // SAFETY: `b` is a token slice of `&str` input, taken on character
    // boundaries, as asserted above.
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
