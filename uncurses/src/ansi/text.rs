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

/// Counts the work the tokenizer does, so a test can assert it stays linear in
/// the input without measuring a clock.
///
/// The property that matters is that the scan for a run boundary, and the
/// UTF-8 validation of what it finds, each visit a byte a bounded number of
/// times over a whole tokenization. Timing that instead makes the test a
/// benchmark: on a loaded machine a linear run reads slower than the
/// quadratic threshold it is meant to catch, so it fails at random and passes
/// at random. Counting the visits is exact and takes no wall-clock time.
#[cfg(test)]
pub(crate) mod instrument {
    use std::cell::Cell;

    thread_local! {
        static SCANNED: Cell<usize> = const { Cell::new(0) };
        static VALIDATED: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn scanned(n: usize) {
        SCANNED.with(|c| c.set(c.get() + n));
    }

    pub(super) fn validated(n: usize) {
        VALIDATED.with(|c| c.set(c.get() + n));
    }

    /// Tokenize `bytes` fully and return (bytes visited by the run scan, bytes
    /// submitted to UTF-8 validation).
    ///
    /// The tokens are checked, not discarded. This runs the byte shapes most
    /// likely to desynchronise the run cache, so measuring their cost while
    /// ignoring what they produced would miss a wrong answer that happens to
    /// be cheap.
    pub(crate) fn measure(bytes: &[u8]) -> (usize, usize) {
        SCANNED.with(|c| c.set(0));
        VALIDATED.with(|c| c.set(0));
        let mut rebuilt = Vec::with_capacity(bytes.len());
        for t in super::tokenize(bytes, super::WidthMode::Wc, false) {
            match t {
                super::Token::Text { text, .. } => {
                    assert!(
                        std::str::from_utf8(text).is_ok(),
                        "text token split a character: {text:x?}"
                    );
                    rebuilt.extend_from_slice(text);
                }
                super::Token::Escape(b) => rebuilt.extend_from_slice(b),
                super::Token::Control(c) => rebuilt.push(c),
            }
        }
        assert_eq!(rebuilt, bytes, "tokens did not reassemble the input");
        (SCANNED.with(|c| c.get()), VALIDATED.with(|c| c.get()))
    }
}

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
        scan_end: 0,
        run: "",
    }
}

/// Iterator returned by [`tokenize`].
pub struct Tokenizer<'a> {
    bytes: &'a [u8],
    pos: usize,
    mode: WidthMode,
    eaw_wide: bool,
    /// Where the current run of plain text ends - the next byte that opens an
    /// escape or a control token, or that cannot start a UTF-8 character.
    ///
    /// The run is what makes this iterator linear. Finding where the plain
    /// text ends is work proportional to the run's length, and it is done
    /// once for the run rather than once for each grapheme inside it. `pos`
    /// then walks the run a grapheme at a time and only crossing `scan_end`
    /// makes this look again.
    scan_end: usize,
    /// The validated remainder of the run, starting at `pos`.
    ///
    /// Held separately from `scan_end` on purpose. Malformed UTF-8 ends this
    /// slice early but says nothing about where the *run* ends, and conflating
    /// the two made every malformed byte rescan the rest of the input: text
    /// that was one byte of UTF-8 short cost O(n^2) to tokenize. Keeping the
    /// two boundaries apart means the scan still happens once per run, and
    /// only the validation - which stops at the first bad byte, so it is cheap
    /// exactly when it is repeated - runs again.
    run: &'a str,
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
        if self.pos >= self.scan_end {
            let mut end = self.pos;
            while end < self.bytes.len() {
                let bb = self.bytes[end];
                if bb == 0x1b || bb < 0x20 || bb == 0x7f || (0x80..=0x9f).contains(&bb) {
                    break;
                }
                match utf8_char_at(self.bytes, end) {
                    Some(n) => end += n,
                    None => break,
                }
            }
            #[cfg(test)]
            instrument::scanned(end.max(self.pos + 1) - self.pos);
            self.scan_end = end;
            // Valid by construction: the scan stopped at the first byte that
            // does not begin a well-formed character, so this cannot fail.
            #[cfg(test)]
            instrument::validated(end - self.pos);
            self.run = std::str::from_utf8(&self.bytes[self.pos..end]).unwrap_or("");
        }
        if self.run.is_empty() {
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
                self.run = &self.run[1..];
                return Some(Token::Text {
                    text: &self.bytes[start..self.pos],
                    width: 1,
                });
            }
        }
        let g = graphemes(self.run).next()?;
        let start = self.pos;
        self.pos += g.len();
        self.run = &self.run[g.len()..];
        let width = self.mode.grapheme_width(g, self.eaw_wide) as u16;
        Some(Token::Text {
            text: &self.bytes[start..self.pos],
            width,
        })
    }
}

/// The length of the well-formed UTF-8 character starting at `i`, if there is
/// one.
///
/// A lead byte announces how many bytes follow it, and nothing else in this
/// file may take that announcement on trust. Bytes that do not follow are the
/// whole problem: a lead byte with the wrong continuations makes a naive walk
/// step *over* whatever comes next, which is how a scan can pass a terminator
/// it should have stopped at, and how the tokenizer's two boundaries came to
/// disagree about where a character starts.
fn utf8_char_at(bytes: &[u8], i: usize) -> Option<usize> {
    let n = utf8_char_len(bytes[i]);
    if n == 0 || i + n > bytes.len() {
        return None;
    }
    // A one-byte character is ASCII, and `utf8_char_len` already established
    // that; anything longer has to prove its continuations.
    if n > 1 && std::str::from_utf8(&bytes[i..i + n]).is_err() {
        return None;
    }
    Some(n)
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
        // `U+009C` is 8-bit ST as well, and its two-byte UTF-8 form is the
        // only way one can appear in a `&str` - a raw `0x9C` byte cannot.
        // Callers building sequences in Rust source write it as `"\u{9c}"`,
        // and reading that as ordinary text swallows the rest of the string.
        if b == 0xc2 && bytes.get(i + 1) == Some(&0x9c) {
            return i + 2;
        }
        if b == 0x1b && i + 1 < len && bytes[i + 1] == b'\\' {
            return i + 2;
        }
        // Lone ESC (without trailing `\`) terminates the string but is left
        // to be re-parsed as the next sequence.
        if b == 0x1b {
            return i;
        }
        // Step a whole UTF-8 character at a time. `0x9C` is a continuation
        // byte as well as 8-bit ST, and stepping byte by byte read it as a
        // terminator in the middle of a character: every code point in
        // U+2700..U+273F - "✅", "✔", "✨" - ends an OSC title early and
        // leaves its trailing bytes orphaned outside the sequence. As a lead
        // byte 0x9C is never valid, so a 0x9C on a character boundary is
        // still unambiguously ST.
        //
        // Only a *well-formed* character is stepped whole. Trusting a lead
        // byte whose continuations do not match would step over whatever
        // followed it, terminators included, so a single malformed byte in a
        // payload would swallow the BEL that ends it and all the text after.
        i += utf8_char_at(bytes, i).unwrap_or(1);
    }
    len
}

fn scan_esc_intermediate(bytes: &[u8], at: usize) -> usize {
    let len = bytes.len();
    let mut i = at;
    while i < len && (0x20..=0x2f).contains(&bytes[i]) {
        i += 1;
    }
    if i < len {
        // The final byte, or the whole character it leads - splitting one
        // strands continuation bytes that the next token reads as C1
        // controls.
        i + utf8_char_at(bytes, i).unwrap_or(1)
    } else {
        len
    }
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

    /// The visible text of `s`, which is what a caller loses when a sequence
    /// runs past its terminator.
    fn strip_for_test(s: &str) -> String {
        tokenize(s.as_bytes(), WidthMode::Wc, false)
            .filter_map(|t| match t {
                Token::Text { text, .. } => Some(String::from_utf8_lossy(text).into_owned()),
                _ => None,
            })
            .collect()
    }

    fn text_tokens_eaw(s: &str, mode: WidthMode, eaw_wide: bool) -> Vec<(String, u16)> {
        tokenize(s.as_bytes(), mode, eaw_wide)
            .filter_map(|t| match t {
                // Not lossy: a token that is not valid UTF-8 is a bug, and
                // rendering it as U+FFFD hides exactly the failure these
                // tests exist to catch.
                Token::Text { text, width } => Some((
                    std::str::from_utf8(text)
                        .expect("a text token is whole UTF-8")
                        .to_owned(),
                    width,
                )),
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
            for eaw_wide in [false, true] {
                let toks = |s: &str| text_tokens_eaw(s, mode, eaw_wide);
                assert_eq!(
                    toks("abc"),
                    vec![("a".into(), 1), ("b".into(), 1), ("c".into(), 1)],
                    "plain ASCII is one column per byte"
                );
                // A combining acute joins the `e` before it: one cluster, and
                // the shortcut must not have claimed that `e` on its own.
                assert_eq!(
                    toks("e\u{301}f"),
                    vec![("e\u{301}".into(), 1), ("f".into(), 1)],
                    "a combining mark still joins the ASCII base before it"
                );
                assert_eq!(
                    toks("a"),
                    vec![("a".into(), 1)],
                    "the last byte of the input takes the shortcut too"
                );
                // A variation selector and a keycap sequence both extend an
                // ASCII base from bytes at 0x80 and above.
                assert_eq!(
                    toks("1\u{fe0f}\u{20e3}z"),
                    vec![("1\u{fe0f}\u{20e3}".into(), 1), ("z".into(), 1)],
                    "a keycap keeps its ASCII digit"
                );
                // A zero-width joiner cannot follow ASCII in any real text,
                // but it must not be able to strand one either.
                assert_eq!(
                    toks("a\u{200d}b"),
                    vec![("a\u{200d}".into(), 1), ("b".into(), 1)],
                    "a ZWJ joins the ASCII base before it"
                );
                // The shortcut never applies across a non-ASCII byte, so a
                // wide character keeps its two columns and its neighbours
                // keep one each.
                assert_eq!(
                    toks("a\u{4e00}b"),
                    vec![("a".into(), 1), ("\u{4e00}".into(), 2), ("b".into(), 1)],
                    "a wide character between two ASCII ones is still wide"
                );
            }
        }
        // The ambiguous-width block is the only thing `eaw_wide` moves, and
        // it must not move the ASCII on either side of it.
        assert_eq!(
            text_tokens_eaw("a\u{2018}b", WidthMode::Wc, false),
            vec![("a".into(), 1), ("\u{2018}".into(), 1), ("b".into(), 1)]
        );
        assert_eq!(
            text_tokens_eaw("a\u{2018}b", WidthMode::Wc, true),
            vec![("a".into(), 1), ("\u{2018}".into(), 2), ("b".into(), 1)]
        );
    }

    /// Every byte the fast path can see is printable ASCII.
    ///
    /// The shortcut answers "one column" for any byte below 0x80, which would
    /// be wrong for the C0 controls and DEL - they are zero columns, not one.
    /// It is correct because it never sees them: they are taken by the
    /// control branch above it. This pins that ordering, which is the only
    /// thing keeping the shortcut honest.
    #[test]
    fn controls_never_reach_the_ascii_shortcut() {
        for b in 0u8..=0x7f {
            let input = [b];
            let toks: Vec<_> = tokenize(&input, WidthMode::Wc, false).collect();
            match toks.as_slice() {
                [Token::Text { text, width }] => {
                    assert!(
                        (0x20..=0x7e).contains(&b),
                        "0x{b:02x} was emitted as text, but only printable ASCII may be"
                    );
                    assert_eq!(*text, &input[..]);
                    assert_eq!(*width, 1);
                }
                [Token::Control(c)] => {
                    assert_eq!(*c, b);
                    assert!(
                        b < 0x20 || b == 0x7f,
                        "0x{b:02x} is printable but was emitted as a control"
                    );
                }
                // A bare ESC opens a sequence with nothing in it.
                [Token::Escape(seq)] => assert_eq!(*seq, b"\x1b"),
                other => panic!("0x{b:02x} produced {other:?}"),
            }
        }
    }

    /// An OSC payload may contain any UTF-8, and `0x9C` appears inside a great
    /// many characters as a continuation byte. Terminating the string there
    /// cut those characters in half, which left the trailing bytes of one
    /// outside the escape token - where the rest of the crate, reasonably,
    /// treated a token it had been told was text as UTF-8.
    #[test]
    fn a_continuation_byte_does_not_terminate_a_string_sequence() {
        // Every scanner that steps through a payload, not just the OSC one.
        // `scan_esc_intermediate` has the same bug and the same fix, and a
        // test that only builds OSC titles leaves half of it uncovered.
        const PREFIXES: &[&str] = &[
            "\u{1b}]0;",
            "\u{1b}P",
            "\u{1b}_",
            "\u{1b}^",
            "\u{1b}X",
            "\u{1b}",
            "\u{1b} ",
            "\u{1b}#",
        ];
        for c in [
            '\u{2705}',
            '\u{2714}',
            '\u{2728}',
            '\u{171c}',
            '\u{4e00}',
            '\u{1f600}',
        ] {
            for prefix in PREFIXES {
                let input = format!("{prefix}{c}\u{7}after");
                for t in tokenize(input.as_bytes(), WidthMode::Wc, false) {
                    let bytes = match t {
                        Token::Text { text, .. } | Token::Escape(text) => text,
                        Token::Control(_) => continue,
                    };
                    assert!(
                        std::str::from_utf8(bytes).is_ok(),
                        "{input:?} produced a token split mid-character: {bytes:x?}"
                    );
                }
            }
        }
        // The OSC case in full: the sequence stays whole and the text after it
        // survives.
        for c in ['\u{2705}', '\u{2714}', '\u{2728}', '\u{171c}'] {
            let input = format!("\x1b]0;Build {c}\x07after");
            let toks: Vec<_> = tokenize(input.as_bytes(), WidthMode::Wc, false).collect();
            let (escape, rest) = toks.split_first().expect("a token");
            let Token::Escape(seq) = escape else {
                panic!("expected the OSC to be one escape token, got {escape:?}")
            };
            assert_eq!(
                std::str::from_utf8(seq).expect("the sequence is whole"),
                format!("\x1b]0;Build {c}\x07")
            );
            let text: String = rest
                .iter()
                .filter_map(|t| match t {
                    Token::Text { text, .. } => std::str::from_utf8(text).ok(),
                    _ => None,
                })
                .collect();
            assert_eq!(text, "after");
        }
        // A bare ESC takes the whole character after it, not its lead byte.
        let toks: Vec<_> = tokenize("\u{1b}\u{2705}x".as_bytes(), WidthMode::Wc, false).collect();
        assert_eq!(toks[0], Token::Escape("\u{1b}\u{2705}".as_bytes()));
        // A `0x9C` on a character boundary is still an 8-bit ST.
        let toks: Vec<_> = tokenize(b"\x1b]0;t\x9cx", WidthMode::Wc, false).collect();
        assert!(matches!(toks[0], Token::Escape(b"\x1b]0;t\x9c")));
    }

    /// `U+009C` is 8-bit ST, and in a `&str` its two-byte UTF-8 form is the
    /// only form it can take - a raw `0x9C` byte is not valid UTF-8. Reading
    /// it as ordinary text swallows the terminator and every visible
    /// character after it.
    #[test]
    fn the_utf8_form_of_8_bit_st_terminates_a_string_sequence() {
        for s in [
            "\u{1b}]0;title\u{9c}visible text",
            "\u{1b}Pq#0;2\u{9c}visible text",
            "\u{1b}_payload\u{9c}visible text",
        ] {
            assert_eq!(strip_for_test(s), "visible text", "input {s:?}");
        }
        // The same byte pair inside a longer character is not a terminator:
        // "\u{2705}" is E2 9C 85 and has no C2 lead.
        assert_eq!(
            strip_for_test("\u{1b}]0;\u{2705}\u{7}visible text"),
            "visible text"
        );
    }

    /// A malformed byte in a payload must not eat the terminator.
    ///
    /// Stepping a whole character on the strength of a lead byte alone steps
    /// over whatever actually follows it. A stray high byte then swallowed
    /// the BEL or `ESC \` that ends the sequence, and with it every visible
    /// character after - and could consume a following legitimate CSI whole.
    #[test]
    fn a_malformed_byte_does_not_swallow_a_terminator() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"\x1b]0;\xe0\x07Zz", b"\x1b]0;\xe0\x07"),
            (b"\x1b]0;\xf0\x1b\\Zz", b"\x1b]0;\xf0\x1b\\"),
            (b"\x1b]0;\xe0\x9cZz", b"\x1b]0;\xe0\x9c"),
            (b"\x1b_G\xf0\x9f\x1b\\Zz", b"\x1b_G\xf0\x9f\x1b\\"),
            (b"\x1b]0;\xc2\x07visible", b"\x1b]0;\xc2\x07"),
        ];
        for (input, escape) in cases {
            let toks: Vec<_> = tokenize(input, WidthMode::Wc, false).collect();
            assert_eq!(
                toks.first(),
                Some(&Token::Escape(escape)),
                "input {input:x?} did not stop at its terminator"
            );
            let visible: Vec<u8> = toks
                .iter()
                .filter_map(|t| match t {
                    Token::Text { text, .. } => Some(*text),
                    _ => None,
                })
                .flatten()
                .copied()
                .collect();
            assert!(!visible.is_empty(), "input {input:x?} lost all its text");
        }
        // A bare ESC must not consume the CSI that follows a malformed byte.
        let toks: Vec<_> = tokenize(b"\x1b\xe0\x1b[31mZ", WidthMode::Wc, false).collect();
        assert_eq!(toks[0], Token::Escape(b"\x1b\xe0"));
        assert_eq!(toks[1], Token::Escape(b"\x1b[31m"));
    }

    /// Arbitrary bytes must not panic, and must not produce a token that
    /// splits a character.
    ///
    /// `tokenize` and `string_width` take `&[u8]`, so this is reachable from
    /// the public API by anyone reading a PTY. The run cache holds a byte
    /// offset and a validated `&str` and they have to agree about where a
    /// character starts; when the scan trusted a lead byte its continuations
    /// did not match, they disagreed, and slicing the run panicked.
    #[test]
    fn arbitrary_bytes_never_panic_or_split_a_character() {
        // The token stream is pinned, not just checked for well-formedness.
        // The desync had a quieter failure mode than the panic: a run cache
        // that disagrees with itself demotes perfectly good characters to
        // `Control` bytes, which round-trips and splits nothing while losing
        // every character after the first bad byte. These are the merge-base
        // streams, byte for byte.
        let pinned: &[(&[u8], &str)] = &[
            (
                b"\xe0\x20\x0d\xef\xb8\x8f",
                "Ce0 T[20]w1 C0d T[ef, b8, 8f]w0",
            ),
            (
                b"\xf0\x30\x0d\x5b\xe2\x9c\x85\x30",
                "Cf0 T[30]w1 C0d T[5b]w1 T[e2, 9c, 85]w2 T[30]w1",
            ),
            (
                b"\xe0\x41\x1b\x41\xc3\xa9",
                "Ce0 T[41]w1 E[1b, 41] T[c3, a9]w1",
            ),
            (b"\xcf\xc8\x9c", "Ccf T[c8, 9c]w1"),
            (
                b"\xe3\x61\x0a\xc4\x81\xc4\x81\x78",
                "Ce3 T[61]w1 C0a T[c4, 81]w1 T[c4, 81]w1 T[78]w1",
            ),
        ];
        for (seed, expected) in pinned {
            let rendered: Vec<String> = tokenize(seed, WidthMode::Grapheme, false)
                .map(|t| match t {
                    Token::Text { text, width } => format!("T{text:x?}w{width}"),
                    Token::Escape(b) => format!("E{b:x?}"),
                    Token::Control(c) => format!("C{c:02x}"),
                })
                .collect();
            assert_eq!(rendered.join(" "), *expected, "input {seed:x?}");
            string_width(seed, WidthMode::Grapheme, false);
            string_width(seed, WidthMode::Wc, false);
        }

        // A cheap xorshift beats no fuzzing at all; the seeds above are the
        // cases it found, kept so a failure names itself.
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut buf = [0u8; 24];
        for _ in 0..200_000 {
            let len = {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state % 24) as usize
            };
            for b in buf.iter_mut().take(len) {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                // Weighted towards the bytes that break things: lead bytes,
                // continuations, introducers, controls.
                *b =
                    match state % 4 {
                        0 => (state >> 8) as u8,
                        1 => [0x1b, 0x9b, 0x9d, 0x90, 0x98, 0x9e, 0x9f, 0x07]
                            [(state >> 8) as usize % 8],
                        2 => [0xc2, 0xe0, 0xe2, 0xf0, 0xf4, 0xff, 0x9c, 0x80]
                            [(state >> 8) as usize % 8],
                        _ => b"abc \n\r\\;0"[(state >> 8) as usize % 9],
                    };
            }
            let input = &buf[..len];
            for t in tokenize(input, WidthMode::Grapheme, false) {
                if let Token::Text { text, .. } = t {
                    assert!(
                        std::str::from_utf8(text).is_ok(),
                        "{input:x?} produced a split character {text:x?}"
                    );
                }
            }
            string_width(input, WidthMode::Grapheme, false);
        }
    }
}

#[cfg(test)]
mod scaling {
    use super::*;

    /// Tokenizing one long line must cost work proportional to its length.
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
    /// The work is counted rather than timed. A timing test here is a
    /// benchmark wearing a test's clothes: on a machine running anything else
    /// the linear code reads slower than the quadratic threshold it is meant
    /// to catch, so it fails for reasons that have nothing to do with the
    /// code. Byte visits are exact, identical on every machine, and are the
    /// thing that actually went quadratic.
    #[test]
    fn tokenizing_one_long_line_is_linear_in_its_length() {
        for n in [1_000usize, 4_000, 16_000, 64_000] {
            let line: String = std::iter::repeat_n('x', n).collect();
            let (scanned, validated) = instrument::measure(line.as_bytes());
            assert_eq!(scanned, n, "the run is found once, in one pass");
            assert_eq!(validated, n, "and validated once");
        }
    }

    /// The same bound with the UTF-8 broken.
    ///
    /// This is the case the first version of the run cache missed. It cached
    /// one boundary for two questions - where the run ends, and how much of
    /// it is valid UTF-8 - so a single malformed byte invalidated the run and
    /// sent the *scan* back over the rest of the input, once per byte. The
    /// tokens were right and the timing test above passed, because its input
    /// is valid; only text that was one byte short of valid stayed quadratic.
    #[test]
    fn malformed_utf8_is_linear_too() {
        for n in [1_000usize, 4_000, 16_000, 64_000] {
            // `C2` opens a two-byte character that `41` cannot continue: a
            // malformed byte every two bytes, all the way through.
            let bytes: Vec<u8> = std::iter::repeat_n([0xc2u8, 0x41], n / 2)
                .flatten()
                .collect();
            let (scanned, validated) = instrument::measure(&bytes);
            assert!(
                scanned <= 2 * n,
                "scanning {n} malformed bytes visited {scanned} of them"
            );
            assert!(
                validated <= 4 * n,
                "validating {n} malformed bytes submitted {validated} bytes"
            );
        }
    }
}
