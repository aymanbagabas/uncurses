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
        pos: impl Into<crate::Position>,
        s: &str,
        wrap: WrapMode,
    ) -> crate::Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide);
        Painter::new(self)
            .with_mode(mode)
            .with_eaw_wide(eaw)
            .set_str(pos, s, wrap)
    }

    /// Like [`Self::set_str`] but starts with the given `style` instead
    /// of [`Style::default()`]. Inline SGR/OSC 8 sequences in `s` still
    /// mutate the painter's state as they're encountered.
    pub fn set_str_with(
        &mut self,
        pos: impl Into<crate::Position>,
        s: &str,
        wrap: WrapMode,
        style: Style,
    ) -> crate::Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide);
        Painter::new(self)
            .with_mode(mode)
            .with_eaw_wide(eaw)
            .set_str_with(pos, s, wrap, style)
    }

    /// Paint `s` clipped to `rect` (in the screen's own coordinate
    /// space). Painting starts at `rect`'s top-left corner; `\n` resets
    /// `x` to `rect.left()` and advances `y`. See [`Painter::set_str_rect`].
    pub fn set_str_rect(
        &mut self,
        rect: impl Into<Rect>,
        s: &str,
        wrap: WrapMode,
    ) -> crate::Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide);
        Painter::new(self)
            .with_mode(mode)
            .with_eaw_wide(eaw)
            .set_str_rect(rect, s, wrap)
    }

    /// Like [`Self::set_str_rect`] but starts with the given `style`
    /// instead of [`Style::default()`].
    pub fn set_str_rect_with(
        &mut self,
        rect: impl Into<Rect>,
        s: &str,
        wrap: WrapMode,
        style: Style,
    ) -> crate::Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide);
        Painter::new(self)
            .with_mode(mode)
            .with_eaw_wide(eaw)
            .set_str_rect_with(rect, s, wrap, style)
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
    /// typically want `true`. See [`crate::text::char_width`].
    pub fn eaw_wide(&self) -> bool {
        self.eaw_wide
    }

    /// Replace the East-Asian Ambiguous policy (see [`Self::eaw_wide`]).
    /// Affects subsequent string writes and width measurements.
    pub fn set_eaw_wide(&mut self, eaw_wide: bool) {
        self.eaw_wide = eaw_wide;
    }

    /// Whether Unicode core / grapheme-cluster mode (DEC 2027) is on.
    pub fn grapheme_clusters(&self) -> bool {
        self.state.grapheme_clusters
    }
}
