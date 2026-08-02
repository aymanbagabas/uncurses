//! ANSI-aware byte-stream tokenizer for text utilities.
//!
//! ## Category
//!
//! The tokenizer classifies an input byte slice as visible grapheme clusters,
//! complete ANSI escape/string sequences, or standalone control bytes. Width,
//! stripping, truncation, and wrapping utilities all build on this stream.
//!
//! ## 7-bit and 8-bit controls
//!
//! Both 7-bit forms (`ESC [`, `ESC ]`, `ESC P`, `ESC _`) and 8-bit C1 forms
//! (`0x9b`, `0x9d`, `0x90`, `0x9f`) are recognized. String controls terminate on
//! BEL, `ST` (`ESC \\`), or 8-bit ST where applicable.
//!
//! ## Mode interaction
//!
//! This module does not interpret terminal modes or sequence semantics. Escape
//! bytes are passed through as zero-width tokens so callers can preserve or drop
//! them according to their own policy.

pub use crate::text::WidthMode;
use crate::unicode::graphemes;

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
        valid_end: 0,
    }
}

/// Iterator returned by [`tokenize`].
pub struct Tokenizer<'a> {
    bytes: &'a [u8],
    pos: usize,
    mode: WidthMode,
    eaw_wide: bool,
    /// How far past `pos` the current run of plain text has been scanned and
    /// found to be valid UTF-8.
    ///
    /// The run is what makes this iterator linear. Finding where the plain
    /// text ends, and validating it, is work proportional to the run's
    /// length, and it is done once for the run rather than once for each
    /// grapheme inside it. `pos` then walks the run a grapheme at a time and
    /// only a run boundary - or a byte that is not valid UTF-8 - makes this
    /// look again.
    valid_end: usize,
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

        // Plain text — walk forward one grapheme at a time, stopping at any
        // byte that would start an escape or control token. Stepping per
        // codepoint when scanning guarantees we never confuse a UTF-8
        // continuation byte (which can be in 0x80..=0xBF) with a C1 control.
        //
        // The scan runs once per *run* of plain text, not once per grapheme
        // in it. Doing it per grapheme is O(run) work O(run) times, which
        // made tokenizing a single long line quadratic in its length - a
        // 32 KB line took 852 ms, and nothing about the tokens it produced
        // changed, so only a timing test could see it.
        if self.pos >= self.valid_end {
            let mut end = self.pos;
            while end < self.bytes.len() {
                let bb = self.bytes[end];
                if bb == 0x1b || bb < 0x20 || bb == 0x7f || (0x80..=0x9f).contains(&bb) {
                    break;
                }
                let n = utf8_char_len(bb);
                if n == 0 || end + n > self.bytes.len() {
                    break;
                }
                end += n;
            }
            self.valid_end = match std::str::from_utf8(&self.bytes[self.pos..end]) {
                Ok(_) => end,
                Err(e) => self.pos + e.valid_up_to(),
            };
        }
        if self.valid_end <= self.pos {
            // Nothing plain starts here, or what does is not valid UTF-8.
            // Either way the byte is emitted verbatim to keep forward
            // progress, exactly as an invalid leading byte always was.
            self.pos += 1;
            return Some(Token::Control(b));
        }
        // Printable ASCII, answered without asking Unicode anything.
        //
        // A cluster can only continue past an ASCII byte with a combining
        // mark, a ZWJ, a variation selector or a regional indicator, and in
        // UTF-8 every one of those starts at 0x80 or above. So an ASCII byte
        // followed by another ASCII byte (or by the end of the text) *is* a
        // whole grapheme cluster, one column wide, and the general path below
        // - grapheme segmentation plus a width table lookup, per character -
        // can only arrive at the same answer far more slowly. `\r\n` is the
        // one multi-byte ASCII cluster and it cannot appear here: both bytes
        // are controls, taken by the branch above.
        //
        // True in either width mode. `Wc` measures the cluster's first code
        // point and `Grapheme` measures the whole cluster; for a lone
        // printable ASCII character those are the same one column. The mode
        // is deliberately not tested here - it was, once, and since `Wc` is
        // the default the fast path then applied to nothing that mattered.
        if b < 0x80 {
            let next = self.bytes.get(self.pos + 1).copied();
            if next.is_none_or(|n| n < 0x80) {
                let start = self.pos;
                self.pos += 1;
                return Some(Token::Text {
                    text: &self.bytes[start..self.pos],
                    width: 1,
                });
            }
        }
        // SAFETY: `pos .. valid_end` was validated as UTF-8 above.
        let valid = unsafe { std::str::from_utf8_unchecked(&self.bytes[self.pos..self.valid_end]) };
        let g = graphemes(valid).next()?;
        let start = self.pos;
        self.pos += g.len();
        let width = self.mode.grapheme_width(g, self.eaw_wide) as u16;
        Some(Token::Text {
            text: &self.bytes[start..self.pos],
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

#[cfg(test)]
mod fast_path {
    use super::*;

    fn text_tokens(s: &str, mode: WidthMode) -> Vec<(String, u16)> {
        tokenize(s.as_bytes(), mode, false)
            .filter_map(|t| match t {
                Token::Text { text, width } => {
                    Some((String::from_utf8_lossy(text).into_owned(), width))
                }
                _ => None,
            })
            .collect()
    }

    /// The ASCII shortcut must agree with grapheme segmentation, cluster for
    /// cluster, in both width modes.
    ///
    /// The shortcut answers "one ASCII byte, one column" without consulting
    /// Unicode at all, which is only sound while nothing can join to an ASCII
    /// base from the byte after it. Everything that can - a combining mark, a
    /// zero-width joiner, a variation selector, a regional indicator - begins
    /// at 0x80 or above in UTF-8, so the shortcut declines whenever the next
    /// byte is not ASCII. These are the cases on either side of that line.
    #[test]
    fn the_ascii_shortcut_agrees_with_grapheme_segmentation() {
        for mode in [WidthMode::Wc, WidthMode::Grapheme] {
            assert_eq!(
                text_tokens("abc", mode),
                vec![("a".into(), 1), ("b".into(), 1), ("c".into(), 1)],
                "plain ASCII is one column per byte"
            );
            // A combining acute joins the `e` before it: one cluster, and the
            // shortcut must not have claimed that `e` on its own.
            assert_eq!(
                text_tokens("e\u{301}f", mode),
                vec![("e\u{301}".into(), 1), ("f".into(), 1)],
                "a combining mark still joins the ASCII base before it"
            );
            assert_eq!(
                text_tokens("a", mode),
                vec![("a".into(), 1)],
                "the last byte of the input takes the shortcut too"
            );
        }
        // The shortcut never applies across a non-ASCII byte, so a wide
        // character keeps its two columns and its neighbours keep one each.
        assert_eq!(
            text_tokens("a\u{4e00}b", WidthMode::Wc),
            vec![("a".into(), 1), ("\u{4e00}".into(), 2), ("b".into(), 1)],
            "a wide character between two ASCII ones is still wide"
        );
    }
}

#[cfg(test)]
mod scaling {
    use super::*;

    /// Tokenizing one long line must cost time proportional to its length.
    ///
    /// This is not a micro-optimisation guard, it is a complexity one. The
    /// tokenizer used to find the end of the current run of text - and
    /// validate it as UTF-8 - once **per grapheme**, so a line of `n`
    /// characters did O(n) work `n` times. Nothing in the output changed, so
    /// no correctness test could see it; what it produced was a renderer
    /// whose frame time depended on the longest line in view. A 32 KB line
    /// (one JSON blob, one base64 payload, one minified file in a tool
    /// result) took 852 ms to wrap, which is a hundred dropped frames for
    /// one entry scrolling past.
    ///
    /// Doubling the input must not much more than double the time. The bound
    /// is generous - 2.8x against an ideal 2.0x - because this is timing on a
    /// shared machine and the failure it guards against is 3.9x.
    #[test]
    fn tokenizing_one_long_line_is_linear_in_its_length() {
        fn nanos(n: usize) -> u128 {
            let line: String = std::iter::repeat_n('x', n).collect();
            // Warm the allocator and the branch predictors, then take the
            // best of three: a slow sample can only come from the machine,
            // never from the code, so the minimum is the honest reading.
            let mut best = u128::MAX;
            for _ in 0..3 {
                let started = std::time::Instant::now();
                let width = string_width(line.as_bytes(), WidthMode::Grapheme, false);
                let elapsed = started.elapsed().as_nanos();
                assert_eq!(width, n, "the width is still the width");
                best = best.min(elapsed);
            }
            best
        }

        // Big enough that the quadratic term dominates the constants, small
        // enough that a red run is still quick.
        let small = nanos(8_000);
        let large = nanos(16_000);
        let ratio = large as f64 / small.max(1) as f64;
        assert!(
            ratio < 2.8,
            "tokenizing twice the line took {ratio:.2}x the time \
             ({small} ns -> {large} ns): the cost is super-linear in line length"
        );
    }
}
