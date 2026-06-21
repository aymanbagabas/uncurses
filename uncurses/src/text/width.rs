//! Code-point and grapheme-cluster display width helpers.
//!
//! The public functions in this module return terminal-cell widths. The
//! implementation is selected at compile time: the default feature uses compact
//! code-point width data with conservative property helpers, while the `icu`
//! feature uses property data with broader Unicode coverage.

/// Display width, in terminal cells, of a single code point.
///
/// This function is cluster-blind. It reports width for `c` alone: `0` for
/// controls and zero-width formatting/mark code points, `2` for code points
/// whose East-Asian-Width property is `Wide` or `Fullwidth`, and `1` for most
/// other printable code points.
///
/// `eaw_wide` selects the East-Asian Ambiguous policy: when `true`, ambiguous
/// code points, including many Greek, Cyrillic, and box-drawing characters, are
/// reported as two cells; when `false`, they are reported as one.
///
/// # Parameters
///
/// * `c` — Unicode scalar value to measure.
/// * `eaw_wide` — whether East-Asian Ambiguous code points count as wide.
///
/// # Returns
///
/// The display width in terminal cells.
///
/// # Errors and panics
///
/// This function does not fail or intentionally panic.
pub fn char_width(c: char, eaw_wide: bool) -> u8 {
    cp_width(c, eaw_wide)
}

/// Display width, in terminal cells, of one extended grapheme cluster.
///
/// This function is cluster-aware for the cases that affect terminal layout:
/// it honors text/emoji variation selectors, regional indicators, zero-width
/// joiner sequences, and pictographic default presentation. Non-pictographic
/// clusters use the width of their base code point; combining marks,
/// joiners, and variation selectors in the tail do not add cells.
///
/// # Parameters
///
/// * `g` — one extended grapheme cluster. An empty string has width `0`.
/// * `eaw_wide` — East-Asian Ambiguous policy; see [`char_width`].
///
/// # Returns
///
/// The cluster width in terminal cells.
///
/// # Errors and panics
///
/// This function does not fail or intentionally panic.
pub fn grapheme_width(g: &str, eaw_wide: bool) -> u8 {
    let mut chars = g.chars();
    let Some(first) = chars.next() else {
        return 0;
    };
    let cp = first as u32;

    // ASCII printable fast path.
    if (0x20..0x7f).contains(&cp) {
        return 1;
    }
    // C0 / DEL / C1 controls.
    if cp < 0x20 || (0x7f..=0x9f).contains(&cp) {
        return 0;
    }
    // Flag emoji: a Regional Indicator forms a width-2 cluster (RI+RI
    // or even a lone RI — the latter is what most terminals render).
    if is_regional_indicator(cp) {
        return 2;
    }
    // Default-ignorable lone code points render as zero cells.
    if is_default_ignorable(first) {
        return 0;
    }
    // Extended_Pictographic: honour explicit VS overrides, otherwise
    // fall back to the code point's default Emoji_Presentation.
    if is_extended_pictographic(first) {
        if g.contains('\u{fe0f}') {
            return 2;
        }
        if g.contains('\u{fe0e}') {
            return 1;
        }
        return if is_emoji_presentation(first) { 2 } else { 1 };
    }
    // Non-pictographic base: use the per-codepoint width of the base
    // character. Combining marks / ZWJs / VSes in the tail are 0-width
    // and don't change the cluster's cell count.
    cp_width(first, eaw_wide)
}

// ---------------------------------------------------------------------------
// Code-point width — backend-specific
// ---------------------------------------------------------------------------

#[cfg(all(not(feature = "icu"), feature = "unicode-rs"))]
fn cp_width(c: char, eaw_wide: bool) -> u8 {
    use unicode_width::UnicodeWidthChar;
    let raw = if eaw_wide {
        UnicodeWidthChar::width_cjk(c)
    } else {
        UnicodeWidthChar::width(c)
    };
    raw.unwrap_or(0).min(2) as u8
}

#[cfg(feature = "icu")]
fn cp_width(c: char, eaw_wide: bool) -> u8 {
    use icu_properties::CodePointMapData;
    use icu_properties::props::{EastAsianWidth, GeneralCategory};

    let cp = c as u32;
    if cp < 0x20 || (0x7f..=0x9f).contains(&cp) {
        return 0;
    }
    if is_default_ignorable(c) {
        return 0;
    }
    let gc = CodePointMapData::<GeneralCategory>::new().get(c);
    if matches!(
        gc,
        GeneralCategory::NonspacingMark | GeneralCategory::EnclosingMark | GeneralCategory::Format
    ) {
        return 0;
    }
    match CodePointMapData::<EastAsianWidth>::new().get(c) {
        EastAsianWidth::Wide | EastAsianWidth::Fullwidth => 2,
        EastAsianWidth::Ambiguous if eaw_wide => 2,
        _ => 1,
    }
}

// ---------------------------------------------------------------------------
// Property helpers
// ---------------------------------------------------------------------------

#[inline]
fn is_regional_indicator(cp: u32) -> bool {
    (0x1f1e6..=0x1f1ff).contains(&cp)
}

#[cfg(feature = "icu")]
fn is_default_ignorable(c: char) -> bool {
    use icu_properties::CodePointSetData;
    use icu_properties::props::DefaultIgnorableCodePoint;
    CodePointSetData::new::<DefaultIgnorableCodePoint>().contains(c)
}

#[cfg(all(not(feature = "icu"), feature = "unicode-rs"))]
fn is_default_ignorable(c: char) -> bool {
    // Conservative subset that matters for terminal rendering: format
    // controls and the variation-selector blocks. Full coverage would
    // need a property table; callers wanting strict UAX behaviour
    // should enable `--features icu`.
    matches!(
        c as u32,
        0x00ad           // SOFT HYPHEN
        | 0x034f         // COMBINING GRAPHEME JOINER
        | 0x061c         // ARABIC LETTER MARK
        | 0x115f..=0x1160// HANGUL CHOSEONG/JUNGSEONG FILLER
        | 0x17b4..=0x17b5
        | 0x180b..=0x180f
        | 0x200b..=0x200f// ZWSP..RLM
        | 0x202a..=0x202e
        | 0x2060..=0x206f
        | 0x3164
        | 0xfe00..=0xfe0f// VS1..VS16
        | 0xfeff
        | 0xffa0
        | 0xfff0..=0xfff8
        | 0x1bca0..=0x1bca3
        | 0x1d173..=0x1d17a
        | 0xe0000..=0xe0fff
    )
}

#[cfg(feature = "icu")]
fn is_extended_pictographic(c: char) -> bool {
    use icu_properties::CodePointSetData;
    use icu_properties::props::ExtendedPictographic;
    CodePointSetData::new::<ExtendedPictographic>().contains(c)
}

#[cfg(all(not(feature = "icu"), feature = "unicode-rs"))]
fn is_extended_pictographic(c: char) -> bool {
    // Best-effort coverage of the main Extended_Pictographic ranges
    // (UTS #51 emoji-data). For UAX-strict behaviour use `--features icu`.
    let cp = c as u32;
    matches!(
        cp,
        0x00a9 | 0x00ae
        | 0x203c | 0x2049
        | 0x2122 | 0x2139
        | 0x2194..=0x2199
        | 0x21a9..=0x21aa
        | 0x231a..=0x231b
        | 0x2328
        | 0x2388
        | 0x23cf
        | 0x23e9..=0x23f3
        | 0x23f8..=0x23fa
        | 0x24c2
        | 0x25aa..=0x25ab
        | 0x25b6
        | 0x25c0
        | 0x25fb..=0x25fe
        | 0x2600..=0x27bf
        | 0x2934..=0x2935
        | 0x2b00..=0x2bff
        | 0x3030
        | 0x303d
        | 0x3297
        | 0x3299
        | 0x1f000..=0x1ffff
    )
}

#[cfg(feature = "icu")]
fn is_emoji_presentation(c: char) -> bool {
    use icu_properties::CodePointSetData;
    use icu_properties::props::EmojiPresentation;
    CodePointSetData::new::<EmojiPresentation>().contains(c)
}

#[cfg(all(not(feature = "icu"), feature = "unicode-rs"))]
fn is_emoji_presentation(c: char) -> bool {
    // Best-effort: code points whose default presentation is emoji
    // (UTS #51 Emoji_Presentation = Yes). Coarse subset; for the full
    // property use `--features icu`.
    let cp = c as u32;
    matches!(
        cp,
        0x231a..=0x231b
        | 0x23e9..=0x23ec
        | 0x23f0
        | 0x23f3
        | 0x25fd..=0x25fe
        | 0x2614..=0x2615
        | 0x2648..=0x2653
        | 0x267f
        | 0x2693
        | 0x26a1
        | 0x26aa..=0x26ab
        | 0x26bd..=0x26be
        | 0x26c4..=0x26c5
        | 0x26ce
        | 0x26d4
        | 0x26ea
        | 0x26f2..=0x26f3
        | 0x26f5
        | 0x26fa
        | 0x26fd
        | 0x2705
        | 0x270a..=0x270b
        | 0x2728
        | 0x274c
        | 0x274e
        | 0x2753..=0x2755
        | 0x2757
        | 0x2795..=0x2797
        | 0x27b0
        | 0x27bf
        | 0x2b1b..=0x2b1c
        | 0x2b50
        | 0x2b55
        | 0x1f300..=0x1ffff
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- char_width -----

    #[test]
    fn ascii_is_one() {
        for c in [' ', 'a', 'Z', '0', '~'] {
            assert_eq!(char_width(c, false), 1);
            assert_eq!(char_width(c, true), 1);
        }
    }

    #[test]
    fn controls_are_zero() {
        for c in ['\t', '\n', '\r', '\0', '\x1b', '\x7f'] {
            assert_eq!(char_width(c, false), 0);
        }
    }

    #[test]
    fn cjk_wide_is_two() {
        assert_eq!(char_width('中', false), 2);
        assert_eq!(char_width('中', true), 2);
        assert_eq!(char_width('日', false), 2);
    }

    #[test]
    fn east_asian_ambiguous_flag_flips_width() {
        // U+2026 HORIZONTAL ELLIPSIS — EAW=Ambiguous.
        assert_eq!(char_width('…', false), 1);
        assert_eq!(char_width('…', true), 2);
        // Box drawing (Ambiguous in EAW).
        assert_eq!(char_width('─', false), 1);
        assert_eq!(char_width('─', true), 2);
    }

    // ----- grapheme_width -----

    #[test]
    fn grapheme_ascii_is_one() {
        assert_eq!(grapheme_width("A", false), 1);
    }

    #[test]
    fn grapheme_cjk_is_two() {
        assert_eq!(grapheme_width("中", false), 2);
    }

    #[test]
    fn flag_emoji_is_two() {
        // 🇺🇸 — Regional Indicator pair.
        assert_eq!(grapheme_width("\u{1f1fa}\u{1f1f8}", false), 2);
    }

    #[test]
    fn vs16_promotes_text_glyph_to_emoji() {
        // ✋ + VS16 → emoji presentation, width 2.
        assert_eq!(grapheme_width("\u{270b}\u{fe0f}", false), 2);
    }

    #[test]
    fn vs15_demotes_emoji_to_text() {
        // ✋ + VS15 → text presentation, width 1.
        assert_eq!(grapheme_width("\u{270b}\u{fe0e}", false), 1);
    }

    #[test]
    fn grapheme_ambiguous_respects_eaw() {
        assert_eq!(grapheme_width("…", false), 1);
        assert_eq!(grapheme_width("…", true), 2);
        assert_eq!(grapheme_width("─", true), 2);
    }

    #[test]
    fn combining_mark_does_not_change_base_width() {
        // e + COMBINING ACUTE ACCENT (é) — one cell.
        assert_eq!(grapheme_width("e\u{0301}", false), 1);
    }
}
