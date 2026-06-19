//! String painting and width-mode accessors for [`Screen`].

use std::io::Write;

use crate::layout::Rect;
use crate::style::Style;
use crate::text::{Painter, WidthMode, WrapMode};

use super::Screen;

impl<W: Write> Screen<W> {
    /// Paint `s` into this screen starting at `pos`. See
    /// [`Painter::set_str`] for full semantics — inline SGR (`CSI … m`)
    /// updates style mid-stream and inline OSC 8 sequences attach a
    /// hyperlink to subsequent cells. Returns the position immediately
    /// after the last painted cell.
    ///
    /// ```ignore
    /// screen.set_str((0, 0), "hi", WrapMode::Truncate);
    /// ```
    pub fn set_str(
        &mut self,
        pos: impl Into<crate::layout::Position>,
        s: &str,
        wrap: WrapMode,
    ) -> crate::layout::Position {
        self.painter().set_str(pos, s, wrap)
    }

    /// Like [`Self::set_str`] but starts with the given `style` instead
    /// of [`Style::default()`]. Inline SGR/OSC 8 sequences in `s` still
    /// mutate the painter's state as they're encountered.
    pub fn set_str_with(
        &mut self,
        pos: impl Into<crate::layout::Position>,
        s: &str,
        wrap: WrapMode,
        style: Style,
    ) -> crate::layout::Position {
        self.painter().set_str_with(pos, s, wrap, style)
    }

    /// Paint `s` clipped to `rect` (in the screen's own coordinate
    /// space). Painting starts at `rect`'s top-left corner; `\n` resets
    /// `x` to `rect.left()` and advances `y`. See [`Painter::set_str_rect`].
    pub fn set_str_rect(
        &mut self,
        rect: impl Into<Rect>,
        s: &str,
        wrap: WrapMode,
    ) -> crate::layout::Position {
        self.painter().set_str_rect(rect, s, wrap)
    }

    /// Like [`Self::set_str_rect`] but starts with the given `style`
    /// instead of [`Style::default()`].
    pub fn set_str_rect_with(
        &mut self,
        rect: impl Into<Rect>,
        s: &str,
        wrap: WrapMode,
        style: Style,
    ) -> crate::layout::Position {
        self.painter().set_str_rect_with(rect, s, wrap, style)
    }

    /// Construct a [`Painter`] that writes into this screen, wired up
    /// with the screen's current [width mode](Self::width_mode) and
    /// [East-Asian Ambiguous policy](Self::eaw_wide).
    pub fn painter(&mut self) -> Painter<'_, Self> {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide);
        Painter::new(self, mode, eaw)
    }

    /// The width-calculation mode the screen currently uses. Derived
    /// from the terminal's grapheme-cluster mode (DEC 2027): `Grapheme`
    /// when enabled, `Wc` otherwise.
    pub fn width_mode(&self) -> WidthMode {
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
    /// construction with [`Screen::with_eaw_wide`].
    pub fn eaw_wide(&self) -> bool {
        self.eaw_wide
    }

    /// Whether Unicode core / grapheme-cluster mode (DEC 2027) is on.
    pub fn grapheme_clusters(&self) -> bool {
        self.state.grapheme_clusters
    }

    /// Display width, in columns, of `s` under the screen's current
    /// [width mode](Self::width_mode) and
    /// [East-Asian Ambiguous policy](Self::eaw_wide).
    ///
    /// Inline ANSI escapes (SGR `CSI … m`, OSC 8 hyperlinks) contribute
    /// no width, matching how [`Self::set_str`] paints. The result
    /// saturates at `u16::MAX`.
    pub fn str_width(&self, s: &str) -> u16 {
        crate::ansi::string_width(s.as_bytes(), self.width_mode(), self.eaw_wide)
            .min(u16::MAX as usize) as u16
    }

    /// Display width, in columns, of one extended grapheme cluster `g`
    /// under the screen's current [width mode](Self::width_mode) and
    /// [East-Asian Ambiguous policy](Self::eaw_wide).
    ///
    /// In [`WidthMode::Wc`](crate::text::WidthMode::Wc) this is the width
    /// of `g`'s first code point; in
    /// [`WidthMode::Grapheme`](crate::text::WidthMode::Grapheme) it is the
    /// full cluster width. See [`crate::text::grapheme_width`].
    pub fn grapheme_width(&self, g: &str) -> u8 {
        self.width_mode().grapheme_width(g, self.eaw_wide)
    }

    /// Iterate `s` as `(cluster, width)` pairs under the screen's current
    /// [width mode](Self::width_mode) and
    /// [East-Asian Ambiguous policy](Self::eaw_wide).
    ///
    /// Always segments by extended grapheme cluster; only the per-cluster
    /// width follows the mode. See [`crate::text::grapheme_cells`].
    pub fn grapheme_cells<'a>(
        &self,
        s: &'a str,
    ) -> impl Iterator<Item = (&'a str, u8)> + use<'a, W> {
        crate::text::grapheme_cells(s, self.width_mode(), self.eaw_wide)
    }
}
