//! ANSI-aware byte-stream tokenizer for text utilities.
//!
//! Walks an input byte slice and classifies bytes into one of:
//!
//! * `Text` — a grapheme cluster of visible text + its display width
//! * `Escape` — an ANSI escape sequence (CSI/OSC/DCS/SOS/PM/APC/ESC-only or
//!   their 8-bit C1 equivalents), passed through verbatim
//! * `Control` — a single C0 or C1 control byte (e.g. `\n`, `\r`, `\t`)
//!
//! Both the 7-bit (`\x1b[…`, `\x1b]…\x07`) and the 8-bit (`\x9b…`,
//! `\x9d…\x9c`) sequence forms are recognised, matching the byte-level
//! semantics of the underlying parser.
//!
//! This is the foundation for `strip_ansi`, `truncate`, and the wrap family.

use crate::cell::graphemes;
pub use crate::text::WidthMode;

/// A single token produced by [`tokenize`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token<'a> {
    /// A grapheme of visible text with its display width.
    Text {
        /// Grapheme bytes.
        text: &'a [u8],
        /// Display width in terminal cells.
        width: u16,
    },
    /// An ANSI escape sequence (passed through verbatim, no width).
    Escape(&'a [u8]),
    /// A single control byte (C0, DEL, or a non-introducer C1) that isn't
    /// part of an escape sequence.
    Control(u8),
}

/// Return the display width of `bytes` ignoring ANSI escapes.
///
/// Non-UTF-8 bytes contribute no width.
pub fn string_width(bytes: &[u8], mode: WidthMode, eaw_wide: bool) -> usize {
    tokenize(bytes, mode, eaw_wide)
        .filter_map(|t| match t {
            Token::Text { width, .. } => Some(width as usize),
            _ => None,
        })
        .sum()
}

/// Tokenize an input byte slice into ANSI-aware tokens.
pub fn tokenize(bytes: &[u8], mode: WidthMode, eaw_wide: bool) -> Tokenizer<'_> {
    Tokenizer {
        bytes,
        pos: 0,
        mode,
        eaw_wide,
    }
}

/// Iterator returned by [`tokenize`].
pub struct Tokenizer<'a> {
    bytes: &'a [u8],
    pos: usize,
    mode: WidthMode,
    eaw_wide: bool,
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        if self.pos >= self.bytes.len() {
            return None;
        }

        let b = self.bytes[self.pos];

        // 7-bit escape (ESC) and 8-bit C1 introducers open a sequence.
        if b == 0x1b || is_c1_introducer(b) {
            let start = self.pos;
            let end = scan_sequence(self.bytes, start);
            self.pos = end;
            return Some(Token::Escape(&self.bytes[start..end]));
        }

        // C0 controls (incl. DEL) and non-introducer C1 bytes are emitted as
        // single control bytes.
        if b < 0x20 || b == 0x7f || (0x80..=0x9f).contains(&b) {
            self.pos += 1;
            return Some(Token::Control(b));
        }

        // Plain text — walk forward one UTF-8 codepoint at a time, stopping at
        // any byte that would start an escape or control token. Stepping per
        // codepoint guarantees we never confuse a UTF-8 continuation byte
        // (which can be in 0x80..=0xBF) with a C1 control.
        let chunk_start = self.pos;
        let mut chunk_end = self.pos;
        while chunk_end < self.bytes.len() {
            let bb = self.bytes[chunk_end];
            if bb == 0x1b || bb < 0x20 || bb == 0x7f || (0x80..=0x9f).contains(&bb) {
                break;
            }
            let n = utf8_char_len(bb);
            if n == 0 || chunk_end + n > self.bytes.len() {
                break;
            }
            chunk_end += n;
        }
        if chunk_end == chunk_start {
            // Invalid UTF-8 leading byte that wasn't a control — emit it
            // verbatim as a Control byte to keep forward progress.
            self.pos += 1;
            return Some(Token::Control(b));
        }
        let raw = &self.bytes[chunk_start..chunk_end];
        let valid = match std::str::from_utf8(raw) {
            Ok(s) => s,
            Err(e) => {
                let up_to = e.valid_up_to();
                if up_to == 0 {
                    self.pos += 1;
                    return Some(Token::Control(b));
                }
                // SAFETY: validated up to `up_to` by from_utf8.
                unsafe { std::str::from_utf8_unchecked(&raw[..up_to]) }
            }
        };
        let g = graphemes(valid).next()?;
        let g_bytes = g.as_bytes();
        self.pos = chunk_start + g_bytes.len();
        let width = self.mode.grapheme_width(g, self.eaw_wide) as u16;
        Some(Token::Text {
            text: &self.bytes[chunk_start..chunk_start + g_bytes.len()],
            width,
        })
    }
}

#[inline]
fn utf8_char_len(b: u8) -> usize {
    match b {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => 0,
    }
}

#[inline]
fn is_c1_introducer(b: u8) -> bool {
    matches!(b, 0x90 | 0x98 | 0x9b | 0x9d | 0x9e | 0x9f)
}

/// Return the byte index past the end of an escape sequence starting at
/// `start`.
///
/// `bytes[start]` is either `0x1B` (ESC) or an 8-bit C1 sequence introducer
/// (`0x9B`, `0x9D`, `0x90`, `0x98`, `0x9E`, `0x9F`). The returned index points
/// to the first byte after the sequence. If the sequence is incomplete the
/// index is the end of `bytes`.
fn scan_sequence(bytes: &[u8], start: usize) -> usize {
    let len = bytes.len();
    let head = bytes[start];

    match head {
        0x1b => {
            let i = start + 1;
            if i >= len {
                return len;
            }
            match bytes[i] {
                b'[' => scan_csi(bytes, i + 1),
                b']' | b'P' | b'X' | b'^' | b'_' => scan_string(bytes, i + 1),
                _ => scan_esc_intermediate(bytes, i),
            }
        }
        0x9b => scan_csi(bytes, start + 1),
        0x9d | 0x90 | 0x98 | 0x9e | 0x9f => scan_string(bytes, start + 1),
        _ => start + 1,
    }
}

fn scan_csi(bytes: &[u8], from: usize) -> usize {
    let len = bytes.len();
    let mut i = from;
    while i < len {
        if (0x40..=0x7e).contains(&bytes[i]) {
            return i + 1;
        }
        i += 1;
    }
    len
}

fn scan_string(bytes: &[u8], from: usize) -> usize {
    let len = bytes.len();
    let mut i = from;
    while i < len {
        let b = bytes[i];
        if b == 0x07 || b == 0x9c {
            return i + 1;
        }
        if b == 0x1b && i + 1 < len && bytes[i + 1] == b'\\' {
            return i + 2;
        }
        // Lone ESC (without trailing `\`) terminates the string but is left
        // to be re-parsed as the next sequence.
        if b == 0x1b {
            return i;
        }
        i += 1;
    }
    len
}

fn scan_esc_intermediate(bytes: &[u8], at: usize) -> usize {
    let len = bytes.len();
    let mut i = at;
    while i < len && (0x20..=0x2f).contains(&bytes[i]) {
        i += 1;
    }
    if i < len { i + 1 } else { len }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_plain_text() {
        let toks: Vec<_> = tokenize(b"abc", WidthMode::Grapheme, false).collect();
        assert_eq!(toks.len(), 3);
        assert!(matches!(
            toks[0],
            Token::Text {
                text: b"a",
                width: 1
            }
        ));
    }

    #[test]
    fn tokenize_csi() {
        let toks: Vec<_> = tokenize(b"\x1b[31mhi\x1b[m", WidthMode::Grapheme, false).collect();
        assert_eq!(toks.len(), 4);
        assert!(matches!(toks[0], Token::Escape(b"\x1b[31m")));
        assert!(matches!(toks[3], Token::Escape(b"\x1b[m")));
    }

    #[test]
    fn tokenize_osc_bel() {
        let toks: Vec<_> = tokenize(b"\x1b]0;title\x07rest", WidthMode::Grapheme, false).collect();
        assert!(matches!(toks[0], Token::Escape(b"\x1b]0;title\x07")));
    }

    #[test]
    fn tokenize_osc_st() {
        let toks: Vec<_> =
            tokenize(b"\x1b]0;title\x1b\\rest", WidthMode::Grapheme, false).collect();
        assert!(matches!(toks[0], Token::Escape(b"\x1b]0;title\x1b\\")));
    }

    #[test]
    fn tokenize_wide_char() {
        let toks: Vec<_> = tokenize("中".as_bytes(), WidthMode::Grapheme, false).collect();
        assert_eq!(toks.len(), 1);
        assert!(matches!(toks[0], Token::Text { width: 2, .. }));
    }

    #[test]
    fn tokenize_newline_control() {
        let toks: Vec<_> = tokenize(b"a\nb", WidthMode::Grapheme, false).collect();
        assert_eq!(toks.len(), 3);
        assert!(matches!(toks[1], Token::Control(b'\n')));
    }

    #[test]
    fn string_width_ignores_escapes() {
        assert_eq!(
            string_width(b"\x1b[31mhello\x1b[m", WidthMode::Grapheme, false),
            5
        );
        assert_eq!(
            string_width("中文".as_bytes(), WidthMode::Grapheme, false),
            4
        );
    }

    #[test]
    fn tokenize_8bit_csi() {
        let toks: Vec<_> = tokenize(b"\x9b31mhi\x9bm", WidthMode::Grapheme, false).collect();
        assert!(matches!(toks[0], Token::Escape(b"\x9b31m")));
        assert!(matches!(toks[3], Token::Escape(b"\x9bm")));
    }

    #[test]
    fn tokenize_8bit_osc_with_8bit_st() {
        // 0x9d "0;title" 0x9c "rest"
        let toks: Vec<_> = tokenize(b"\x9d0;title\x9crest", WidthMode::Grapheme, false).collect();
        assert!(matches!(toks[0], Token::Escape(b"\x9d0;title\x9c")));
        // "rest" tokenizes to 4 single-grapheme Text tokens.
        assert_eq!(toks.len(), 5);
    }

    #[test]
    fn tokenize_8bit_dcs() {
        let toks: Vec<_> = tokenize(b"\x90q!data\x9cafter", WidthMode::Grapheme, false).collect();
        assert!(matches!(toks[0], Token::Escape(b"\x90q!data\x9c")));
    }

    #[test]
    fn tokenize_8bit_sos_pm_apc() {
        for &intro in &[0x98u8, 0x9e, 0x9f] {
            let mut input = vec![intro];
            input.extend_from_slice(b"payload");
            input.push(0x9c);
            let toks: Vec<_> = tokenize(&input, WidthMode::Grapheme, false).collect();
            match toks[0] {
                Token::Escape(esc) => {
                    assert_eq!(esc[0], intro);
                    assert_eq!(*esc.last().unwrap(), 0x9c);
                }
                ref other => panic!("expected Escape for 0x{intro:02x}, got {other:?}"),
            }
        }
    }

    #[test]
    fn tokenize_standalone_c1_is_control() {
        // IND (0x84) standalone — not an introducer, emitted as Control.
        let toks: Vec<_> = tokenize(b"a\x84b", WidthMode::Grapheme, false).collect();
        assert_eq!(toks.len(), 3);
        assert!(matches!(toks[1], Token::Control(0x84)));
    }

    #[test]
    fn tokenize_8bit_st_outside_string_is_control() {
        // Bare 0x9c with no preceding string is just a control byte.
        let toks: Vec<_> = tokenize(b"\x9c", WidthMode::Grapheme, false).collect();
        assert_eq!(toks.len(), 1);
        assert!(matches!(toks[0], Token::Control(0x9c)));
    }

    #[test]
    fn tokenize_two_byte_esc() {
        // ESC = (DECKPAM)
        let toks: Vec<_> = tokenize(b"a\x1b=b", WidthMode::Grapheme, false).collect();
        assert_eq!(toks.len(), 3);
        assert!(matches!(toks[1], Token::Escape(b"\x1b=")));
    }
}
