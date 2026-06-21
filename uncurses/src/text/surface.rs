//! [`TextSurface`] — text drawing as an extension of any mutable surface.
//!
//! [`SurfaceMut`](crate::buffer::SurfaceMut) reads and writes cells; it does
//! not decide how many cells a string occupies or how inline styling changes
//! those cells. `TextSurface` adds that policy layer. Implementors provide a
//! [width mode](TextSurface::width_mode) and
//! [East-Asian Ambiguous policy](TextSurface::eaw_wide); the default methods
//! build a [`Painter`] and expose the `set_str` family on top of the existing
//! surface.
//!
//! Use this trait when writing widgets, layout helpers, or tests that should
//! accept any text-capable destination:
//!
//! ```rust,ignore
//! fn label(surface: &mut impl TextSurface, at: Position, text: &str, style: Style) {
//!     surface.set_str(at, text, style);
//! }
//! ```

use crate::buffer::SurfaceMut;
use crate::layout::{Position, Rect};
use crate::style::Style;

use super::{Painter, WidthMode, WrapMode};

/// A [`SurfaceMut`] with a text-measurement policy and string-painting helpers.
///
/// Implement this for surface types that can accept styled text. The only
/// required decisions are [`width_mode`](Self::width_mode), which controls how
/// grapheme clusters are measured, and [`eaw_wide`](Self::eaw_wide), which
/// controls East-Asian Ambiguous code points. All painting methods are default
/// methods backed by [`Painter`].
///
/// Inline SGR (`CSI … m`) and OSC 8 hyperlink sequences in the input are
/// interpreted by the painter. Other control bytes are ignored except newline
/// and carriage return, which move the paint cursor within the active clip
/// rectangle.
pub trait TextSurface: SurfaceMut {
    /// Return the width-measurement mode used when shaping strings.
    ///
    /// [`WidthMode::Wc`] uses the first code point of each grapheme cluster;
    /// [`WidthMode::Grapheme`] measures the whole cluster. The selected mode is
    /// used by [`painter`](Self::painter), the `set_str` family, and
    /// [`str_width`](Self::str_width).
    ///
    /// This method is pure policy lookup. It should not inspect or mutate the
    /// surface contents.
    fn width_mode(&self) -> WidthMode;

    /// Return the East-Asian Ambiguous width policy for this surface.
    ///
    /// When `true`, code points whose East-Asian-Width property is
    /// `Ambiguous` are measured as two cells. When `false`, they are measured
    /// as one. The flag is passed to [`char_width`](super::char_width) and
    /// [`grapheme_width`](super::grapheme_width) by all text operations.
    fn eaw_wide(&self) -> bool;

    /// Construct a [`Painter`] over this surface.
    ///
    /// The painter is configured from [`width_mode`](Self::width_mode) and
    /// [`eaw_wide`](Self::eaw_wide), starts with [`Style::default()`], and
    /// writes directly into `self`. Use it when multiple adjacent writes should
    /// share the same running style or hyperlink state.
    ///
    /// # Returns
    ///
    /// A painter borrowing this surface mutably for the painter's lifetime.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or panic.
    fn painter(&mut self) -> Painter<'_, Self> {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide());
        Painter::new(self, mode, eaw)
    }

    /// Paint `s` at `pos`, clipped to the surface bounds.
    ///
    /// `style` becomes the starting style for this call. Inline SGR sequences
    /// update that style while painting, and OSC 8 sequences attach or clear a
    /// hyperlink. Newline moves to the next row at the surface's left edge;
    /// carriage return moves to that left edge on the current row. If a
    /// non-zero-width grapheme cluster would cross the right edge, painting
    /// stops.
    ///
    /// # Parameters
    ///
    /// * `pos` — starting cell position.
    /// * `s` — UTF-8 string to paint. ANSI SGR and OSC 8 sequences are parsed.
    /// * `style` — initial style applied to cells until changed by inline SGR
    ///   or hyperlink escapes.
    ///
    /// # Returns
    ///
    /// The cursor position immediately after the last written cell, or the
    /// position where painting stopped. The returned position may be outside
    /// the surface when input reaches the bottom edge.
    ///
    /// # Errors and panics
    ///
    /// This method does not return errors and does not intentionally panic.
    ///
    /// # Usage note
    ///
    /// Use [`set_str_wrap`](Self::set_str_wrap) when text should continue on
    /// following rows instead of truncating at the right edge.
    fn set_str(&mut self, pos: impl Into<Position>, s: &str, style: Style) -> Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide());
        Painter::new(self, mode, eaw).set_str(pos, s, style)
    }

    /// Paint `s` at `pos` with an explicit right-edge behavior.
    ///
    /// This is the same operation as [`set_str`](Self::set_str), except
    /// `wrap` decides what happens when a non-zero-width grapheme cluster would
    /// cross the surface's right edge: [`WrapMode::Truncate`] stops, and
    /// [`WrapMode::Wrap`] continues at the left edge of the next row until the
    /// bottom edge is reached.
    ///
    /// # Parameters
    ///
    /// * `pos` — starting cell position.
    /// * `s` — UTF-8 string to paint.
    /// * `wrap` — wrapping policy at the right edge.
    /// * `style` — initial style for painted cells.
    ///
    /// # Returns
    ///
    /// The cursor position immediately after the last written cell, or where
    /// painting stopped.
    ///
    /// # Errors and panics
    ///
    /// This method does not return errors and does not intentionally panic.
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

    /// Paint `s` inside `rect`, clipped to the surface bounds.
    ///
    /// Painting starts at `rect`'s top-left corner and is clipped to
    /// `rect ∩ self.bounds()`. Newline and carriage return use `rect`'s left
    /// edge as the return column. If a non-zero-width grapheme cluster would
    /// cross `rect`'s right edge, painting stops.
    ///
    /// # Parameters
    ///
    /// * `rect` — clipping rectangle and starting origin.
    /// * `s` — UTF-8 string to paint.
    /// * `style` — initial style for painted cells.
    ///
    /// # Returns
    ///
    /// The cursor position immediately after the last written cell, or where
    /// painting stopped.
    ///
    /// # Errors and panics
    ///
    /// This method does not return errors and does not intentionally panic.
    fn set_str_rect(&mut self, rect: impl Into<Rect>, s: &str, style: Style) -> Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide());
        Painter::new(self, mode, eaw).set_str_rect(rect, s, style)
    }

    /// Paint `s` inside `rect` with an explicit right-edge behavior.
    ///
    /// This is the rectangular form of [`set_str_wrap`](Self::set_str_wrap).
    /// [`WrapMode::Wrap`] flows to the next row at `rect`'s left edge;
    /// [`WrapMode::Truncate`] stops at `rect`'s right edge.
    ///
    /// # Parameters
    ///
    /// * `rect` — clipping rectangle and starting origin.
    /// * `s` — UTF-8 string to paint.
    /// * `wrap` — wrapping policy at the rectangle's right edge.
    /// * `style` — initial style for painted cells.
    ///
    /// # Returns
    ///
    /// The cursor position immediately after the last written cell, or where
    /// painting stopped.
    ///
    /// # Errors and panics
    ///
    /// This method does not return errors and does not intentionally panic.
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

    /// Measure the display width of `s` in terminal columns.
    ///
    /// The measurement uses this surface's [`width_mode`](Self::width_mode)
    /// and [`eaw_wide`](Self::eaw_wide) policy. Inline ANSI escape sequences
    /// recognized by the text tokenizer, including SGR and OSC sequences,
    /// contribute no width.
    ///
    /// # Parameters
    ///
    /// * `s` — string to measure.
    ///
    /// # Returns
    ///
    /// The display width in cells, saturated at `u16::MAX`.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
    fn str_width(&self, s: &str) -> u16 {
        crate::ansi::string_width(s.as_bytes(), self.width_mode(), self.eaw_wide())
            .min(u16::MAX as usize) as u16
    }
}
