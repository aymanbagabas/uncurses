//! ANSI and terminal-control sequence subsystem.
//!
//! ## Scope
//!
//! The modules under `ansi` are the byte-level building blocks used to emit,
//! parse, measure, strip, truncate, and wrap terminal control streams. They cover
//! cursor motion, screen editing, modes, SGR styling, OSC metadata, DCS/APC
//! payloads, C0/C1 controls, and ANSI-aware text utilities.
//!
//! ## Sequence families
//!
//! Most writers emit 7-bit forms because they are broadly accepted on byte
//! streams that are otherwise UTF-8 text:
//!
//! ```text
//! CSI: ESC [ params intermediates final      e.g. ESC [ ? 2048 h
//! OSC: ESC ] command ; payload BEL|ST        e.g. ESC ] 2 ; title ESC \\
//! DCS: ESC P params payload ST               e.g. ESC P + q 524742 ESC \\
//! APC: ESC _ command payload ST              e.g. ESC _ G ... ESC \\
//! ```
//!
//! Anatomy of a DEC private mode sequence:
//!
//! ```text
//! ESC [  ?  2 0 4 8  h        CSI ? 2048 h  (enable mode 2048)
//! ──┬── ─┬─ ───┬──── ┬
//!  CSI  priv  params final
//! ```
//!
//! ## 7-bit and 8-bit controls
//!
//! The constants in [`c0`] and [`c1`] name single-byte controls. Parser utilities
//! recognize both the 7-bit `ESC` spellings and the 8-bit C1 bytes, while writer
//! functions generally choose explicit 7-bit byte strings.
//!
//! ## Mode interaction
//!
//! Mode-aware features are represented by [`mode::Mode`]. Enable or disable
//! modes with [`mode::write_set_mode`] and [`mode::write_reset_mode`] before
//! expecting mode-controlled reports such as bracketed paste, focus events,
//! in-band resize, or light/dark notifications.
//!
//! ## Example
//!
//! ```rust,ignore
//! use uncurses::ansi::title::write_window_title;
//!
//! let mut out = Vec::new();
//! write_window_title(&mut out, "my app")?; // ESC ] 2 ; my app ESC \\
//! # Ok::<(), std::io::Error>(())
//! ```

pub mod ascii;
pub mod c0;
pub mod c1;
pub mod charset;
pub mod clipboard;
pub mod color;
pub mod cost;
pub mod ctrl;
pub mod cursor;
pub mod cwd;
pub mod finalterm;
pub mod focus;
pub mod graphics;
pub mod hyperlink;
pub mod inband;
pub mod iterm2;
pub mod keypad;
pub mod kitty;
pub mod mode;
pub mod notification;
pub mod palette;
pub mod params;
pub mod passthrough;
pub mod paste;
pub mod progress;
pub mod screen;
pub mod sgr;
pub mod status;
pub mod strip;
pub mod termcap;
pub mod text;
pub mod title;
pub mod truncate;
pub mod urxvt;
pub mod winop;
pub mod wrap;
pub mod xterm;

#[cfg(uncurses_bench)]
mod bench;

/// The text utilities all reach `from_utf8_unchecked` through their own `bs`,
/// on the tokenizer's promise that no token ever splits a character.
///
/// That promise is checked at the tokenizer, and the `debug_assert!`s in
/// `wrap::bs`, `truncate::bs`, `strip::bs` and `text::painter` exist to check
/// it again where it is relied on - but nothing drove them. Every escape
/// sequence in these modules' tests has an ASCII payload, so reverting the
/// scanner fix (stepping one byte instead of one character) left all of them
/// green while `strip` handed ill-formed bytes to `from_utf8_unchecked`. These
/// inputs put a character that carries a C1 byte inside a sequence, which is
/// the shape that made it undefined behaviour.
#[cfg(test)]
mod utf8_boundaries {
    use super::{strip::strip, truncate, wrap};

    /// Sequences whose payload contains a character with a C1 continuation
    /// byte, followed by visible text.
    ///
    /// `\u{2705}` is `E2 9C 85` and carries 8-bit ST; `\u{9c}` is `C2 9C` and
    /// *is* that byte, encoded; `\u{9d}` is `C2 9D`, the OSC introducer
    /// encoded. Each appears in a terminated sequence, so what follows is
    /// text a caller can see.
    const INPUTS: &[&str] = &[
        "\x1b]0;\u{2705}\x07visible",
        "\x1b]0;a\u{9c}b\x07visible",
        "\x1b]0;a\u{9d}b\x07visible",
        "\x1b]8;;https://example.com/\u{2705}\x07visible\x1b]8;;\x07",
        "\x1bP1$r\u{2705}\x1b\\visible",
        "\x1b_G\u{2705}\x1b\\visible",
        "\x1b^\u{2705}\x1b\\visible",
        "\x1bX\u{2705}\x1b\\visible",
        // An intermediate-byte escape whose final byte is non-ASCII, which is
        // `scan_esc_intermediate`'s half of the same bug.
        "\x1b#\u{2705}visible",
        "\x1b\u{2705}visible",
        // A payload that is not terminated at all: the whole tail is one
        // escape token, and it still must not stop mid-character.
        "\x1b]0;caf\u{e9} \u{2705} \u{4e00} \u{1f600}",
    ];

    #[test]
    fn the_text_utilities_never_see_a_split_character() {
        for input in INPUTS {
            // The assertion is inside the callees: each of these routes every
            // token through its module's `bs`, which checks the slice is whole
            // UTF-8 before `from_utf8_unchecked` takes it on trust. A scanner
            // that stops mid-character makes these panic under
            // `debug_assertions` and makes them UB without.
            for limit in [0usize, 1, 3, 7, 100] {
                wrap::hardwrap(input, limit, false);
                wrap::hardwrap(input, limit, true);
                wrap::wordwrap(input, limit, wrap::DEFAULT_BREAKPOINTS);
                wrap::wrap(input, limit, wrap::DEFAULT_BREAKPOINTS);
                truncate::truncate(input, limit, "…");
                truncate::truncate_left(input, limit, "…");
                truncate::cut(input, limit / 2, limit);
                strip(input);
            }
        }
    }

    /// The visible text survives the sequence, which is the user-facing half
    /// of the same promise: a scanner that stops mid-character leaves the
    /// remaining bytes of that character outside the escape, where they are
    /// dropped or painted as garbage.
    #[test]
    fn the_text_after_a_utf8_payload_survives() {
        for input in &INPUTS[..INPUTS.len() - 1] {
            assert_eq!(
                strip(input),
                "visible",
                "strip lost the text after {input:?}"
            );
            assert_eq!(
                truncate::truncate(input, 7, ""),
                *input,
                "truncate at the full width should keep {input:?} intact"
            );
            assert_eq!(
                strip(&wrap::hardwrap(input, 100, false)),
                "visible",
                "hardwrap lost the text after {input:?}"
            );
        }
    }
}

/// A seeded fuzz over the text utilities' public `&str` API.
///
/// The tokenizer has its own byte-level fuzz in [`text`], but it stops at the
/// token stream. What the callers build on top of it - the wrap's decision to
/// skip a pass, the truncate's running width - is where an invariant can hold
/// for every token and still be wrong for the string, and none of it was
/// driven by anything but hand-written cases.
///
/// Deliberately a seeded xorshift in an ordinary `#[test]` rather than
/// `cargo-fuzz`: it needs no nightly, no new dependency and no separate CI
/// job, so it runs on every `cargo test` instead of whenever somebody
/// remembers. The ceiling is that it explores a fixed alphabet from a fixed
/// seed rather than mutating a corpus, so it cannot find a shape that is not
/// built from these pieces. Reach for `cargo-fuzz` if that stops being enough.
#[cfg(test)]
mod fuzz {
    use super::{
        strip::strip,
        text::{WidthMode, string_width},
        truncate, wrap,
    };

    /// Text with no escape sequence in it, so a parser has no state to carry.
    ///
    /// Words longer than any limit used here are deliberate: they are the only
    /// thing that makes the word wrap report a line over the limit, so without
    /// one the hard-wrap path is never reached.
    const PLAIN: &[&str] = &[
        "a",
        "bb",
        "hello",
        " ",
        "  ",
        "\t",
        "\n",
        "-",
        ",",
        ".",
        ";",
        ":",
        "supercalifragilistic",
        "\u{4e00}",
        "\u{4e00}\u{4e01}\u{4e02}",
        "\u{1f600}",
        "\u{1f1fa}\u{1f1f8}",
        "e\u{301}",
        "a\u{200d}b",
        "\u{2705}",
        // C1 code points as characters. In UTF-8 the lead byte is `C2`, so
        // these are text, not controls - the distinction the tokenizer exists
        // to make.
        "\u{9c}",
        "\u{9d}",
    ];

    /// Sequences that terminate and sequences that do not, with payloads that
    /// carry a C1 byte inside a character.
    const SEQUENCES: &[&str] = &[
        "\x07",
        "\x1b[31m",
        "\x1b[0m",
        "\x1b[1;2;3m",
        "\x1b]8;;https://example.com/\u{2705}\x1b\\",
        "\x1b]0;title\x07",
        "\x1bP1$r\u{2705}\x1b\\",
        "\x1b_G\u{2705}\x1b\\",
        // Unterminated: these carry parser state across a newline, which is
        // the shape that makes a line-by-line measurement of the output lie.
        "\x1b]0;unterminated",
        "\x1b_",
        "\x1b",
    ];

    fn next(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    fn build(state: &mut u64, alphabets: &[&[&str]]) -> String {
        let n = (next(state) % 12) as usize + 1;
        let mut s = String::new();
        for _ in 0..n {
            let a = alphabets[next(state) as usize % alphabets.len()];
            s.push_str(a[next(state) as usize % a.len()]);
        }
        s
    }

    /// The pre-optimization wrap: word wrap, then hard wrap *every* line.
    ///
    /// Only a valid oracle on input with no escape sequence in it. It splits
    /// the output on newlines and measures each line alone, which restarts the
    /// ANSI parser; an unterminated control string spanning a newline then
    /// reads as visible text on the lines after it, and this hard-wraps bytes
    /// that are inside a sequence and have no width at all. `wrap_mode` no
    /// longer asks the question that way - the word wrap already measured
    /// every line it emitted, with the parser state it actually had, and
    /// reports whether any went over. Without an escape there is no such
    /// state to lose and the two must agree byte for byte.
    fn unconditional_wrap(s: &str, limit: usize, mode: WidthMode, eaw_wide: bool) -> String {
        if limit == 0 {
            return s.to_string();
        }
        let wrapped = wrap::wordwrap_mode(s, limit, wrap::DEFAULT_BREAKPOINTS, mode, eaw_wide);
        let mut out = String::with_capacity(wrapped.len());
        for (i, line) in wrapped.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&wrap::hardwrap_mode(line, limit, false, mode, eaw_wide));
        }
        out
    }

    /// `wrap_mode` skips the whole hard-wrap pass when the word wrap reports
    /// that no line went over the limit. If that report is ever wrong, `wrap`
    /// returns lines wider than asked for: nothing panics, nothing is
    /// ill-formed, and the layout is silently broken. That is the failure this
    /// exists to catch.
    #[test]
    fn skipping_the_hard_wrap_matches_never_skipping_it() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..20_000 {
            let s = build(&mut state, &[PLAIN]);
            let limit = (next(&mut state) % 14) as usize;
            for mode in [WidthMode::Wc, WidthMode::Grapheme] {
                for eaw_wide in [false, true] {
                    assert_eq!(
                        wrap::wrap_mode(&s, limit, wrap::DEFAULT_BREAKPOINTS, mode, eaw_wide),
                        unconditional_wrap(&s, limit, mode, eaw_wide),
                        "wrap skipped a hard wrap it needed\n input={s:?}\n limit={limit} mode={mode:?} eaw_wide={eaw_wide}"
                    );
                }
            }
        }
    }

    /// Every text utility, over input that mixes sequences into the text.
    ///
    /// Under `debug_assertions` this also drives the assertions standing in
    /// front of each `from_utf8_unchecked` these reach, so a scanner that
    /// stops mid-character fails here rather than becoming undefined
    /// behaviour in a release build.
    #[test]
    fn the_text_utilities_hold_on_input_containing_sequences() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..20_000 {
            let s = build(&mut state, &[PLAIN, SEQUENCES]);
            let limit = (next(&mut state) % 14) as usize;

            for mode in [WidthMode::Wc, WidthMode::Grapheme] {
                for eaw_wide in [false, true] {
                    // Measured over the whole string rather than line by line,
                    // so this is the parser state the tokenizer actually had.
                    let cut = truncate::truncate_mode(&s, limit, "", mode, eaw_wide);
                    assert!(
                        string_width(cut.as_bytes(), mode, eaw_wide) <= limit,
                        "truncate exceeded its limit\n input={s:?} -> {cut:?}\n limit={limit} mode={mode:?} eaw_wide={eaw_wide}"
                    );

                    truncate::truncate_left_mode(&s, limit, "", mode, eaw_wide);
                    truncate::cut_mode(&s, limit / 2, limit, mode, eaw_wide);
                    wrap::hardwrap_mode(&s, limit, true, mode, eaw_wide);
                    wrap::wrap_mode(&s, limit, wrap::DEFAULT_BREAKPOINTS, mode, eaw_wide);
                    wrap::wordwrap_mode(&s, limit, wrap::DEFAULT_BREAKPOINTS, mode, eaw_wide);
                }
            }

            // Stripping drops every sequence, so no introducer survives it.
            let plain = strip(&s);
            assert!(
                !plain.contains('\x1b'),
                "strip left an escape behind: {s:?} -> {plain:?}"
            );
        }
    }
}
