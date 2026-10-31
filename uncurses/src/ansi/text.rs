//! ANSI-aware byte-stream tokenizer for text utilities.
//!
//! ## Category
//!
//! The tokenizer classifies an input byte slice as visible grapheme clusters,
//! complete ANSI escape/string sequences, or standalone control bytes. Width,
//! stripping, truncation, and wrapping utilities all build on this stream.
//!
//! ## Control encodings
//!
//! Both 7-bit forms (`ESC [`, `ESC ]`, `ESC P`, `ESC X`, `ESC ^`, `ESC _`) and
//! their 8-bit C1 bytes (`0x9B`, `0x9D`, `0x90`, `0x98`, `0x9E`, `0x9F`) open a
//! sequence. The tokenizer also accepts UTF-8 C1 characters (`C2 80` through
//! `C2 9F`) as controls.
//!
//! A control string ends at ST: `0x9C`, `C2 9C`, or `ESC \`.
//! **`BEL` ends an OSC and nothing else**: `OSC Ps ; Pt BEL` is an xterm
//! convention rather than a rule about control strings, and ECMA-48 gives them
//! all one terminator. A `0x07` inside a DCS, SOS, PM or APC is payload, which
//! matters because DCS carries arbitrary data. A lone `ESC` also ends a string
//! and is re-parsed as the start of the next sequence.
//!
//! ## C1 bytes depend on decoder state
//!
//! A byte in `0x80..=0x9F` is a C1 control **between** characters and a UTF-8
//! continuation byte **inside** one. Which it is depends on where the decoder
//! is and on nothing else, so:
//!
//! - the byte `0x9C` is 8-bit ST, and the byte `0x9D` opens an OSC;
//! - `C2 9C` is UTF-8 ST, and `C2 9D` opens an OSC;
//! - the `0x9C` inside `E2 9C 85` ("✅") is neither, and is never examined.
//!
//! That last case is why every code point in U+2700..U+273F survives inside an
//! OSC title. UTF-8 C1 support lets text utilities parse controls in a `&str`.
//! This is an input-text convention. Terminal output should use 7-bit controls:
//! xterm processes control bytes before UTF-8 decoding.
//!
//! ## Malformed input
//!
//! [`tokenize`] takes bytes, so its input need not be valid UTF-8. A byte that
//! begins no well-formed character is emitted as [`TokenKind::Control`] to keep
//! forward progress, and contributes no width. Tokens always concatenate back
//! to the input exactly.
//!
//! ## Mode interaction
//!
//! This module does not interpret terminal modes or sequence semantics. Escape
//! bytes are passed through as zero-width tokens so callers can preserve or drop
//! them according to their own policy.

pub use crate::text::WidthMode;
use crate::unicode::graphemes;

/// The category of a token produced by [`tokenize`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// One whole, well-formed UTF-8 grapheme cluster.
    Text,
    /// An ANSI escape sequence or control string, with zero display width.
    Escape,
    /// A standalone C0 control, DEL, C1 control, or malformed byte, with zero display width.
    Control,
}

/// A single token produced by [`tokenize`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<'a> {
    /// The token category.
    pub kind: TokenKind,
    /// The original input bytes, borrowed without modification.
    ///
    /// Tokens returned by [`tokenize`] contain a nonempty slice.
    /// Standalone UTF-8 C1 controls contain `0xC2` followed by the control code.
    pub bytes: &'a [u8],
    /// The display width in terminal cells.
    ///
    /// Escape and control tokens have width zero. Text width depends on the
    /// selected width mode and can be zero for combining marks.
    pub width: u16,
    /// The decoded byte for a control token, or zero for text and escape tokens.
    ///
    /// Raw and UTF-8 C1 encodings produce the same control code.
    /// A malformed byte retains its byte value here.
    pub control: u8,
}

/// Return the display width of `bytes` ignoring ANSI escapes.
///
/// Non-UTF-8 bytes contribute no width.
pub fn string_width(bytes: &[u8], mode: WidthMode, eaw_wide: bool) -> usize {
    tokenize(bytes, mode, eaw_wide)
        .map(|t| usize::from(t.width))
        .sum()
}

/// Tokenize an input byte slice into ANSI-aware tokens.
///
/// The tokens concatenate back to `bytes` exactly; the tokenizer reclassifies
/// bytes but never invents or drops one. `bytes` need not be valid UTF-8.
///
/// See the [module docs](self) for which byte ends a control string, and for
/// why a byte in `0x80..=0x9F` is a C1 control in one position and part of a
/// character in another.
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
        #[cfg(test)]
        validated: 0,
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
    /// Bytes handed to UTF-8 validation, counted separately from `scanned`.
    ///
    /// The scan and the validation are two different pieces of work over the
    /// same run, and only one of them is refreshed at `scan_end`. A version
    /// that finds the run once but revalidates the remainder on every token
    /// is quadratic in real time - 4x per doubling, 8 s for a 128 KB line of
    /// CJK - while visiting each byte exactly once *in the scan*. `scanned`
    /// alone cannot see that; this can.
    #[cfg(test)]
    validated: usize,
}

impl<'a> Tokenizer<'a> {
    fn control_token(&mut self, control: u8, size: usize) -> Token<'a> {
        let start = self.pos;
        self.pos += size;
        Token {
            kind: TokenKind::Control,
            bytes: &self.bytes[start..self.pos],
            width: 0,
            control,
        }
    }

    /// Set `run` to the validated text from `pos` up to `to`.
    ///
    /// The byte count lives here rather than at the call site on purpose: the
    /// cost that has to stay linear is "bytes handed to `from_utf8`", so
    /// moving the validation moves its meter with it. A version that finds the
    /// run once but revalidates the remainder per token keeps `scanned` at
    /// exactly one visit per byte while running in quadratic time.
    #[inline]
    fn validate(&mut self, to: usize) {
        let from = self.pos;
        #[cfg(test)]
        {
            self.validated += to - from;
        }
        self.run = std::str::from_utf8(&self.bytes[from..to]).unwrap_or("");
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        let b = *self.bytes.get(self.pos)?;

        // Printable ASCII followed by ASCII or end-of-input is a whole
        // cluster. Consume it before scanning or validating a text run.
        if (0x20..0x7f).contains(&b) && self.bytes.get(self.pos + 1).is_none_or(u8::is_ascii) {
            let start = self.pos;
            self.pos += 1;
            if !self.run.is_empty() {
                self.run = &self.run[1..];
            }
            return Some(Token {
                kind: TokenKind::Text,
                bytes: &self.bytes[start..self.pos],
                width: 1,
                control: 0,
            });
        }

        // Keep escape introducers ahead of the run check for dense sequences.
        let c1 = c1_at(self.bytes, self.pos);
        if b == 0x1b || c1.is_some_and(|(control, _)| is_c1_introducer(control)) {
            let start = self.pos;
            let end = scan_sequence(self.bytes, start);
            self.pos = end;
            return Some(Token {
                kind: TokenKind::Escape,
                bytes: &self.bytes[start..end],
                width: 0,
                control: 0,
            });
        }
        if let Some((control, size)) = c1 {
            return Some(self.control_token(control, size));
        }

        // A cached run contains only validated plain text. Check remaining
        // control bytes and find a new run only after that run ends.
        if self.run.is_empty() {
            // C0 controls and DEL occupy one byte.
            if b < 0x20 || b == 0x7f {
                return Some(self.control_token(b, 1));
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
                    if bb < 0x20 || bb == 0x7f || c1_at(self.bytes, end).is_some() {
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
                self.validate(end);
            }
            if self.run.is_empty() {
                // Nothing plain starts here, or what does is not valid UTF-8.
                // Either way the byte is emitted verbatim to keep forward
                // progress, exactly as an invalid leading byte always was.
                return Some(self.control_token(b, 1));
            }
        }
        let g = graphemes(self.run).next()?;
        let start = self.pos;
        self.pos += g.len();
        self.run = &self.run[g.len()..];
        let width = self.mode.grapheme_width(g, self.eaw_wide) as u16;
        Some(Token {
            kind: TokenKind::Text,
            bytes: &self.bytes[start..self.pos],
            width,
            control: 0,
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

#[inline]
fn c1_at(bytes: &[u8], at: usize) -> Option<(u8, usize)> {
    let b = bytes[at];
    if (0x80..=0x9f).contains(&b) {
        Some((b, 1))
    } else if b == 0xc2
        && let Some(&control @ 0x80..=0x9f) = bytes.get(at + 1)
    {
        Some((control, 2))
    } else {
        None
    }
}

/// Return the byte index past the end of an escape sequence starting at
/// `start`.
///
/// `bytes[start]` starts ESC or a raw or UTF-8 C1 sequence introducer
/// (`0x9B`, `0x9D`, `0x90`, `0x98`, `0x9E`, `0x9F`). The returned index points
/// to the first byte after the sequence. If the sequence is incomplete the
/// index is the end of `bytes`.
fn scan_sequence(bytes: &[u8], start: usize) -> usize {
    let len = bytes.len();
    let (head, size) = c1_at(bytes, start).unwrap_or((bytes[start], 1));

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
        0x9b => scan_csi(bytes, start + size),
        0x9d => scan_string(bytes, start + size, true),
        0x90 | 0x98 | 0x9e | 0x9f => scan_string(bytes, start + size, false),
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
/// `from` is the byte after the introducer, which for DCS is not yet the
/// payload: a DCS carries a CSI-shaped command section first, so the shape is
/// `ESC P` prefix params intermediates final, and only then the string. This
/// scan does not need to know where that boundary is, because every byte the
/// command section may hold - prefix `0x3C..=0x3F`, parameters `0x30..=0x3B`,
/// intermediates `0x20..=0x2F`, final `0x40..=0x7E` - lies inside
/// `0x20..=0x7E` and so is none of ST, BEL or ESC. Anything that has to treat
/// the payload differently from the command section, such as the doubled ESC
/// bytes in a tmux passthrough, does need to find it.
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
        if let Some((0x9c, size)) = c1_at(bytes, i) {
            return i + size;
        }
        if b == 0x07 && bel_ends {
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

/// Helpers shared by the test modules below.
#[cfg(test)]
mod util {
    use super::*;

    pub(super) fn text_token(bytes: &[u8], width: u16) -> Token<'_> {
        Token {
            kind: TokenKind::Text,
            bytes,
            width,
            control: 0,
        }
    }

    pub(super) fn escape_token(bytes: &[u8]) -> Token<'_> {
        Token {
            kind: TokenKind::Escape,
            bytes,
            width: 0,
            control: 0,
        }
    }

    pub(super) fn control_token(bytes: &[u8]) -> Token<'_> {
        Token {
            kind: TokenKind::Control,
            bytes,
            width: 0,
            control: *bytes.last().unwrap(),
        }
    }

    pub(super) fn tokens(input: &[u8]) -> Vec<Token<'_>> {
        tokenize(input, WidthMode::Grapheme, false).collect()
    }

    /// The visible text of a token stream, which is what a caller loses when
    /// a sequence runs past its terminator or a character is cut in half.
    ///
    /// Not lossy: a text token that is not whole UTF-8 is a bug, and
    /// rendering it as U+FFFD hides the failure these tests exist to catch.
    pub(super) fn visible(toks: &[Token<'_>]) -> String {
        toks.iter()
            .filter(|t| t.kind == TokenKind::Text)
            .map(|t| std::str::from_utf8(t.bytes).expect("a text token is whole UTF-8"))
            .collect()
    }

    /// Concatenate a token stream back into bytes. It must equal the input:
    /// the tokenizer may reclassify a byte but must never invent or drop one.
    pub(super) fn rebuild(toks: &[Token<'_>]) -> Vec<u8> {
        let mut out = Vec::new();
        for t in toks {
            out.extend_from_slice(t.bytes);
        }
        out
    }

    /// A control function in 7-bit, raw C1, and UTF-8 C1 forms.
    pub(super) fn forms(seven: &[u8], eight: u8) -> [(&'static str, Vec<u8>); 3] {
        [
            ("7-bit", seven.to_vec()),
            ("8-bit", vec![eight]),
            ("UTF-8", vec![0xc2, eight]),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::util::*;
    use super::*;

    #[test]
    fn token_fields_preserve_bytes_widths_and_control_codes() {
        let input = b"A\x1b[31m\t\x85\xc2\x85\xff\xe4\xb8\xad";
        let expected = [
            text_token(b"A", 1),
            escape_token(b"\x1b[31m"),
            control_token(b"\t"),
            control_token(b"\x85"),
            control_token(b"\xc2\x85"),
            control_token(b"\xff"),
            text_token(b"\xe4\xb8\xad", 2),
        ];
        for mode in [WidthMode::Wc, WidthMode::Grapheme] {
            let toks: Vec<_> = tokenize(input, mode, false).collect();
            assert_eq!(toks, expected);
            let mut offset = 0;
            for token in &toks {
                assert_eq!(token.bytes.as_ptr(), input[offset..].as_ptr());
                offset += token.bytes.len();
            }
            assert_eq!(offset, input.len());
            assert_eq!(rebuild(&toks), input);
        }
    }

    #[test]
    fn tokenize_plain_text() {
        let toks: Vec<_> = tokenize(b"abc", WidthMode::Grapheme, false).collect();
        assert_eq!(toks.len(), 3);
        assert_eq!(toks[0], text_token(b"a", 1));
    }

    #[test]
    fn tokenize_csi() {
        let toks: Vec<_> = tokenize(b"\x1b[31mhi\x1b[m", WidthMode::Grapheme, false).collect();
        assert_eq!(toks.len(), 4);
        assert_eq!(toks[0], escape_token(b"\x1b[31m"));
        assert_eq!(toks[3], escape_token(b"\x1b[m"));
    }

    #[test]
    fn utf8_c1_sequences_are_zero_width() {
        for input in [
            &b"\x9b31mhello\x9bm"[..],
            b"\xc2\x9b31mhello\xc2\x9bm",
            b"\xc2\x9d8;;https://example.com\xc2\x9chello\xc2\x9d8;;\xc2\x9c",
        ] {
            for mode in [WidthMode::Wc, WidthMode::Grapheme] {
                assert_eq!(string_width(input, mode, false), 5, "{input:x?}");
            }
        }
    }

    #[test]
    fn tokenize_osc_bel() {
        let toks: Vec<_> = tokenize(b"\x1b]0;title\x07rest", WidthMode::Grapheme, false).collect();
        assert_eq!(toks[0], escape_token(b"\x1b]0;title\x07"));
    }

    #[test]
    fn tokenize_osc_st() {
        let toks: Vec<_> =
            tokenize(b"\x1b]0;title\x1b\\rest", WidthMode::Grapheme, false).collect();
        assert_eq!(toks[0], escape_token(b"\x1b]0;title\x1b\\"));
    }

    #[test]
    fn tokenize_wide_char() {
        let toks: Vec<_> = tokenize("中".as_bytes(), WidthMode::Grapheme, false).collect();
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0], text_token("中".as_bytes(), 2));
    }

    #[test]
    fn tokenize_newline_control() {
        let toks: Vec<_> = tokenize(b"a\nb", WidthMode::Grapheme, false).collect();
        assert_eq!(toks.len(), 3);
        assert_eq!(toks[1], control_token(b"\n"));
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
        assert_eq!(toks[0], escape_token(b"\x9b31m"));
        assert_eq!(toks[3], escape_token(b"\x9bm"));
    }

    #[test]
    fn tokenize_8bit_osc_with_8bit_st() {
        // 0x9d "0;title" 0x9c "rest"
        let toks: Vec<_> = tokenize(b"\x9d0;title\x9crest", WidthMode::Grapheme, false).collect();
        assert_eq!(toks[0], escape_token(b"\x9d0;title\x9c"));
        // "rest" tokenizes to 4 single-grapheme Text tokens.
        assert_eq!(toks.len(), 5);
    }

    #[test]
    fn tokenize_8bit_dcs() {
        let toks: Vec<_> = tokenize(b"\x90q!data\x9cafter", WidthMode::Grapheme, false).collect();
        assert_eq!(toks[0], escape_token(b"\x90q!data\x9c"));
    }

    #[test]
    fn tokenize_8bit_sos_pm_apc() {
        for &intro in &[0x98u8, 0x9e, 0x9f] {
            let mut input = vec![intro];
            input.extend_from_slice(b"payload");
            input.push(0x9c);
            let toks: Vec<_> = tokenize(&input, WidthMode::Grapheme, false).collect();
            assert_eq!(toks[0], escape_token(&input));
        }
    }

    #[test]
    fn tokenize_standalone_c1_is_control() {
        // IND (0x84) standalone — not an introducer, emitted as Control.
        let toks: Vec<_> = tokenize(b"a\x84b", WidthMode::Grapheme, false).collect();
        assert_eq!(toks.len(), 3);
        assert_eq!(toks[1], control_token(b"\x84"));
    }

    #[test]
    fn tokenize_8bit_st_outside_string_is_control() {
        // Bare 0x9c with no preceding string is just a control byte.
        let toks: Vec<_> = tokenize(b"\x9c", WidthMode::Grapheme, false).collect();
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0], control_token(b"\x9c"));
    }

    #[test]
    fn tokenize_two_byte_esc() {
        // ESC = (DECKPAM)
        let toks: Vec<_> = tokenize(b"a\x1b=b", WidthMode::Grapheme, false).collect();
        assert_eq!(toks.len(), 3);
        assert_eq!(toks[1], escape_token(b"\x1b="));
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
    use super::util::*;
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
        ("UTF-8 ST", b"\xc2\x9c"),
    ];

    /// Payloads with and without UTF-8. The non-ASCII one is chosen for the
    /// bytes it contains, not for how it reads: `é` is `C3 A9`, `✅` is
    /// `E2 9C 85` and carries a `0x9C`, `一` is `E4 B8 80`, and `😀` is a
    /// four-byte character.
    const PAYLOADS: &[(&str, &str)] = &[
        ("ascii", "0;plain title"),
        ("utf8", "0;caf\u{e9} \u{2705} \u{4e00} \u{1f600}"),
    ];

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
                    for (form, intro) in forms(seven, *eight) {
                        let mut input = intro;
                        input.extend_from_slice(payload.as_bytes());
                        input.extend_from_slice(term);
                        let seq_len = input.len();
                        input.extend_from_slice("after \u{2705}".as_bytes());

                        let what = format!("{name} {form} {pname} payload, {tname} terminator");
                        let toks = tokens(&input);
                        assert_eq!(
                            toks.first(),
                            Some(&escape_token(&input[..seq_len])),
                            "{what}: the sequence is not one token"
                        );
                        assert_eq!(
                            visible(&toks[1..]),
                            "after \u{2705}",
                            "{what}: text after was lost"
                        );
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
            for (form, intro) in forms(seven, *eight) {
                for (pname, payload) in PAYLOADS {
                    let mut input = intro.clone();
                    input.extend_from_slice(payload.as_bytes());
                    let toks = tokens(&input);
                    assert_eq!(
                        toks,
                        vec![escape_token(&input[..])],
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
            for (form, intro) in forms(seven, *eight) {
                let mut input = intro.clone();
                input.extend_from_slice("pay\u{2705}".as_bytes());
                let cut = input.len();
                input.extend_from_slice(b"\x1b[31mZ");

                let toks = tokens(&input);
                assert_eq!(
                    toks[0],
                    escape_token(&input[..cut]),
                    "{name} {form}: the string should stop at the ESC"
                );
                assert_eq!(
                    toks[1],
                    escape_token(b"\x1b[31m"),
                    "{name} {form}: the CSI after it should survive whole"
                );
                assert_eq!(toks[2], text_token(b"Z", 1));
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
            assert_eq!(toks[0], escape_token(first), "{input:x?}");
            assert_eq!(toks[1], escape_token(second), "{input:x?}");
            assert_eq!(toks[2], text_token(b"Z", 1), "{input:x?}");
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
        for (what, input, opening, want) in cases {
            let toks = tokens(input);
            assert_eq!(toks.first(), Some(&escape_token(opening)), "{what}");
            assert_eq!(visible(&toks), *want, "{what}");
            // Nothing is invented or dropped anywhere in the stream.
            assert_eq!(
                rebuild(&toks),
                *input,
                "{what}: the stream did not reassemble"
            );
        }
    }

    #[test]
    fn c1_encodings_preserve_sequence_boundaries() {
        for input in [
            &b"\x1b]0;title\x9cZ"[..],
            b"\x1b]0;title\xc2\x9cZ",
            b"\x9d0;title\x07Z",
            b"\xc2\x9d0;title\x07Z",
            b"\x1b]0;\xe2\x9c\x85\x07Z",
        ] {
            let toks = tokens(input);
            assert_eq!(toks[0], escape_token(&input[..input.len() - 1]));
            assert_eq!(visible(&toks), "Z");
            assert_eq!(rebuild(&toks), input);
        }
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
            for (form, intro) in forms(seven, *eight) {
                let mut input = intro;
                input.extend_from_slice(b"pay\x07load");
                let bel_at = input.len() - b"load".len();
                input.extend_from_slice(b"\x9cZ");

                let toks = tokens(&input);
                let what = format!("{name} {form}");
                if *name == "OSC" {
                    assert_eq!(
                        toks[0],
                        escape_token(&input[..bel_at]),
                        "{what}: BEL should have ended it"
                    );
                    // What followed the BEL is now visible text.
                    assert!(
                        toks.iter()
                            .any(|t| t.kind == TokenKind::Text && t.bytes == b"l"),
                        "{what}: text after the BEL went missing"
                    );
                } else {
                    assert_eq!(
                        toks[0],
                        escape_token(&input[..input.len() - 1]),
                        "{what}: BEL is payload, so it should have run to the ST"
                    );
                    assert_eq!(toks[1], text_token(b"Z", 1), "{what}");
                    assert_eq!(string_width(&input, WidthMode::Grapheme, false), 1);
                }
            }
        }
    }

    #[test]
    fn every_c1_control_preserves_its_encoding() {
        for c in 0x80u8..=0x9f {
            for input in [vec![c], vec![0xc2, c]] {
                for mode in [WidthMode::Wc, WidthMode::Grapheme] {
                    let toks: Vec<_> = tokenize(&input, mode, false).collect();
                    let expected = if is_c1_introducer(c) {
                        escape_token(&input)
                    } else {
                        control_token(&input)
                    };
                    assert_eq!(toks, [expected]);
                    assert_eq!(rebuild(&toks), input);
                    assert_eq!(string_width(&input, mode, false), 0);

                    let payload = [b"\x1b]0;", input.as_slice(), b"Z\x07"].concat();
                    let expected_len = if c == 0x9c {
                        4 + input.len()
                    } else {
                        payload.len()
                    };
                    assert_eq!(
                        tokenize(&payload, mode, false).next(),
                        Some(escape_token(&payload[..expected_len])),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod utf8_c1 {
    use super::{util::*, *};
    use crate::ansi::{strip::strip, truncate, wrap};

    #[test]
    fn text_utilities_preserve_standalone_control_encoding() {
        let input = std::str::from_utf8(b"ab\xc2\x80cdef").unwrap();
        assert_eq!(strip(input), input);
        assert_eq!(
            wrap::hardwrap(input, 3, true).as_bytes(),
            b"ab\xc2\x80c\ndef"
        );
        for result in [
            wrap::wordwrap(
                std::str::from_utf8(b"ab\xc2\x80 cd").unwrap(),
                2,
                wrap::DEFAULT_BREAKPOINTS,
            ),
            wrap::wrap(
                std::str::from_utf8(b"ab\xc2\x80 cd").unwrap(),
                2,
                wrap::DEFAULT_BREAKPOINTS,
            ),
        ] {
            assert_eq!(result.as_bytes(), b"ab\xc2\x80\ncd");
        }
        assert_eq!(truncate::truncate(input, 4, "").as_bytes(), b"ab\xc2\x80cd");
        assert_eq!(
            truncate::truncate_left(input, 4, "").as_bytes(),
            b"\xc2\x80cdef"
        );
        assert_eq!(truncate::cut(input, 1, 1).as_bytes(), b"b\xc2\x80cde");
    }

    #[test]
    fn text_utilities_preserve_encoded_sequences() {
        let input = std::str::from_utf8(b"\xc2\x9b31mabcdef\xc2\x9bm").unwrap();
        assert_eq!(strip(input), "abcdef");
        assert_eq!(
            wrap::hardwrap(input, 3, true).as_bytes(),
            b"\xc2\x9b31mabc\ndef\xc2\x9bm"
        );
        assert_eq!(
            truncate::truncate(input, 4, "").as_bytes(),
            b"\xc2\x9b31mabcd\xc2\x9bm"
        );
        assert_eq!(
            truncate::truncate_left(input, 4, "").as_bytes(),
            b"\xc2\x9b31mcdef\xc2\x9bm"
        );
        assert_eq!(
            truncate::cut(input, 1, 1).as_bytes(),
            b"\xc2\x9b31mbcde\xc2\x9bm"
        );
    }

    #[test]
    fn other_two_byte_characters_remain_text() {
        for lead in 0xc2..=0xdf {
            for tail in 0x80..=0xbf {
                if lead == 0xc2 && tail <= 0x9f {
                    continue;
                }
                let input = [lead, tail];
                let toks = tokens(&input);
                assert_eq!(toks.len(), 1);
                assert_eq!(toks[0].kind, TokenKind::Text);
                assert_eq!(toks[0].bytes, input);
                assert_eq!(rebuild(&toks), input);
            }
        }
    }

    #[test]
    fn malformed_leads_preserve_encoded_controls_and_progress() {
        for input in [
            &b"\xc2\xc2\x9b31mZ"[..],
            b"\xe0\xc2\x9d0;title\xc2\x9cZ",
            b"\xc2\x1b]0;title\xc2\x9cZ",
        ] {
            let toks = tokens(input);
            assert_eq!(visible(&toks), "Z");
            assert_eq!(rebuild(&toks), input);
        }
    }
}

#[cfg(test)]
mod fast_path {
    use super::util::*;
    use super::*;

    #[test]
    fn cached_runs_stop_before_controls_escapes_and_invalid_bytes() {
        let before = "ab中e\u{301}".as_bytes();
        let after = "xy界".as_bytes();
        for boundary in [
            &b"\n"[..],
            b"\x7f",
            b"\x9c",
            b"\xc2\x9c",
            b"\xff",
            b"\x1b[31m",
            b"\x9b31m",
            b"\xc2\x9b31m",
            b"\xc2\x9d0;title\xc2\x9c",
            b"\x1b]0;title\x07",
            b"\x1bPpayload\x1b\\",
        ] {
            let input = [before, boundary, after].concat();
            for mode in [WidthMode::Wc, WidthMode::Grapheme] {
                for eaw_wide in [false, true] {
                    let expected: Vec<_> = [before, boundary, after]
                        .into_iter()
                        .flat_map(|part| tokenize(part, mode, eaw_wide))
                        .collect();
                    let mut iter = tokenize(&input, mode, eaw_wide);
                    assert_eq!(iter.by_ref().collect::<Vec<_>>(), expected);
                    assert_eq!(iter.next(), None);
                }
            }
        }
    }

    fn text_tokens_eaw(s: &str, mode: WidthMode, eaw_wide: bool) -> Vec<(String, u16)> {
        tokenize(s.as_bytes(), mode, eaw_wide)
            .filter(|t| t.kind == TokenKind::Text)
            .map(|t| {
                // Not lossy: a token that is not valid UTF-8 is a bug, and
                // rendering it as U+FFFD hides exactly the failure these
                // tests exist to catch.
                (
                    std::str::from_utf8(t.bytes)
                        .expect("a text token is whole UTF-8")
                        .to_owned(),
                    t.width,
                )
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

    /// The fast path emits text only for printable ASCII.
    ///
    /// Its `0x20..0x7f` range excludes C0 controls and DEL, even though the
    /// shortcut runs before control handling. This test checks every
    /// seven-bit byte: printable ASCII has width one, controls stay controls,
    /// and ESC opens an escape sequence.
    #[test]
    fn controls_never_reach_the_ascii_shortcut() {
        for b in 0u8..=0x7f {
            let input = [b];
            let toks: Vec<_> = tokenize(&input, WidthMode::Wc, false).collect();
            match toks.as_slice() {
                [
                    Token {
                        kind: TokenKind::Text,
                        bytes,
                        width,
                        control,
                    },
                ] => {
                    assert!(
                        (0x20..=0x7e).contains(&b),
                        "0x{b:02x} was emitted as text, but only printable ASCII may be"
                    );
                    assert_eq!(*bytes, &input[..]);
                    assert_eq!(*width, 1);
                    assert_eq!(*control, 0);
                }
                [token] if token.kind == TokenKind::Control => {
                    assert_eq!(*token, control_token(&input));
                    assert!(
                        b < 0x20 || b == 0x7f,
                        "0x{b:02x} is printable but was emitted as a control"
                    );
                }
                // A bare ESC opens a sequence with nothing in it.
                [token] if token.kind == TokenKind::Escape => {
                    assert_eq!(*token, escape_token(b"\x1b"));
                }
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
                    if t.kind == TokenKind::Control {
                        continue;
                    }
                    let bytes = t.bytes;
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
            assert_eq!(escape.kind, TokenKind::Escape);
            let mut want = b"\x1b]0;Build ".to_vec();
            want.extend_from_slice(c.to_string().as_bytes());
            want.push(0x07);
            assert_eq!(*escape, escape_token(&want), "the sequence is whole");
            assert_eq!(visible(rest), "after");
        }
        // A bare ESC takes the whole character after it, not its lead byte.
        let toks: Vec<_> = tokenize(b"\x1b\xe2\x9c\x85x", WidthMode::Wc, false).collect();
        assert_eq!(toks[0], escape_token(b"\x1b\xe2\x9c\x85"));
        // A `0x9C` on a character boundary is still an 8-bit ST.
        let toks: Vec<_> = tokenize(b"\x1b]0;t\x9cx", WidthMode::Wc, false).collect();
        assert_eq!(toks[0], escape_token(b"\x1b]0;t\x9c"));
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
                Some(&escape_token(escape)),
                "input {input:x?} did not stop at its terminator"
            );
            assert!(
                !visible(&toks).is_empty(),
                "input {input:x?} lost all its text"
            );
        }
        // A bare ESC must not consume the CSI that follows a malformed byte.
        let toks: Vec<_> = tokenize(b"\x1b\xe0\x1b[31mZ", WidthMode::Wc, false).collect();
        assert_eq!(toks[0], escape_token(b"\x1b\xe0"));
        assert_eq!(toks[1], escape_token(b"\x1b[31m"));
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
                "C[e0] T[20]w1 C[0d] T[ef, b8, 8f]w0",
            ),
            (
                b"\xf0\x30\x0d\x5b\xe2\x9c\x85\x30",
                "C[f0] T[30]w1 C[0d] T[5b]w1 T[e2, 9c, 85]w2 T[30]w1",
            ),
            (
                b"\xe0\x41\x1b\x41\xc3\xa9",
                "C[e0] T[41]w1 E[1b, 41] T[c3, a9]w1",
            ),
            (b"\xcf\xc8\x9c", "C[cf] T[c8, 9c]w1"),
            (
                b"\xe3\x61\x0a\xc4\x81\xc4\x81\x78",
                "C[e3] T[61]w1 C[0a] T[c4, 81]w1 T[c4, 81]w1 T[78]w1",
            ),
        ];
        for (seed, expected) in pinned {
            let rendered: Vec<String> = tokenize(seed, WidthMode::Grapheme, false)
                .map(|t| match t.kind {
                    TokenKind::Text => format!("T{:x?}w{}", t.bytes, t.width),
                    TokenKind::Escape => format!("E{:x?}", t.bytes),
                    TokenKind::Control => format!("C{:02x?}", t.bytes),
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
                assert!(!t.bytes.is_empty());
                match t.kind {
                    TokenKind::Text => assert_eq!(t.control, 0),
                    TokenKind::Escape => {
                        assert_eq!(t.width, 0);
                        assert_eq!(t.control, 0);
                    }
                    TokenKind::Control => {
                        assert_eq!(t.width, 0);
                        assert_eq!(t.control, *t.bytes.last().unwrap());
                        if t.bytes.len() == 2 {
                            assert_eq!(t.bytes[0], 0xc2);
                            assert!((0x80..=0x9f).contains(&t.control));
                        } else {
                            assert_eq!(t.bytes.len(), 1);
                        }
                    }
                }
                if t.kind == TokenKind::Text {
                    assert!(
                        std::str::from_utf8(t.bytes).is_ok(),
                        "{input:x?} produced a split character {:x?}",
                        t.bytes
                    );
                }
            }
            string_width(input, WidthMode::Grapheme, false);
        }
    }
}

#[cfg(test)]
mod scaling {
    use super::util::*;
    use super::*;

    /// Tokenize `bytes` fully and return the work it cost: bytes visited by
    /// the run scan, and bytes handed to UTF-8 validation.
    ///
    /// Both are needed. They are two passes over the same run refreshed by two
    /// different conditions, so either can go quadratic while the other stays
    /// linear - a tokenizer that scans once per run but revalidates the
    /// remainder once per token takes 4x as long for each doubling of the
    /// input and still reports exactly one scan visit per byte.
    ///
    /// The tokens are checked rather than discarded: these inputs are the
    /// shapes most likely to desynchronise the run cache, so measuring what
    /// they cost while ignoring what they produced would miss an answer that
    /// is wrong but cheap.
    fn scan_cost(bytes: &[u8]) -> (usize, usize) {
        let mut t = tokenize(bytes, WidthMode::Wc, false);
        let toks: Vec<_> = (&mut t).collect();
        // Measuring what a wrong answer costs is no use, so check it. The
        // discarded `visible` is the check: it panics on a text token that is
        // not whole UTF-8.
        let _ = visible(&toks);
        assert_eq!(rebuild(&toks), bytes, "tokens did not reassemble the input");
        (t.scanned, t.validated)
    }

    #[test]
    fn utf8_c1_boundaries_keep_scans_linear() {
        for unit in [
            &b"\xe7\x95\x8c\xc2\x9b31m\xe7\x95\x8c\xc2\x9bm"[..],
            b"\xe7\x95\x8c\xc2\x80\xe7\x95\x8c",
            b"\xe7\x95\x8c\xc2\x9d0;title\xc2\x9c",
        ] {
            for count in [1_000, 4_000, 16_000] {
                let input = unit.repeat(count);
                let (scanned, validated) = scan_cost(&input);
                assert!(scanned <= input.len());
                assert!(validated <= input.len());
            }
        }
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
                (0, 0),
                "printable ASCII needs neither a run scan nor UTF-8 validation"
            );
        }
    }

    /// The same bound for text that is not ASCII.
    ///
    /// The line above is all `x`, so every token leaves through the ASCII
    /// shortcut and the grapheme path is never entered. A run of non-ASCII
    /// characters is the one that walks `run` a cluster at a time, and it is
    /// the one the run cache exists for: `latin1`, wide, astral, and a base
    /// plus a combining mark, which is the case where a cluster spans more
    /// than one character.
    #[test]
    fn tokenizing_a_long_non_ascii_line_is_linear_too() {
        for unit in ["\u{e9}", "\u{4e00}", "\u{1f600}", "e\u{301}"] {
            for n in [1_000usize, 4_000, 16_000, 64_000] {
                let line = unit.repeat(n);
                let len = line.len();
                assert_eq!(
                    scan_cost(line.as_bytes()),
                    (len, len),
                    "{unit:?} x{n}: the run is found once and validated once"
                );
            }
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
            let (scanned, validated) = scan_cost(&bytes);
            assert!(
                scanned <= 2 * n,
                "scanning {n} malformed bytes visited {scanned} of them"
            );
            assert!(
                validated <= 2 * n,
                "validating {n} malformed bytes submitted {validated} of them"
            );
        }
    }
}
