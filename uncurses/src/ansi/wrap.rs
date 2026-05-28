//! Word- and hard-wrap utilities that preserve ANSI escape sequences.

use super::text::{Token, WidthMode, tokenize};

#[inline]
fn bs(b: &[u8]) -> &str {
    // SAFETY: token slices from `&str` input fall on valid UTF-8 boundaries.
    unsafe { std::str::from_utf8_unchecked(b) }
}

/// Default word-break runes used by [`wordwrap`].
pub const DEFAULT_BREAKPOINTS: &str = "-,.;: ";

/// Hard-wrap `s` so no visible line exceeds `limit` columns. Breaks happen at
/// exact column boundaries, in the middle of words if necessary. ANSI escapes
/// are preserved verbatim.
///
/// If `preserve_space` is `false`, whitespace at break boundaries is collapsed
/// in the same way wordwrap collapses it.
pub fn hardwrap(s: &str, limit: usize, preserve_space: bool) -> String {
    hardwrap_mode(s, limit, preserve_space, WidthMode::default(), false)
}

/// Like [`hardwrap`] but with [`WidthMode`].
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

/// Word-wrap `s` at `breakpoints` so no visible line exceeds `limit` columns.
/// Long words that exceed `limit` are not broken (use [`wrap`] for that).
/// ANSI escapes are preserved verbatim.
pub fn wordwrap(s: &str, limit: usize, breakpoints: &str) -> String {
    wordwrap_mode(s, limit, breakpoints, WidthMode::default(), false)
}

/// Like [`wordwrap`] but with [`WidthMode`].
pub fn wordwrap_mode(
    s: &str,
    limit: usize,
    breakpoints: &str,
    mode: WidthMode,
    eaw_wide: bool,
) -> String {
    if limit == 0 {
        return s.to_string();
    }
    let bp: Vec<char> = breakpoints.chars().collect();

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

    let flush_word_to_line = |line: &mut String,
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
                let is_break = bs(text).chars().all(|c| bp.contains(&c));
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
        // Trailing space attaches to line.
        line.push_str(&space);
    }
    out.push_str(&line);
    out
}

/// Soft-wrap: try word-wrap at `breakpoints`, but hard-wrap any single word that
/// is longer than `limit`. Combines [`wordwrap`] with a hardwrap fallback.
pub fn wrap(s: &str, limit: usize, breakpoints: &str) -> String {
    wrap_mode(s, limit, breakpoints, WidthMode::default(), false)
}

/// Like [`wrap`] but with [`WidthMode`].
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
    // First wordwrap, then hardwrap each resulting line in case any word is
    // longer than `limit`.
    let wrapped = wordwrap_mode(s, limit, breakpoints, mode, eaw_wide);
    let mut out = String::with_capacity(wrapped.len());
    for (i, line) in wrapped.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&hardwrap_mode(line, limit, false, mode, eaw_wide));
    }
    out
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

    #[test]
    fn wordwrap_zero_limit_returns_input() {
        assert_eq!(wordwrap("anything", 0, " "), "anything");
    }
}
