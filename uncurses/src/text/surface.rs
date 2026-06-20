//! [`TextSurface`] — string painting on top of any
//! [`SurfaceMut`](crate::buffer::SurfaceMut).
//!
//! A [`SurfaceMut`] knows how to read and write cells, but not how to
//! shape text. `TextSurface` adds that: an implementor supplies its
//! [width mode](TextSurface::width_mode) and
//! [East-Asian Ambiguous policy](TextSurface::eaw_wide), and the trait
//! provides the [`set_str`](TextSurface::set_str) family plus a
//! [`painter`](TextSurface::painter) constructor, all built on
//! [`Painter`]. This lets drawing helpers be written generically over
//! `&mut impl TextSurface` rather than a concrete surface type.

use crate::buffer::SurfaceMut;
use crate::layout::{Position, Rect};
use crate::style::Style;

use super::{Painter, WidthMode, WrapMode};

/// A [`SurfaceMut`] that can paint styled strings.
///
/// Implementors provide [`width_mode`](Self::width_mode) and
/// [`eaw_wide`](Self::eaw_wide); the painting methods are supplied by
/// default. Each [`set_str`](Self::set_str) call interprets inline SGR
/// (`CSI … m`) and OSC 8 hyperlinks in the input — see [`Painter`].
pub trait TextSurface: SurfaceMut {
    /// The width-measurement mode used when shaping text.
    fn width_mode(&self) -> WidthMode;

    /// East-Asian Ambiguous policy: when `true`, code points whose
    /// East-Asian-Width property is `Ambiguous` are measured as 2 cells
    /// instead of 1.
    fn eaw_wide(&self) -> bool;

    /// Construct a [`Painter`] over this surface, wired with its
    /// [width mode](Self::width_mode) and
    /// [East-Asian Ambiguous policy](Self::eaw_wide).
    fn painter(&mut self) -> Painter<'_, Self> {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide());
        Painter::new(self, mode, eaw)
    }

    /// Paint `s` at `pos` with `style` as the starting style, truncating
    /// at the right edge. Returns the position after the last painted
    /// cell. See [`Painter::set_str`].
    fn set_str(&mut self, pos: impl Into<Position>, s: &str, style: Style) -> Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide());
        Painter::new(self, mode, eaw).set_str(pos, s, style)
    }

    /// Like [`set_str`](Self::set_str) but with an explicit [`WrapMode`].
    fn set_str_wrap(
        &mut self,
        pos: impl Into<Position>,
        s: &str,
        wrap: WrapMode,
        style: Style,
    ) -> Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide());
        Painter::new(self, mode, eaw).set_str_wrap(pos, s, wrap, style)
    }

    /// Paint `s` clipped to `rect` with `style` as the starting style,
    /// truncating at `rect`'s right edge. See [`Painter::set_str_rect`].
    fn set_str_rect(&mut self, rect: impl Into<Rect>, s: &str, style: Style) -> Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide());
        Painter::new(self, mode, eaw).set_str_rect(rect, s, style)
    }

    /// Like [`set_str_rect`](Self::set_str_rect) but with an explicit
    /// [`WrapMode`].
    fn set_str_rect_wrap(
        &mut self,
        rect: impl Into<Rect>,
        s: &str,
        wrap: WrapMode,
        style: Style,
    ) -> Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide());
        Painter::new(self, mode, eaw).set_str_rect_wrap(rect, s, wrap, style)
    }

    /// Display width, in columns, of `s` under this surface's
    /// [width mode](Self::width_mode) and
    /// [East-Asian Ambiguous policy](Self::eaw_wide). Inline ANSI escapes
    /// contribute no width. Saturates at `u16::MAX`.
    fn str_width(&self, s: &str) -> u16 {
        crate::ansi::string_width(s.as_bytes(), self.width_mode(), self.eaw_wide())
            .min(u16::MAX as usize) as u16
    }
}
