//! Tokenizer and wrapping micro-benchmarks (nightly only).
//!
//! The numbers this crate claims for text measurement are complexity claims,
//! not micro-optimisations, and the tests that guard them count work rather
//! than time it - a wall-clock assertion in the test suite is a benchmark
//! that fails when the machine is busy. These are where the timing lives
//! instead. Build and run them with:
//!
//! ```sh
//! RUSTFLAGS="--cfg uncurses_bench" cargo +nightly bench
//! ```
//!
//! Nothing compiles this module unless `--cfg uncurses_bench` is set, so it
//! has no effect on normal builds, tests, or downstream consumers. It needs
//! nightly because the `test` crate's `Bencher` is unstable.

extern crate test;

use test::{Bencher, black_box};

use crate::ansi::text::{Token, WidthMode, string_width, tokenize};
use crate::ansi::wrap::{DEFAULT_BREAKPOINTS, hardwrap, wordwrap, wrap};

/// 250 KB is a scrollback page of tool output, not a stress test.
const BIG: usize = 250_000;

fn ascii(n: usize) -> String {
    const UNIT: &str = "the quick brown fox jumps over the lazy dog ";
    std::iter::repeat_n(UNIT, n / UNIT.len()).collect()
}

/// Latin, CJK, a combining mark and an emoji - the mix that makes grapheme
/// segmentation do real work.
fn mixed(n: usize) -> String {
    let unit = "hello \u{4e00}\u{4e16} caf\u{e9} e\u{301} \u{1f600} ";
    std::iter::repeat_n(unit, n / unit.len()).collect()
}

/// One line with no break in it, which is the case hard wrapping exists for
/// and the one the run cache is about.
fn one_long_line(n: usize) -> String {
    std::iter::repeat_n('x', n).collect()
}

#[bench]
fn width_ascii_250k(b: &mut Bencher) {
    let s = ascii(BIG);
    b.iter(|| black_box(string_width(black_box(s.as_bytes()), WidthMode::Wc, false)));
}

#[bench]
fn width_mixed_250k(b: &mut Bencher) {
    let s = mixed(BIG);
    b.iter(|| black_box(string_width(black_box(s.as_bytes()), WidthMode::Wc, false)));
}

#[bench]
fn width_grapheme_mixed_250k(b: &mut Bencher) {
    let s = mixed(BIG);
    b.iter(|| {
        black_box(string_width(
            black_box(s.as_bytes()),
            WidthMode::Grapheme,
            false,
        ))
    });
}

/// The shape that was quadratic: one line, measured whole.
#[bench]
fn width_one_32k_line(b: &mut Bencher) {
    let s = one_long_line(32_000);
    b.iter(|| {
        black_box(string_width(
            black_box(s.as_bytes()),
            WidthMode::Grapheme,
            false,
        ))
    });
}

/// The shape that stayed quadratic after the first fix. `C2` opens a two-byte
/// character that `41` cannot continue, so every other byte is malformed.
/// Reachable only through the byte APIs, since a `&str` is always valid.
#[bench]
fn tokenize_32k_malformed(b: &mut Bencher) {
    let bytes: Vec<u8> = std::iter::repeat_n([0xc2u8, 0x41], 16_000)
        .flatten()
        .collect();
    b.iter(|| {
        let mut n = 0usize;
        for t in tokenize(black_box(&bytes), WidthMode::Wc, false) {
            n += match t {
                Token::Text { text, width } => text.len() + width as usize,
                Token::Escape(seq) => seq.len(),
                Token::Control(_) => 1,
            };
        }
        black_box(n)
    });
}

/// Text whose words all fit: the common path, where the second pass should
/// not happen at all.
#[bench]
fn wrap_ascii_250k(b: &mut Bencher) {
    let s = ascii(BIG);
    b.iter(|| black_box(wrap(black_box(&s), 80, DEFAULT_BREAKPOINTS)));
}

#[bench]
fn wrap_mixed_250k(b: &mut Bencher) {
    let s = mixed(BIG);
    b.iter(|| black_box(wrap(black_box(&s), 80, DEFAULT_BREAKPOINTS)));
}

/// Text where every line needs hard wrapping: the path the second pass is
/// for, and the one the double measurement made slower.
#[bench]
fn wrap_all_hard_250k(b: &mut Bencher) {
    let s: String = std::iter::repeat_n("abcdefghij", BIG / 10).collect();
    b.iter(|| black_box(wrap(black_box(&s), 80, DEFAULT_BREAKPOINTS)));
}

#[bench]
fn wrap_one_32k_word(b: &mut Bencher) {
    let s = one_long_line(32_000);
    b.iter(|| black_box(wrap(black_box(&s), 80, DEFAULT_BREAKPOINTS)));
}

#[bench]
fn wordwrap_ascii_250k(b: &mut Bencher) {
    let s = ascii(BIG);
    b.iter(|| black_box(wordwrap(black_box(&s), 80, DEFAULT_BREAKPOINTS)));
}

#[bench]
fn hardwrap_ascii_250k(b: &mut Bencher) {
    let s = ascii(BIG);
    b.iter(|| black_box(hardwrap(black_box(&s), 80, true)));
}

/// Styled output - an SGR pair around every word - which breaks the text into
/// short runs and is what a real TUI actually measures.
/// One SGR after every character, so every run of plain text is a single
/// character. This is the shape that pays the per-run cost most often, and
/// the one where carrying the run as a `&str` rather than an index costs
/// most.
#[bench]
fn width_escape_dense_250k(b: &mut Bencher) {
    let s: String = std::iter::repeat_n("a\x1b[31m", BIG / 6).collect();
    b.iter(|| black_box(string_width(black_box(s.as_bytes()), WidthMode::Wc, false)));
}

#[bench]
fn width_cjk_250k(b: &mut Bencher) {
    let s: String = std::iter::repeat_n('\u{4e00}', BIG / 3).collect();
    b.iter(|| black_box(string_width(black_box(s.as_bytes()), WidthMode::Wc, false)));
}

#[bench]
fn width_emoji_250k(b: &mut Bencher) {
    let s: String = std::iter::repeat_n('\u{1f600}', BIG / 4).collect();
    b.iter(|| {
        black_box(string_width(
            black_box(s.as_bytes()),
            WidthMode::Grapheme,
            false,
        ))
    });
}

/// An SGR pair around every word, which is what coloured log output looks
/// like: short runs, but not degenerate ones.
#[bench]
fn width_styled_250k(b: &mut Bencher) {
    let s: String = std::iter::repeat_n("\x1b[31mword\x1b[0m ", BIG / 15).collect();
    b.iter(|| black_box(string_width(black_box(s.as_bytes()), WidthMode::Wc, false)));
}
