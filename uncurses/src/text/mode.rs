//! Width-measurement policy and grapheme-cell iteration.
//!
//! Strings are always segmented into extended grapheme clusters by
//! [`grapheme_cells`]. [`WidthMode`] changes only how each cluster's cell width
//! is computed: by the first code point (`Wc`) or by cluster-aware Unicode
//! presentation rules (`Grapheme`).

use crate::unicode::graphemes;

use super::width::{char_width, grapheme_width};

/// How grapheme clusters are measured for terminal-cell layout.
///
/// This enum does not control segmentation: [`grapheme_cells`] always
/// segments by extended grapheme cluster. It controls only width calculation
/// for each cluster. The East-Asian Ambiguous policy is orthogonal and is
/// passed as a separate `eaw_wide` boolean to [`char_width`] and
/// [`grapheme_width`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WidthMode {
    /// Measure each cluster by the width of its first code point.
    ///
    /// This is wcwidth-style and intentionally cluster-blind: variation
    /// selectors, zero-width joiners, and emoji presentation overrides in the
    /// rest of the cluster do not change the result. Use this when matching
    /// older or simpler terminal width behavior is more important than
    /// cluster-aware emoji presentation.
    #[default]
    Wc,
    /// Measure the whole grapheme cluster.
    ///
    /// This mode accounts for variation selectors, regional indicators,
    /// zero-width-joiner sequences, and pictographic presentation when
    /// deciding whether a cluster occupies zero, one, or two cells. Use it for
    /// modern terminal rendering that treats emoji and joined clusters as a
    /// single displayed unit.
    Grapheme,
}

impl WidthMode {
    /// Measure one extended grapheme cluster under this mode.
    ///
    /// In [`WidthMode::Wc`] mode, the width is the [`char_width`] of `g`'s
    /// first code point, or `0` for an empty string. In
    /// [`WidthMode::Grapheme`] mode, the width is [`grapheme_width`] for the
    /// whole cluster.
    ///
    /// # Parameters
    ///
    /// * `g` — an extended grapheme cluster. Passing a longer string is
    ///   accepted but only the first code point is considered in `Wc` mode.
    /// * `eaw_wide` — East-Asian Ambiguous policy; see [`char_width`].
    ///
    /// # Returns
    ///
    /// The cluster width in terminal cells, normally `0`, `1`, or `2`.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
    pub fn grapheme_width(self, g: &str, eaw_wide: bool) -> u8 {
        match self {
            Self::Wc => g.chars().next().map_or(0, |c| char_width(c, eaw_wide)),
            Self::Grapheme => grapheme_width(g, eaw_wide),
        }
    }
}

/// Iterate `s` as `(grapheme_cluster, width)` pairs.
///
/// Segmentation is always by extended grapheme cluster. Width calculation uses
/// [`WidthMode::grapheme_width`] with the supplied East-Asian Ambiguous policy.
/// The string slice in each yielded pair borrows from `s`.
///
/// # Parameters
///
/// * `s` — UTF-8 string to segment and measure.
/// * `mode` — cluster-width policy.
/// * `eaw_wide` — East-Asian Ambiguous policy.
///
/// # Returns
///
/// An iterator yielding borrowed cluster slices and their display widths.
///
/// # Errors and panics
///
/// This function does not fail or intentionally panic.
pub fn grapheme_cells(
    s: &str,
    mode: WidthMode,
    eaw_wide: bool,
) -> impl Iterator<Item = (&str, u8)> {
    graphemes(s).map(move |g| (g, mode.grapheme_width(g, eaw_wide)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wc_mode_uses_first_codepoint_width() {
        // 'e' + combining acute: first char 'e' → 1.
        assert_eq!(WidthMode::Wc.grapheme_width("e\u{0301}", false), 1);
        // Lone combining mark cluster: first char width 0.
        assert_eq!(WidthMode::Wc.grapheme_width("\u{0301}", false), 0);
    }

    #[test]
    fn grapheme_mode_uses_cluster_width() {
        // ✋ + VS15: Grapheme honours VS15 → text presentation, width 1.
        assert_eq!(
            WidthMode::Grapheme.grapheme_width("\u{270b}\u{fe0e}", false),
            1
        );
        // Wc ignores the cluster and just measures '✋' alone (width 2
        // under the default emoji-presentation tables).
        assert_eq!(WidthMode::Wc.grapheme_width("\u{270b}\u{fe0e}", false), 2);
    }

    #[test]
    fn east_asian_width_flag_applies_to_both_modes() {
        assert_eq!(WidthMode::Wc.grapheme_width("…", false), 1);
        assert_eq!(WidthMode::Wc.grapheme_width("…", true), 2);
        assert_eq!(WidthMode::Grapheme.grapheme_width("…", false), 1);
        assert_eq!(WidthMode::Grapheme.grapheme_width("…", true), 2);
    }

    #[test]
    fn grapheme_cells_segments_both_modes_by_cluster() {
        let g: Vec<_> = grapheme_cells("Aé中", WidthMode::Grapheme, false).collect();
        assert_eq!(g.len(), 3);
        assert_eq!(g[2], ("中", 2));

        // Wc mode also segments by cluster; widths come from first char.
        let w: Vec<_> = grapheme_cells("e\u{0301}A", WidthMode::Wc, false).collect();
        assert_eq!(w.len(), 2);
        assert_eq!(w[0], ("e\u{0301}", 1));
        assert_eq!(w[1], ("A", 1));
    }
}
