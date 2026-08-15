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
