//! Width-aware truncation and cutting for ANSI-decorated strings.
//!
//! ## Category
//!
//! This module shortens strings by terminal display columns while preserving
//! escape sequences such as SGR resets and OSC hyperlinks. Escape bytes do not
//! count toward width.
//!
//! ## Width conventions
//!
//! Width is computed by [`crate::ansi::text::tokenize`] using [`WidthMode`].
//! Visible text is truncated only on token boundaries; escape sequences before,
//! inside, or after the retained text are copied so terminal state remains
//! attached to the result.
//!
//! ## Mode interaction
//!
//! Truncation does not emulate terminal modes. Mode-dependent sequence semantics
//! are preserved as bytes but not interpreted.

use super::text::{Token, WidthMode, string_width, tokenize};

#[inline]
fn bs(b: &[u8]) -> &str {
    // SAFETY: token slices from `&str` input fall on valid UTF-8 boundaries.
    unsafe { std::str::from_utf8_unchecked(b) }
}

/// Truncate `s` to at most `length` display columns, appending `tail` if truncation occurs.
///
/// ANSI escape sequences are preserved verbatim and do not count toward the width budget. When `length == 0`, this function returns an empty string.
pub fn truncate(s: &str, length: usize, tail: &str) -> String {
    truncate_mode(s, length, tail, WidthMode::default(), false)
}

/// Width-mode variant of [`truncate`].
///
/// `mode` and `eaw_wide` control grapheme width calculation. If the input already fits, it is returned unchanged; otherwise the visible prefix is shortened enough to fit `tail`, and trailing escape sequences are still copied.
pub fn truncate_mode(
    s: &str,
    length: usize,
    tail: &str,
    mode: WidthMode,
    eaw_wide: bool,
) -> String {
    if length == 0 {
        return String::new();
    }
    if string_width(s.as_bytes(), mode, eaw_wide) <= length {
        return s.to_string();
    }
    let tail_w = string_width(tail.as_bytes(), mode, eaw_wide);
    let budget = length.saturating_sub(tail_w);

    let mut out = String::new();
    let mut used = 0usize;
    let mut tail_inserted = false;
    for tok in tokenize(s.as_bytes(), mode, eaw_wide) {
        match tok {
            Token::Escape(esc) => out.push_str(bs(esc)),
            Token::Control(b) => {
                if !tail_inserted {
                    out.push(b as char);
                }
            }
            Token::Text { text, width } => {
                let w = width as usize;
                if used + w > budget {
                    if !tail_inserted {
                        out.push_str(tail);
                        tail_inserted = true;
                    }
                    // Continue scanning to capture trailing escapes (e.g. SGR reset).
                    continue;
                }
                out.push_str(bs(text));
                used += w;
            }
        }
    }
    if !tail_inserted {
        out.push_str(tail);
    }
    out
}

/// Truncate `s` from the left until at most `length` display columns remain.
///
/// If truncation occurs, `prefix` is prepended after any leading escape sequences needed to preserve active terminal state.
pub fn truncate_left(s: &str, length: usize, prefix: &str) -> String {
    truncate_left_mode(s, length, prefix, WidthMode::default(), false)
}

/// Width-mode variant of [`truncate_left`].
///
/// `mode` and `eaw_wide` control grapheme width calculation. Escape sequences before the cut are retained before `prefix` so styling can carry into the visible suffix.
pub fn truncate_left_mode(
    s: &str,
    length: usize,
    prefix: &str,
    mode: WidthMode,
    eaw_wide: bool,
) -> String {
    let total = string_width(s.as_bytes(), mode, eaw_wide);
    if total <= length {
        return s.to_string();
    }
    let prefix_w = string_width(prefix.as_bytes(), mode, eaw_wide);
    let drop = total.saturating_sub(length.saturating_sub(prefix_w));

    let mut head_escapes = String::new();
    let mut out = String::new();
    let mut dropped = 0usize;
    let mut dropping = true;
    for tok in tokenize(s.as_bytes(), mode, eaw_wide) {
        match tok {
            Token::Escape(esc) => {
                if dropping {
                    head_escapes.push_str(bs(esc));
                } else {
                    out.push_str(bs(esc));
                }
            }
            Token::Control(b) => {
                if !dropping {
                    out.push(b as char);
                }
            }
            Token::Text { text, width } => {
                let w = width as usize;
                if dropping {
                    dropped += w;
                    if dropped >= drop {
                        dropping = false;
                    }
                } else {
                    out.push_str(bs(text));
                }
            }
        }
    }
    let mut result = String::new();
    // Preserve escapes encountered before the cut so style carries over.
    result.push_str(&head_escapes);
    result.push_str(prefix);
    result.push_str(&out);
    result
}

/// Remove `left` display columns from the start and `right` display columns from the end of `s`.
///
/// ANSI escape sequences are preserved and do not count toward either cut amount.
pub fn cut(s: &str, left: usize, right: usize) -> String {
    cut_mode(s, left, right, WidthMode::default(), false)
}

/// Width-mode variant of [`cut`].
///
/// `mode` and `eaw_wide` control grapheme width calculation. If the requested cuts leave no visible columns, the result is empty; otherwise trailing escape sequences are retained.
pub fn cut_mode(s: &str, left: usize, right: usize, mode: WidthMode, eaw_wide: bool) -> String {
    if left == 0 && right == 0 {
        return s.to_string();
    }
    let total = string_width(s.as_bytes(), mode, eaw_wide);
    if left >= total {
        return String::new();
    }
    let keep_end = total.saturating_sub(right);
    if keep_end <= left {
        return String::new();
    }
    let target = keep_end - left;

    let mut out = String::new();
    let mut col = 0usize;
    let mut emitted = 0usize;
    for tok in tokenize(s.as_bytes(), mode, eaw_wide) {
        match tok {
            Token::Escape(esc) => out.push_str(bs(esc)),
            Token::Control(b) => {
                if col >= left && emitted < target {
                    out.push(b as char);
                }
            }
            Token::Text { text, width } => {
                let w = width as usize;
                if col >= left && emitted + w <= target {
                    out.push_str(bs(text));
                    emitted += w;
                }
                col += w;
                if emitted >= target {
                    // Continue scanning so trailing escapes still attach.
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_basic() {
        assert_eq!(truncate("hello world", 5, ""), "hello");
    }

    #[test]
    fn truncate_with_tail() {
        assert_eq!(truncate("hello world", 8, "..."), "hello...");
    }

    #[test]
    fn truncate_no_op() {
        assert_eq!(truncate("hi", 5, "..."), "hi");
    }

    #[test]
    fn truncate_preserves_ansi() {
        let s = "\x1b[31mhello world\x1b[m";
        let got = truncate(s, 5, "");
        assert_eq!(got, "\x1b[31mhello\x1b[m");
    }

    #[test]
    fn truncate_wide_chars() {
        assert_eq!(truncate("中文测试", 4, ""), "中文");
        // Width-3 budget with 2-wide chars: only first fits.
        assert_eq!(truncate("中文", 3, ""), "中");
    }

    #[test]
    fn truncate_zero_length() {
        assert_eq!(truncate("hello", 0, ""), "");
        assert_eq!(truncate("hello", 0, "..."), "");
    }

    #[test]
    fn truncate_left_basic() {
        assert_eq!(truncate_left("hello world", 5, ""), "world");
    }

    #[test]
    fn truncate_left_with_prefix() {
        assert_eq!(truncate_left("hello world", 8, "..."), "...world");
    }

    #[test]
    fn cut_basic() {
        assert_eq!(cut("hello world", 2, 2), "llo wor");
    }

    #[test]
    fn cut_zero_zero_is_noop() {
        assert_eq!(cut("hello", 0, 0), "hello");
    }

    #[test]
    fn cut_too_wide() {
        assert_eq!(cut("hi", 10, 0), "");
    }

    #[test]
    fn truncate_preserves_osc_link() {
        let s = "\x1b]8;;https://example.com\x1b\\link text\x1b]8;;\x1b\\";
        let got = truncate(s, 4, "");
        assert_eq!(got, "\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\");
    }
}
