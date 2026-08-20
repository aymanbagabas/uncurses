//! Width-aware wrapping for ANSI-decorated strings.
//!
//! ## Category
//!
//! Hard wrap, word wrap, and combined wrap utilities insert newlines based on
//! terminal display columns while preserving ANSI escape sequences verbatim.
//!
//! ## Width conventions
//!
//! Visible text width comes from [`crate::ansi::text::tokenize`] and [`WidthMode`].
//! Escape tokens are zero-width and stay attached to the current word, separator,
//! or line segment so styling and hyperlinks survive wrapping.
//!
//! ## Mode interaction
//!
//! Wrapping does not interpret terminal modes. It treats mode-setting and
//! mode-dependent sequences as bytes to preserve, not as state transitions.
//!
//! Sequence boundaries and widths come from [`crate::ansi::text`];
//! which byte ends a control string, and when a byte in `0x80..=0x9F`
//! is a C1 control rather than part of a character, are documented there.

use super::text::{Token, WidthMode, string_width, tokenize};

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

/// Default break characters for [`wordwrap`] and [`wrap`]: hyphen, comma, period, semicolon, colon, and space (`"-,.;: "`).
pub const DEFAULT_BREAKPOINTS: &str = "-,.;: ";

/// Hard-wrap `s` so no visible line exceeds `limit` columns.
///
/// Breaks occur at exact width boundaries, including inside words. ANSI escapes are copied verbatim and contribute zero width. If `preserve_space` is `false`, a space that lands at a break boundary is dropped.
pub fn hardwrap(s: &str, limit: usize, preserve_space: bool) -> String {
    hardwrap_mode(s, limit, preserve_space, WidthMode::default(), false)
}

/// Width-mode variant of [`hardwrap`].
///
/// `mode` and `eaw_wide` control grapheme width calculation. `limit == 0` returns the input unchanged.
pub fn hardwrap_mode(
    s: &str,
    limit: usize,
    preserve_space: bool,
    mode: WidthMode,
    eaw_wide: bool,
) -> String {
    if limit == 0 {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut col = 0usize;
    for tok in tokenize(s.as_bytes(), mode, eaw_wide) {
        match tok {
            Token::Escape(e) => out.push_str(bs(e)),
            Token::Control(b'\n') => {
                out.push('\n');
                col = 0;
            }
            Token::Control(b) => out.push(b as char),
            Token::Text { text, width } => {
                let w = width as usize;
                if w == 0 {
                    out.push_str(bs(text));
                    continue;
                }
                if col + w > limit {
                    out.push('\n');
                    col = 0;
                    if !preserve_space && text == b" " {
                        continue;
                    }
                }
                out.push_str(bs(text));
                col += w;
            }
        }
    }
    out
}

/// Word-wrap `s` at `breakpoints` so visible lines fit within `limit` where possible.
///
/// Long words are not split; use [`wrap`] when oversized words should be hard-wrapped. ANSI escapes are preserved and do not contribute width.
pub fn wordwrap(s: &str, limit: usize, breakpoints: &str) -> String {
    wordwrap_mode(s, limit, breakpoints, WidthMode::default(), false)
}

/// Width-mode variant of [`wordwrap`].
///
/// `breakpoints` is a set of characters where a line may break. `mode` and `eaw_wide` control grapheme width calculation; `limit == 0` returns the input unchanged.
pub fn wordwrap_mode(
    s: &str,
    limit: usize,
    breakpoints: &str,
    mode: WidthMode,
    eaw_wide: bool,
) -> String {
    wordwrap_inner(s, limit, breakpoints, mode, eaw_wide).0
}

/// [`wordwrap_mode`], and whether any line it produced is still wider than
/// `limit`.
///
/// Word wrapping measures every line it emits in order to decide where to
/// break, so the answer costs nothing here and a second width pass over the
/// whole output anywhere else. It is what lets [`wrap_mode`] stop after one
/// pass on text whose words all fit, which is nearly all text.
fn wordwrap_inner(
    s: &str,
    limit: usize,
    breakpoints: &str,
    mode: WidthMode,
    eaw_wide: bool,
) -> (String, bool) {
    if limit == 0 {
        // No wrapping happened, so nothing was made to fit and nothing needs
        // to be: `hardwrap_mode` returns its input unchanged at this limit
        // too.
        return (s.to_string(), false);
    }
    // A breakpoint test that is an array index, not a scan. `is_break` runs
    // once per grapheme, and a linear search of the breakpoint list per
    // character is the kind of constant that only shows up on a megabyte.
    let mut ascii_bp = [false; 128];
    let mut wide_bp: Vec<char> = Vec::new();
    for c in breakpoints.chars() {
        match u32::from(c) {
            n if n < 128 => ascii_bp[n as usize] = true,
            _ => wide_bp.push(c),
        }
    }
    // The ASCII half was an index and the non-ASCII half was still a scan,
    // which put the same constant back for anyone whose breakpoints are not
    // ASCII: wrapping n characters on n breakpoints measured an exponent of
    // 1.895. Sorting once buys a binary search and needs no new type.
    wide_bp.sort_unstable();
    let is_bp = |c: char| match u32::from(c) {
        n if n < 128 => ascii_bp[n as usize],
        _ => wide_bp.binary_search(&c).is_ok(),
    };

    // We build the output as: completed lines + current line state.
    // `line` is the bytes already committed to the current line.
    // `word` is the current pending word.
    // `space` is whitespace separator buffered between `line` and `word`.
    let mut out = String::with_capacity(s.len());
    let mut line = String::new();
    let mut line_w = 0usize;
    let mut word = String::new();
    let mut word_w = 0usize;
    let mut space = String::new();
    let mut space_w = 0usize;
    let mut over = false;

    let mut flush_word_to_line = |line: &mut String,
                                  line_w: &mut usize,
                                  word: &mut String,
                                  word_w: &mut usize,
                                  space: &mut String,
                                  space_w: &mut usize| {
        if !word.is_empty() {
            // If line+space+word exceeds limit and line is non-empty, wrap.
            // Handled by caller before calling.
            line.push_str(space);
            line.push_str(word);
            *line_w += *space_w + *word_w;
            // The only place `line_w` grows, and it is cleared the moment the
            // line is emitted, so its value here is the width that line will
            // be emitted at - checking it once here covers all five emit
            // sites. A line ends up over the limit when a single word is
            // wider than the limit, which is what hard wrapping is for.
            over |= *line_w > limit;
            space.clear();
            *space_w = 0;
            word.clear();
            *word_w = 0;
        }
    };

    for tok in tokenize(s.as_bytes(), mode, eaw_wide) {
        match tok {
            Token::Escape(e) => {
                // Attach escapes to whatever segment is currently being built.
                if !word.is_empty() {
                    word.push_str(bs(e));
                } else if !space.is_empty() {
                    space.push_str(bs(e));
                } else {
                    line.push_str(bs(e));
                }
            }
            Token::Control(b'\n') => {
                // Flush current word and emit the line.
                flush_word_to_line(
                    &mut line,
                    &mut line_w,
                    &mut word,
                    &mut word_w,
                    &mut space,
                    &mut space_w,
                );
                out.push_str(&line);
                out.push('\n');
                line.clear();
                line_w = 0;
                space.clear();
                space_w = 0;
            }
            Token::Control(b) => {
                // Treat as part of the current word.
                word.push(b as char);
            }
            Token::Text { text, width } => {
                let w = width as usize;
                let is_break = bs(text).chars().all(&is_bp);
                let is_space = text == b" " || text == b"\t";

                if is_space {
                    // Spaces become separator.
                    // First flush pending word to line.
                    if !word.is_empty() {
                        // If word doesn't fit on current line, wrap first.
                        if line_w > 0 && line_w + space_w + word_w > limit {
                            out.push_str(&line);
                            out.push('\n');
                            line.clear();
                            line_w = 0;
                            space.clear();
                            space_w = 0;
                        }
                        flush_word_to_line(
                            &mut line,
                            &mut line_w,
                            &mut word,
                            &mut word_w,
                            &mut space,
                            &mut space_w,
                        );
                    }
                    space.push_str(bs(text));
                    space_w += w;
                } else if is_break {
                    // Non-space breakpoint (e.g. '-', ','). Stay attached to word but
                    // mark that we can break after.
                    word.push_str(bs(text));
                    word_w += w;
                    // Flush after the breakpoint character.
                    if line_w > 0 && line_w + space_w + word_w > limit {
                        out.push_str(&line);
                        out.push('\n');
                        line.clear();
                        line_w = 0;
                        space.clear();
                        space_w = 0;
                    }
                    flush_word_to_line(
                        &mut line,
                        &mut line_w,
                        &mut word,
                        &mut word_w,
                        &mut space,
                        &mut space_w,
                    );
                } else {
                    word.push_str(bs(text));
                    word_w += w;
                }
            }
        }
    }

    // Final flush.
    if !word.is_empty() {
        if line_w > 0 && line_w + space_w + word_w > limit {
            out.push_str(&line);
            out.push('\n');
            line.clear();
            line_w = 0;
            space.clear();
            space_w = 0;
        }
        flush_word_to_line(
            &mut line,
            &mut line_w,
            &mut word,
            &mut word_w,
            &mut space,
            &mut space_w,
        );
    } else if !space.is_empty() {
        // Trailing space attaches to line, and can be what carries it over -
        // the one line that ends up too wide without an oversized word in it.
        line.push_str(&space);
        over |= line_w + space_w > limit;
    }
    out.push_str(&line);
    (out, over)
}

/// Soft-wrap `s` at word breakpoints, then hard-wrap any remaining overlong line.
///
/// This combines [`wordwrap`] with [`hardwrap`] so every visible line fits within `limit` when `limit > 0`.
pub fn wrap(s: &str, limit: usize, breakpoints: &str) -> String {
    wrap_mode(s, limit, breakpoints, WidthMode::default(), false)
}

/// Width-mode variant of [`wrap`].
///
/// `mode` and `eaw_wide` control grapheme width calculation. `limit == 0` returns the input unchanged.
pub fn wrap_mode(
    s: &str,
    limit: usize,
    breakpoints: &str,
    mode: WidthMode,
    eaw_wide: bool,
) -> String {
    if limit == 0 {
        return s.to_string();
    }
    // First wordwrap, then hardwrap - but only the lines that need it, and
    // only if any line does.
    //
    // Hard-wrapping exists for the one case word-wrapping cannot solve: a
    // single word longer than the limit. Every other line is already inside
    // it, and running the hard wrap over those lines is a second full pass
    // that copies them to themselves. That pass is not cheap - measured, the
    // wrap was exactly the sum of its two halves, so it cost as much as the
    // wrapping did.
    //
    // Nor is asking whether a line is too wide, if it is asked by measuring
    // the line again: the word wrap has already measured every line it
    // emitted, so it is the only pass that needs to measure at all. It
    // returns what it found. On text whose words all fit, which is nearly all
    // text, the second pass now disappears entirely - along with the width
    // pass that used to decide whether to run it.
    let (wrapped, over) = wordwrap_inner(s, limit, breakpoints, mode, eaw_wide);
    if !over {
        // No cross-check of `over` against a fresh measurement of the output
        // here. Measuring line by line restarts the parser at every newline,
        // so a control string spanning one - an unterminated APC, say - reads
        // as visible text on the lines after it, which is why the two
        // disagree on output that is correct. The word wrap carries that
        // state across newlines and is the one that can see it.
        return wrapped;
    }
    // Some line is over. Measure to find which - the output is allocated
    // lazily, at the first line that is.
    let mut out: Option<String> = None;
    let mut consumed = 0usize;
    for line in wrapped.split('\n') {
        let wide = string_width(line.as_bytes(), mode, eaw_wide) > limit;
        match (&mut out, wide) {
            // Still byte-for-byte `wrapped`; nothing to copy yet.
            (None, false) => {}
            (None, true) => {
                let mut o = String::with_capacity(wrapped.len());
                o.push_str(&wrapped[..consumed]);
                o.push_str(&hardwrap_mode(line, limit, false, mode, eaw_wide));
                out = Some(o);
            }
            (Some(o), _) => {
                o.push('\n');
                if wide {
                    o.push_str(&hardwrap_mode(line, limit, false, mode, eaw_wide));
                } else {
                    // Copied rather than re-emitted. A line that fits comes
                    // back from the hard wrap byte-identical, so this is
                    // simply not doing the work - it is not guarding against
                    // a difference. `hardwrap_mode` does re-encode a lone C1
                    // control byte as the two-byte UTF-8 for that code point,
                    // but that cannot arise from `&str` input: at a character
                    // boundary in valid UTF-8 a byte in `0x80..=0x9F` is never
                    // a lead byte, so the tokenizer never emits one as a
                    // `Control`.
                    o.push_str(line);
                }
            }
        }
        consumed += line.len() + 1;
    }
    out.unwrap_or(wrapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardwrap_basic() {
        assert_eq!(hardwrap("hello world", 5, true), "hello\n worl\nd");
    }

    #[test]
    fn hardwrap_no_preserve_space() {
        assert_eq!(hardwrap("hello world", 5, false), "hello\nworld");
    }

    #[test]
    fn hardwrap_preserves_ansi() {
        let s = "\x1b[31mhello world\x1b[m";
        let got = hardwrap(s, 5, false);
        assert_eq!(got, "\x1b[31mhello\nworld\x1b[m");
    }

    #[test]
    fn hardwrap_explicit_newline() {
        assert_eq!(hardwrap("ab\ncd", 5, true), "ab\ncd");
    }

    #[test]
    fn wordwrap_basic() {
        assert_eq!(wordwrap("hello world", 5, " "), "hello\nworld");
    }

    #[test]
    fn wordwrap_long_word_not_broken() {
        // "abcdefghij" is 10 wide, limit 5 — wordwrap leaves it intact.
        assert_eq!(wordwrap("abcdefghij", 5, " "), "abcdefghij");
    }

    #[test]
    fn wordwrap_explicit_newline() {
        assert_eq!(wordwrap("a b\nc d", 10, " "), "a b\nc d");
    }

    #[test]
    fn wordwrap_with_hyphen_break() {
        let got = wordwrap("foo-bar-baz", 4, DEFAULT_BREAKPOINTS);
        assert_eq!(got, "foo-\nbar-\nbaz");
    }

    /// Non-ASCII breakpoints are looked up in a sorted list, so they only
    /// work if that list is actually sorted.
    #[test]
    fn wordwrap_breaks_on_non_ascii_breakpoints() {
        // Deliberately unsorted, and with a decoy that is not in the input.
        assert_eq!(
            wordwrap("ab\u{3002}cd", 3, "\u{ff1b}\u{3002}\u{300c}"),
            "ab\u{3002}\ncd"
        );
        assert_eq!(
            wordwrap("ab\u{ff1b}cd", 3, "\u{ff1b}\u{3002}"),
            "ab\u{ff1b}\ncd"
        );
        // A character absent from the set must not break.
        assert_eq!(wordwrap("ab\u{3001}cd", 9, "\u{3002}"), "ab\u{3001}cd");
    }

    #[test]
    fn wrap_breaks_long_words() {
        assert_eq!(wrap("abcdefghij", 5, " "), "abcde\nfghij");
    }

    #[test]
    fn wrap_mixed() {
        assert_eq!(
            wrap("hello superlongword end", 5, " "),
            "hello\nsuper\nlongw\nord\nend"
        );
    }

    /// `wrap` hard wraps only when word wrapping tells it a line is still too
    /// wide, and word wrapping counts a line's width as it builds it. A
    /// trailing space is the one thing that lands on a line after that count
    /// is otherwise final: "hello" fits in five columns and "hello " does not,
    /// with no oversized word anywhere to notice.
    #[test]
    fn wrap_hard_wraps_a_line_carried_over_by_a_trailing_space() {
        assert_eq!(wrap("hello ", 5, " "), "hello\n");
        assert_eq!(wrap("ab cd ", 5, " "), "ab cd\n");
        // Still within the limit with the space on it, so left alone.
        assert_eq!(wrap("hi  ", 5, " "), "hi  ");
    }

    #[test]
    fn wordwrap_zero_limit_returns_input() {
        assert_eq!(wordwrap("anything", 0, " "), "anything");
    }

    /// An unterminated control string swallows everything after it, newlines
    /// included, so the text on the following lines is never visible and the
    /// input needs no wrapping at all. Measuring the output one line at a time
    /// cannot see that - the parser restarts at each newline and reads the
    /// payload as ordinary text - which is why `wrap` trusts the width the
    /// word wrap carried across the newline instead of measuring again.
    #[test]
    fn wrap_leaves_a_control_string_spanning_a_newline_alone() {
        let s = "\x1b_\nworld\u{4e00}\u{1f1fa}\x07\u{1f1fa}";
        assert_eq!(string_width(s.as_bytes(), WidthMode::default(), false), 0);
        assert_eq!(wrap(s, 8, " \t-"), s);
    }
}
