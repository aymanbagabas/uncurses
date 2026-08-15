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
        scan_end: 0,
        run: "",
        #[cfg(test)]
        scanned: 0,
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
    /// Bytes visited by the run scan, which is the thing that went quadratic.
    ///
    /// It is not visible in the tokens - that is why the bug survived a full
    /// test suite - and timing it is a benchmark, not a test. So the tests
    /// count it.
    #[cfg(test)]
    scanned: usize,
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
            {
                self.scanned += end.max(self.pos + 1) - self.pos;
            }
            self.scan_end = end;
            // Valid by construction: the scan stopped at the first byte that
            // does not begin a well-formed character, so this cannot fail.
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
                // The `true` is "BEL ends this": OSC only.
                b']' => scan_string(bytes, i + 1, true),
                b'P' | b'X' | b'^' | b'_' => scan_string(bytes, i + 1, false),
                _ => scan_esc_intermediate(bytes, i),
            }
        }
        0x9b => scan_csi(bytes, start + 1),
        0x9d => scan_string(bytes, start + 1, true),
        0x90 | 0x98 | 0x9e | 0x9f => scan_string(bytes, start + 1, false),
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

/// Scan a control string to its terminator, which is ST - and also `BEL` if
/// `bel_ends`, that being an xterm convention for OSC and no other string.
///
/// Terminators are only ever tested at a character boundary. `0x9C` is 8-bit
/// ST between characters and a continuation byte inside one, and every code
/// point in U+2700..U+273F carries one, so a scan that does not know which it
/// is looking at ends the sequence in the middle of "✅".
fn scan_string(bytes: &[u8], from: usize, bel_ends: bool) -> usize {
    let len = bytes.len();
    let mut i = from;
    while i < len {
        let b = bytes[i];
        // At a boundary, so this is C1 ST and not a continuation byte.
        if b == 0x9c || (b == 0x07 && bel_ends) {
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
        // Collect a whole character, so the bytes inside it are never tested
        // as terminators. Only a *well-formed* one: trusting a lead byte
        // whose continuations do not match would step over whatever followed
        // it, terminators included, and one malformed byte in a payload would
        // swallow the BEL that ends it and all the text after.
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

/// The five string sequences - OSC, DCS, SOS, PM, APC - across both control
/// forms, every terminator, and payloads that do and do not contain UTF-8.
///
/// These share one scanner, so they share one bug. The `0x9C` split that made
/// this file's `from_utf8_unchecked` calls undefined behaviour was found in an
/// OSC title, but it was never an OSC bug: DCS, SOS, PM and APC all reached
/// the same code, and only OSC was tested. A payload is arbitrary text, so any
/// of them can carry a character whose continuation bytes look like a
/// terminator.
#[cfg(test)]
mod sequences {
    use super::*;

    /// `(name, 7-bit introducer, 8-bit introducer)`.
    const STRINGS: &[(&str, &[u8], u8)] = &[
        ("OSC", b"\x1b]", 0x9d),
        ("DCS", b"\x1bP", 0x90),
        ("SOS", b"\x1bX", 0x98),
        ("PM", b"\x1b^", 0x9e),
        ("APC", b"\x1b_", 0x9f),
    ];

    /// Every way a string sequence may end.
    ///
    /// `BEL` is in here for OSC only; the loop skips it elsewhere.
    const TERMINATORS: &[(&str, &[u8])] = &[
        ("BEL", b"\x07"),
        ("ESC backslash", b"\x1b\\"),
        ("8-bit ST", b"\x9c"),
    ];

    /// Payloads with and without UTF-8. The non-ASCII one is chosen for the
    /// bytes it contains, not for how it reads: `é` is `C3 A9`, `✅` is
    /// `E2 9C 85` and carries a `0x9C`, `一` is `E4 B8 80`, and `😀` is a
    /// four-byte character.
    const PAYLOADS: &[(&str, &str)] = &[
        ("ascii", "0;plain title"),
        ("utf8", "0;caf\u{e9} \u{2705} \u{4e00} \u{1f600}"),
    ];

    fn tokens(input: &[u8]) -> Vec<Token<'_>> {
        tokenize(input, WidthMode::Grapheme, false).collect()
    }

    /// Whatever the type, the form, the payload or the terminator, the whole
    /// sequence is exactly one zero-width token and the text after it
    /// survives intact.
    #[test]
    fn every_string_sequence_is_one_whole_token() {
        for (name, seven, eight) in STRINGS {
            for (pname, payload) in PAYLOADS {
                for (tname, term) in TERMINATORS {
                    if *term == b"\x07" && *name != "OSC" {
                        continue;
                    }
                    for (form, intro) in [("7-bit", seven.to_vec()), ("8-bit", vec![*eight])] {
                        let mut input = intro;
                        input.extend_from_slice(payload.as_bytes());
                        input.extend_from_slice(term);
                        let seq_len = input.len();
                        input.extend_from_slice("after \u{2705}".as_bytes());

                        let what = format!("{name} {form} {pname} payload, {tname} terminator");
                        let toks = tokens(&input);
                        assert_eq!(
                            toks.first(),
                            Some(&Token::Escape(&input[..seq_len])),
                            "{what}: the sequence is not one token"
                        );
                        let visible: String = toks[1..]
                            .iter()
                            .filter_map(|t| match t {
                                Token::Text { text, .. } => {
                                    Some(std::str::from_utf8(text).expect("a whole character"))
                                }
                                _ => None,
                            })
                            .collect();
                        assert_eq!(visible, "after \u{2705}", "{what}: text after was lost");
                        // The sequence itself is invisible.
                        assert_eq!(
                            string_width(&input, WidthMode::Grapheme, false),
                            8,
                            "{what}: the sequence was measured"
                        );
                    }
                }
            }
        }
    }

    /// A payload that runs off the end is still one token, not a stream of
    /// stray control bytes.
    #[test]
    fn an_unterminated_string_sequence_runs_to_the_end() {
        for (name, seven, eight) in STRINGS {
            for (form, intro) in [("7-bit", seven.to_vec()), ("8-bit", vec![*eight])] {
                for (pname, payload) in PAYLOADS {
                    let mut input = intro.clone();
                    input.extend_from_slice(payload.as_bytes());
                    let toks = tokens(&input);
                    assert_eq!(
                        toks,
                        vec![Token::Escape(&input[..])],
                        "{name} {form} {pname}: an unterminated sequence should be one token"
                    );
                    assert_eq!(string_width(&input, WidthMode::Grapheme, false), 0);
                }
            }
        }
    }

    /// A lone ESC ends the string and is left to open the next sequence, so a
    /// sequence cannot swallow the one that follows it.
    #[test]
    fn a_lone_esc_ends_a_string_and_is_reparsed() {
        for (name, seven, eight) in STRINGS {
            for (form, intro) in [("7-bit", seven.to_vec()), ("8-bit", vec![*eight])] {
                let mut input = intro.clone();
                input.extend_from_slice("pay\u{2705}".as_bytes());
                let cut = input.len();
                input.extend_from_slice(b"\x1b[31mZ");

                let toks = tokens(&input);
                assert_eq!(
                    toks[0],
                    Token::Escape(&input[..cut]),
                    "{name} {form}: the string should stop at the ESC"
                );
                assert_eq!(
                    toks[1],
                    Token::Escape(b"\x1b[31m"),
                    "{name} {form}: the CSI after it should survive whole"
                );
                assert_eq!(
                    toks[2],
                    Token::Text {
                        text: b"Z",
                        width: 1
                    }
                );
            }
        }
    }

    /// Two sequences back to back, in either control form, stay two.
    #[test]
    fn adjacent_sequences_do_not_merge() {
        // (input, first sequence, second sequence); the input names the case.
        let cases: &[(&[u8], &[u8], &[u8])] = &[
            // 7-bit OSC then 7-bit DCS.
            (
                b"\x1b]0;a\x07\x1bPq\x1b\\Z",
                b"\x1b]0;a\x07",
                b"\x1bPq\x1b\\",
            ),
            // 8-bit APC then 8-bit PM.
            (b"\x9fa\x9c\x9eb\x9cZ", b"\x9fa\x9c", b"\x9eb\x9c"),
            // An 8-bit introducer closed by 7-bit ST, then a 7-bit introducer
            // closed by 8-bit ST: both mixed forms, adjacent.
            (b"\x9d0;a\x1b\\\x1b^b\x9cZ", b"\x9d0;a\x1b\\", b"\x1b^b\x9c"),
        ];
        for (input, first, second) in cases {
            let toks = tokens(input);
            assert_eq!(toks[0], Token::Escape(first), "{input:x?}");
            assert_eq!(toks[1], Token::Escape(second), "{input:x?}");
            assert_eq!(
                toks[2],
                Token::Text {
                    text: b"Z",
                    width: 1
                },
                "{input:x?}"
            );
        }
    }

    /// Sequences as they actually arrive from terminals and applications.
    #[test]
    fn real_payloads_survive() {
        // (what, input, the opening sequence, the visible text)
        let cases: &[(&str, &[u8], &[u8], &str)] = &[
            (
                "sixel",
                b"\x1bPq#0;2;0;0;0#1;2;100;100;100\x1b\\ok",
                b"\x1bPq#0;2;0;0;0#1;2;100;100;100\x1b\\",
                "ok",
            ),
            (
                "DECRQSS reply",
                b"\x1bP1$r0;1m\x1b\\ok",
                b"\x1bP1$r0;1m\x1b\\",
                "ok",
            ),
            (
                "OSC 8 hyperlink with a UTF-8 target",
                b"\x1b]8;;https://example.com/\xe2\x9c\x85\x07link\x1b]8;;\x07",
                b"\x1b]8;;https://example.com/\xe2\x9c\x85\x07",
                "link",
            ),
            (
                "OSC with an empty payload",
                b"\x1b]\x07ok",
                b"\x1b]\x07",
                "ok",
            ),
            (
                "8-bit APC with an empty payload",
                b"\x9f\x9cok",
                b"\x9f\x9c",
                "ok",
            ),
        ];
        for (what, input, opening, visible) in cases {
            let toks = tokens(input);
            assert_eq!(toks.first(), Some(&Token::Escape(opening)), "{what}");
            let got: String = toks
                .iter()
                .filter_map(|t| match t {
                    Token::Text { text, .. } => {
                        Some(std::str::from_utf8(text).expect("a whole character"))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(got, *visible, "{what}");
            // Nothing is invented or dropped anywhere in the stream.
            let mut rebuilt = Vec::new();
            for t in &toks {
                match t {
                    Token::Text { text, .. } | Token::Escape(text) => {
                        rebuilt.extend_from_slice(text)
                    }
                    Token::Control(c) => rebuilt.push(*c),
                }
            }
            assert_eq!(rebuilt, *input, "{what}: the stream did not reassemble");
        }
    }

    /// A byte in `0x80..=0x9F` is a C1 control between characters and a
    /// continuation byte inside one. Which it is depends only on the decoder's
    /// state, never on what a caller meant.
    ///
    /// So `C2 9C` is the character U+009C - a lead byte, then a continuation
    /// byte collected as part of it - and not 8-bit ST, which is the single
    /// byte `9C`. Likewise `C2 9D` is the character U+009D and does not open
    /// an OSC. A `&str` cannot hold a raw C1 byte at all, which is what 7-bit
    /// `ESC \` and `ESC ]` are for.
    #[test]
    fn a_c1_byte_inside_a_character_is_not_a_control() {
        // Between characters: 8-bit ST, and the sequence ends.
        let toks = tokens(b"\x1b]0;title\x9cZ");
        assert_eq!(toks[0], Token::Escape(b"\x1b]0;title\x9c"));
        assert_eq!(
            toks[1],
            Token::Text {
                text: b"Z",
                width: 1
            }
        );

        // Inside a character: a continuation byte, and the sequence runs on.
        // `C2 9C` is the character U+009C - what `"\u{9c}"` would compile to -
        // so the OSC here is simply unterminated.
        let unterminated = b"\x1b]0;title\xc2\x9cZ";
        assert_eq!(tokens(unterminated), vec![Token::Escape(&unterminated[..])]);

        // The same for an introducer: the byte opens a sequence, the
        // character does not.
        let toks = tokens(b"\x9d0;title\x07Z");
        assert_eq!(toks[0], Token::Escape(b"\x9d0;title\x07"));
        let toks = tokens(b"\xc2\x9d0;title\x07Z");
        assert!(
            !matches!(toks[0], Token::Escape(_)),
            "the character U+009D opened a sequence: {toks:?}"
        );
        // U+009D measures zero, `0;title` seven, BEL zero, `Z` one.
        assert_eq!(
            string_width(b"\xc2\x9d0;title\x07Z", WidthMode::Wc, false),
            8
        );

        // And the case that made this matter: the `9C` inside "✅" is a
        // continuation byte, so the title survives whole.
        let toks = tokens(b"\x1b]0;\xe2\x9c\x85\x07Z");
        assert_eq!(toks[0], Token::Escape(b"\x1b]0;\xe2\x9c\x85\x07"));
    }

    /// `BEL` ends an OSC and only an OSC.
    ///
    /// `OSC Ps ; Pt BEL` is an xterm convention, and the common way to write
    /// one, but it is not a rule about control strings in general - ECMA-48
    /// gives them all a single terminator, ST. A `0x07` inside a DCS, SOS, PM
    /// or APC is payload, and ending the sequence there would spill the rest
    /// of that payload onto the screen as visible text.
    #[test]
    fn bel_ends_an_osc_and_nothing_else() {
        for (name, seven, eight) in STRINGS {
            for (form, intro) in [("7-bit", seven.to_vec()), ("8-bit", vec![*eight])] {
                let mut input = intro;
                input.extend_from_slice(b"pay\x07load");
                let bel_at = input.len() - b"load".len();
                input.extend_from_slice(b"\x9cZ");

                let toks = tokens(&input);
                let what = format!("{name} {form}");
                if *name == "OSC" {
                    assert_eq!(
                        toks[0],
                        Token::Escape(&input[..bel_at]),
                        "{what}: BEL should have ended it"
                    );
                    // What followed the BEL is now visible text.
                    assert!(
                        toks.iter()
                            .any(|t| matches!(t, Token::Text { text: b"l", .. })),
                        "{what}: text after the BEL went missing"
                    );
                } else {
                    assert_eq!(
                        toks[0],
                        Token::Escape(&input[..input.len() - 1]),
                        "{what}: BEL is payload, so it should have run to the ST"
                    );
                    assert_eq!(
                        toks[1],
                        Token::Text {
                            text: b"Z",
                            width: 1
                        },
                        "{what}"
                    );
                    assert_eq!(string_width(&input, WidthMode::Grapheme, false), 1);
                }
            }
        }
    }

    /// The rule, over every C1 byte rather than the two that caused trouble.
    ///
    /// `C2 xx` encodes U+0080..U+00BF, so for any C1 byte there is a
    /// character that carries it as a continuation byte. On its own the byte
    /// is a control; inside that character it is not.
    #[test]
    fn every_c1_byte_depends_on_the_decoder_state() {
        for c in 0x80u8..=0x9f {
            // Between characters: a control, either standalone or opening a
            // sequence.
            let single = [c];
            let alone: Vec<Token<'_>> = tokenize(&single, WidthMode::Wc, false).collect();
            match alone.as_slice() {
                [Token::Control(got)] => assert_eq!(*got, c),
                [Token::Escape(seq)] => assert!(
                    is_c1_introducer(c) && *seq == &single[..],
                    "0x{c:02x} opened a sequence but is not an introducer"
                ),
                other => panic!("0x{c:02x} alone produced {other:?}"),
            }
            assert_eq!(string_width(&single, WidthMode::Wc, false), 0);

            // Inside a character: one zero-width text token, no control.
            let encoded = [0xc2, c];
            let inside: Vec<Token<'_>> = tokenize(&encoded, WidthMode::Wc, false).collect();
            assert_eq!(
                inside,
                vec![Token::Text {
                    text: &encoded[..],
                    width: 0
                }],
                "U+{:04X} was not read as a character",
                0x80 + u32::from(c) - 0x80
            );

            // The same inside a control string: the byte ends it only if it
            // is ST, the character never does.
            let mut raw = b"\x1b]0;".to_vec();
            raw.push(c);
            raw.extend_from_slice(b"Z\x07");
            let ends_early = matches!(
                tokenize(&raw, WidthMode::Wc, false).next(),
                Some(Token::Escape(seq)) if seq.len() < raw.len()
            );
            assert_eq!(
                ends_early,
                c == 0x9c || c == 0x1b,
                "0x{c:02x} in a payload terminated the sequence unexpectedly"
            );

            let mut encoded_in = b"\x1b]0;".to_vec();
            encoded_in.extend_from_slice(&encoded);
            encoded_in.extend_from_slice(b"Z\x07");
            let len = encoded_in.len();
            assert_eq!(
                tokenize(&encoded_in, WidthMode::Wc, false).next(),
                Some(Token::Escape(&encoded_in[..len])),
                "U+00{c:02X} in a payload ended the sequence"
            );
        }
    }
}

#[cfg(test)]
mod fast_path {
    use super::*;

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
        const PREFIXES: &[&[u8]] = &[
            b"\x1b]0;", b"\x1bP", b"\x1b_", b"\x1b^", b"\x1bX", b"\x1b", b"\x1b ", b"\x1b#",
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
                // Controls as bytes, the character as a character.
                let mut input = prefix.to_vec();
                input.extend_from_slice(c.to_string().as_bytes());
                input.extend_from_slice(b"\x07after");
                for t in tokenize(&input, WidthMode::Wc, false) {
                    let bytes = match t {
                        Token::Text { text, .. } | Token::Escape(text) => text,
                        Token::Control(_) => continue,
                    };
                    assert!(
                        std::str::from_utf8(bytes).is_ok(),
                        "{input:x?} produced a token split mid-character: {bytes:x?}"
                    );
                }
            }
        }
        // The OSC case in full: the sequence stays whole and the text after it
        // survives.
        for c in ['\u{2705}', '\u{2714}', '\u{2728}', '\u{171c}'] {
            let mut input = b"\x1b]0;Build ".to_vec();
            input.extend_from_slice(c.to_string().as_bytes());
            input.extend_from_slice(b"\x07after");
            let toks: Vec<_> = tokenize(&input, WidthMode::Wc, false).collect();
            let (escape, rest) = toks.split_first().expect("a token");
            let Token::Escape(seq) = escape else {
                panic!("expected the OSC to be one escape token, got {escape:?}")
            };
            let mut want = b"\x1b]0;Build ".to_vec();
            want.extend_from_slice(c.to_string().as_bytes());
            want.push(0x07);
            assert_eq!(*seq, &want[..], "the sequence is whole");
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
        let toks: Vec<_> = tokenize(b"\x1b\xe2\x9c\x85x", WidthMode::Wc, false).collect();
        assert_eq!(toks[0], Token::Escape(b"\x1b\xe2\x9c\x85"));
        // A `0x9C` on a character boundary is still an 8-bit ST.
        let toks: Vec<_> = tokenize(b"\x1b]0;t\x9cx", WidthMode::Wc, false).collect();
        assert!(matches!(toks[0], Token::Escape(b"\x1b]0;t\x9c")));
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

    /// Tokenize `bytes` fully and return the bytes the run scan visited.
    ///
    /// The tokens are checked rather than discarded: these inputs are the
    /// shapes most likely to desynchronise the run cache, so measuring what
    /// they cost while ignoring what they produced would miss an answer that
    /// is wrong but cheap.
    fn scan_cost(bytes: &[u8]) -> usize {
        let mut t = tokenize(bytes, WidthMode::Wc, false);
        let mut rebuilt = Vec::with_capacity(bytes.len());
        for tok in &mut t {
            match tok {
                Token::Text { text, .. } => {
                    assert!(
                        std::str::from_utf8(text).is_ok(),
                        "text token split a character: {text:x?}"
                    );
                    rebuilt.extend_from_slice(text);
                }
                Token::Escape(b) => rebuilt.extend_from_slice(b),
                Token::Control(c) => rebuilt.push(c),
            }
        }
        assert_eq!(rebuilt, bytes, "tokens did not reassemble the input");
        t.scanned
    }

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
            assert_eq!(
                scan_cost(line.as_bytes()),
                n,
                "the run is found once, in one pass"
            );
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
            let scanned = scan_cost(&bytes);
            assert!(
                scanned <= 2 * n,
                "scanning {n} malformed bytes visited {scanned} of them"
            );
        }
    }
}
