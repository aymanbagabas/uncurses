//! String painting and width-mode accessors for [`Canvas`].

use std::io::Write;

use crate::text::{TextSurface, WidthMode};

use super::Canvas;

impl<W: Write> TextSurface for Canvas<W> {
    /// The width-calculation mode the screen currently uses. Derived
    /// from the terminal's grapheme-cluster mode (DEC 2027): `Grapheme`
    /// when enabled, `Wc` otherwise.
    fn width_mode(&self) -> WidthMode {
        if self.state.grapheme_clusters {
            WidthMode::Grapheme
        } else {
            WidthMode::Wc
        }
    }

    /// East-Asian Ambiguous policy: when `true`, code points whose
    /// East-Asian-Width property is `Ambiguous` are measured as 2
    /// cells instead of 1. Terminals configured for CJK locales
    /// typically want `true`. See [`crate::text::char_width`]. Set at
    /// construction with [`Canvas::with_eaw_wide`].
    fn eaw_wide(&self) -> bool {
        self.eaw_wide
    }
}

impl<W: Write> Canvas<W> {
    /// Whether Unicode core / grapheme-cluster mode (DEC 2027) is on.
    pub fn grapheme_clusters(&self) -> bool {
        self.state.grapheme_clusters
    }

    /// Display width, in columns, of one extended grapheme cluster `g`
    /// under the screen's current width mode and East-Asian Ambiguous
    /// policy.
    ///
    /// In [`WidthMode::Wc`](crate::text::WidthMode::Wc) this is the width
    /// of `g`'s first code point; in
    /// [`WidthMode::Grapheme`](crate::text::WidthMode::Grapheme) it is the
    /// full cluster width. See [`crate::text::grapheme_width`].
    pub fn grapheme_width(&self, g: &str) -> u8 {
        TextSurface::width_mode(self).grapheme_width(g, self.eaw_wide)
    }

    /// Iterate `s` as `(cluster, width)` pairs under the screen's current
    /// width mode and East-Asian Ambiguous policy.
    ///
    /// Always segments by extended grapheme cluster; only the per-cluster
    /// width follows the mode. See [`crate::text::grapheme_cells`].
    pub fn grapheme_cells<'a>(
        &self,
        s: &'a str,
    ) -> impl Iterator<Item = (&'a str, u8)> + use<'a, W> {
        crate::text::grapheme_cells(s, TextSurface::width_mode(self), self.eaw_wide)
    }
}
