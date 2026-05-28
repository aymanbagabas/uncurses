//! Width-measurement policy: [`WidthMode`] + the `(cluster, width)`
//! iterator [`grapheme_cells`].

use crate::cell::graphemes;

use super::width::{char_width, grapheme_width};

/// How strings are split into cells and how each cell's width is
/// measured.
///
/// This enum covers only the segmentation strategy. The East-Asian
/// Ambiguous policy (see [`char_width`]) is orthogonal and is passed
/// alongside as a separate `eaw_wide: bool`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WidthMode {
    /// wcwidth-style: each cluster's width is the width of its first
    /// code point alone. Cluster-blind — VS16, ZWJ joins, and emoji
    /// presentation overrides have no effect on the result.
    #[default]
    Wc,
    /// Cluster-aware: width considers the whole grapheme cluster
    /// (variation selectors, Regional Indicators, ZWJ sequences,
    /// Extended_Pictographic presentation).
    Grapheme,
}

impl WidthMode {
    /// Display width of one extended grapheme cluster under this mode
    /// and the given East-Asian Ambiguous policy (see [`char_width`]).
    ///
    /// * `Wc` — width of `g`'s first code point via [`char_width`].
    /// * `Grapheme` — full cluster width via [`grapheme_width`].
    pub fn grapheme_width(self, g: &str, eaw_wide: bool) -> u8 {
        match self {
            Self::Wc => g.chars().next().map_or(0, |c| char_width(c, eaw_wide)),
            Self::Grapheme => grapheme_width(g, eaw_wide),
        }
    }
}

/// Iterate `s` as `(cluster, width)` pairs under `mode` and `eaw_wide`.
///
/// Always segments by UTS-29 extended grapheme cluster; only the width
/// computation differs between modes (see [`WidthMode::grapheme_width`]).
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
