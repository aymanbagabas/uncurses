//! [`TextSurface`] — text drawing as an extension of any mutable surface.
//!
//! [`SurfaceMut`](crate::buffer::SurfaceMut) reads and writes cells; it does
//! not decide how many cells a string occupies. `TextSurface` adds that policy
//! layer. Implementors provide a [width mode](TextSurface::width_mode) and
//! [East-Asian Ambiguous policy](TextSurface::eaw_wide); the default `set_str`
//! family segments the input into grapheme clusters and paints each with the
//! given style.
//!
//! The default methods paint text **literally**: inline SGR (`CSI … m`) and
//! OSC 8 hyperlink sequences are not interpreted, so they are segmented and
//! drawn like any other text. Reach for [`Painter`](super::Painter) — itself a
//! `TextSurface` — when a string should be parsed for inline style and
//! hyperlink escapes. Newline and carriage return still move the paint cursor
//! within the active clip rectangle.
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
use crate::cell::Style as CellStyle;
use crate::cell::{Cell, Content, Kind};
use crate::layout::{Position, Rect};

use super::{WidthMode, WrapMode, grapheme_cells};

/// A [`SurfaceMut`] with a text-measurement policy and string-painting helpers.
///
/// Implement this for surface types that can accept styled text. The only
/// required decisions are [`width_mode`](Self::width_mode), which controls how
/// grapheme clusters are measured, and [`eaw_wide`](Self::eaw_wide), which
/// controls East-Asian Ambiguous code points.
///
/// The default `set_str` family paints text literally: each grapheme cluster is
/// drawn with the given style and inline SGR / OSC 8 escapes are not
/// interpreted. Use [`Painter`](super::Painter), which is itself a
/// `TextSurface`, to parse inline style and hyperlink escapes. Newline and
/// carriage return move the paint cursor within the active clip rectangle.
pub trait TextSurface: SurfaceMut {
    /// Return the width-measurement mode used when shaping strings.
    ///
    /// [`WidthMode::Wc`] uses the first code point of each grapheme cluster;
    /// [`WidthMode::Grapheme`] measures the whole cluster. The selected mode is
    /// used by the `set_str` family and [`str_width`](Self::str_width).
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

    /// Paint `s` at `pos`, clipped to the surface bounds.
    ///
    /// Every cell painted gets `style`. This default implementation paints
    /// *literally*: escape sequences in `s` are drawn as visible characters,
    /// not interpreted. For SGR/OSC 8-aware painting that turns inline escapes
    /// into styling, wrap the surface in a [`Painter`](super::Painter). Newline
    /// moves to the next row at the surface's left edge; carriage return moves
    /// to that left edge on the current row. If a non-zero-width grapheme
    /// cluster would cross the right edge, the rest of that row is dropped and
    /// painting resumes on the next row.
    ///
    /// # Parameters
    ///
    /// * `pos` — starting cell position.
    /// * `s` — UTF-8 string to paint. Escape sequences are drawn literally.
    /// * `style` — style applied to every cell painted in this call.
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
    fn set_str(
        &mut self,
        pos: impl Into<Position>,
        s: &str,
        style: impl Into<CellStyle>,
    ) -> Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide());
        let clip = self.bounds();
        paint_literal(
            self,
            pos.into(),
            clip,
            s,
            WrapMode::Truncate,
            mode,
            eaw,
            &style.into(),
        )
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
        style: impl Into<CellStyle>,
    ) -> Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide());
        let clip = self.bounds();
        paint_literal(self, pos.into(), clip, s, wrap, mode, eaw, &style.into())
    }

    /// Paint `s` inside `rect`, clipped to the surface bounds.
    ///
    /// Painting starts at `rect`'s top-left corner and is clipped to
    /// `rect ∩ self.bounds()`. Newline and carriage return use `rect`'s left
    /// edge as the return column. If a non-zero-width grapheme cluster would
    /// cross `rect`'s right edge, the rest of that row is dropped and painting
    /// resumes on the next row.
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
    fn set_str_rect(
        &mut self,
        rect: impl Into<Rect>,
        s: &str,
        style: impl Into<CellStyle>,
    ) -> Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide());
        let rect = rect.into();
        let clip = rect.intersection(self.bounds());
        paint_literal(
            self,
            rect.position(),
            clip,
            s,
            WrapMode::Truncate,
            mode,
            eaw,
            &style.into(),
        )
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
        style: impl Into<CellStyle>,
    ) -> Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide());
        let rect = rect.into();
        let clip = rect.intersection(self.bounds());
        paint_literal(
            self,
            rect.position(),
            clip,
            s,
            wrap,
            mode,
            eaw,
            &style.into(),
        )
    }

    /// Paint `s` at `pos`, truncating with a `tail` indicator on overflow.
    ///
    /// When a cluster would cross the surface's right edge, the rest of that
    /// row is dropped and `tail` is stamped over its trailing columns, ending
    /// at the right edge. Painting resumes on the next row if `s` continues
    /// past a newline, so a multi-line `s` can stamp one tail per overflowing
    /// row. The tail appears only on rows that actually overflow. `tail` is
    /// painted with `tail_style` and may carry inline escape sequences, so it
    /// can be a single glyph (`"…"`), a word (`" more"`), or a multi-style
    /// span. A tail wider than the surface is dropped in favor of a hard
    /// truncate.
    ///
    /// # Parameters
    ///
    /// * `pos` — starting cell position.
    /// * `s` — UTF-8 string to paint.
    /// * `tail` — truncation indicator, painted when `s` overflows.
    /// * `tail_style` — starting style for the tail.
    ///
    /// # Returns
    ///
    /// The cursor position immediately after the last written cell, or where
    /// painting stopped.
    ///
    /// # Errors and panics
    ///
    /// This method does not return errors and does not intentionally panic.
    fn set_str_truncate(
        &mut self,
        pos: impl Into<Position>,
        s: &str,
        tail: &str,
        tail_style: impl Into<CellStyle>,
    ) -> Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide());
        let clip = self.bounds();
        paint_literal_truncate(
            self,
            pos.into(),
            clip,
            s,
            tail,
            &tail_style.into(),
            mode,
            eaw,
        )
    }

    /// Paint `s` inside `rect`, truncating with a `tail` indicator on overflow.
    ///
    /// This is the rectangular form of [`set_str_truncate`](Self::set_str_truncate):
    /// the clip rectangle is `rect ∩ self.bounds()`, and a tail is stamped at
    /// `rect`'s right edge on each row that overflows it.
    ///
    /// # Parameters
    ///
    /// * `rect` — clipping rectangle and starting origin.
    /// * `s` — UTF-8 string to paint.
    /// * `tail` — truncation indicator, painted when `s` overflows.
    /// * `tail_style` — starting style for the tail.
    ///
    /// # Returns
    ///
    /// The cursor position immediately after the last written cell, or where
    /// painting stopped.
    ///
    /// # Errors and panics
    ///
    /// This method does not return errors and does not intentionally panic.
    fn set_str_rect_truncate(
        &mut self,
        rect: impl Into<Rect>,
        s: &str,
        tail: &str,
        tail_style: impl Into<CellStyle>,
    ) -> Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide());
        let rect = rect.into();
        let clip = rect.intersection(self.bounds());
        paint_literal_truncate(
            self,
            rect.position(),
            clip,
            s,
            tail,
            &tail_style.into(),
            mode,
            eaw,
        )
    }

    /// Measure the display width of `s` in terminal columns.
    ///
    /// The measurement segments `s` into grapheme clusters under this
    /// surface's [`width_mode`](Self::width_mode) and
    /// [`eaw_wide`](Self::eaw_wide) policy and sums their widths. Like the
    /// default `set_str` family, this does **not** interpret inline escape
    /// sequences: an SGR or OSC 8 sequence in `s` is measured as the width of
    /// its visible bytes. Use [`Painter`](super::Painter), whose `str_width`
    /// skips recognized escapes, to measure escape-bearing text.
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
        self.grapheme_cells(s)
            .fold(0u16, |acc, (_, w)| acc.saturating_add(u16::from(w)))
    }

    /// Measure one extended grapheme cluster in cells under this surface's
    /// width mode and East-Asian Ambiguous policy.
    ///
    /// # Parameters
    ///
    /// * `g` — a single grapheme cluster.
    ///
    /// # Returns
    ///
    /// The cluster width in cells, normally `0`, `1`, or `2`.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
    fn grapheme_width(&self, g: &str) -> u8 {
        self.width_mode().grapheme_width(g, self.eaw_wide())
    }

    /// Iterate `s` as `(cluster, width)` pairs under this surface's width mode
    /// and East-Asian Ambiguous policy.
    ///
    /// # Parameters
    ///
    /// * `s` — UTF-8 string to segment and measure.
    ///
    /// # Returns
    ///
    /// An iterator yielding borrowed cluster slices and their cell widths.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
    fn grapheme_cells<'a>(&self, s: &'a str) -> impl Iterator<Item = (&'a str, u8)> {
        grapheme_cells(s, self.width_mode(), self.eaw_wide())
    }
}

/// A literal truncation tail: indicator text, its starting style, and its
/// measured cell width.
struct LiteralTail<'a> {
    text: &'a str,
    style: &'a CellStyle,
    width: u16,
}

/// Paint `s` literally, clipped to `clip`, with `wrap` behavior at the right
/// edge. Each grapheme cluster is drawn with `style`; inline escapes are not
/// interpreted (they are segmented and drawn like any other text). Newline and
/// carriage return reposition within `clip`.
#[allow(clippy::too_many_arguments)]
fn paint_literal<S: SurfaceMut + ?Sized>(
    target: &mut S,
    start: Position,
    clip: Rect,
    s: &str,
    wrap: WrapMode,
    mode: WidthMode,
    eaw_wide: bool,
    style: &CellStyle,
) -> Position {
    paint_literal_inner(target, start, clip, s, wrap, mode, eaw_wide, style, None)
}

/// Paint `s` literally with [`WrapMode::Truncate`], stamping `tail` on overflow.
///
/// The main text is drawn with [`Style::default()`]; the tail is drawn with
/// `tail_style`. The tail is dropped (hard truncate) when it is empty or wider
/// than the clip.
#[allow(clippy::too_many_arguments)]
fn paint_literal_truncate<S: SurfaceMut + ?Sized>(
    target: &mut S,
    start: Position,
    clip: Rect,
    s: &str,
    tail_text: &str,
    tail_style: &CellStyle,
    mode: WidthMode,
    eaw_wide: bool,
) -> Position {
    if clip.is_empty() {
        return start;
    }
    let tail_w = grapheme_cells(tail_text, mode, eaw_wide)
        .fold(0u16, |acc, (_, w)| acc.saturating_add(u16::from(w)));
    let tail = if tail_w == 0 || tail_w > clip.width {
        None
    } else {
        Some(LiteralTail {
            text: tail_text,
            style: tail_style,
            width: tail_w,
        })
    };
    paint_literal_inner(
        target,
        start,
        clip,
        s,
        WrapMode::Truncate,
        mode,
        eaw_wide,
        &CellStyle::default(),
        tail,
    )
}

#[allow(clippy::too_many_arguments)]
fn paint_literal_inner<S: SurfaceMut + ?Sized>(
    target: &mut S,
    start: Position,
    clip: Rect,
    s: &str,
    wrap: WrapMode,
    mode: WidthMode,
    eaw_wide: bool,
    style: &CellStyle,
    tail: Option<LiteralTail<'_>>,
) -> Position {
    if clip.is_empty() {
        return start;
    }
    // `y` only ever advances, so a start below the clip can never paint.
    if start.y >= clip.bottom() {
        return start;
    }
    let mut x = start.x;
    let mut y = start.y;
    // Truncation is per row: once a row overflows, clusters are dropped until
    // `\n` or `\r` puts the cursor back inside the clip.
    let mut truncated = false;

    for (cluster, w) in grapheme_cells(s, mode, eaw_wide) {
        // Extended grapheme segmentation joins CR LF into a single cluster, so
        // it has to be matched alongside a lone `\n` or it reads as zero-width
        // filler and never breaks the line.
        if cluster == "\n" || cluster == "\r\n" {
            y = y.saturating_add(1);
            x = clip.left();
            truncated = false;
            if y >= clip.bottom() {
                return Position::new(x, y);
            }
            continue;
        }
        if cluster == "\r" {
            x = clip.left();
            truncated = false;
            continue;
        }
        if truncated || w == 0 {
            continue;
        }
        let w = w as u16;
        if x + w > clip.right() {
            match wrap {
                WrapMode::Truncate => {
                    if let Some(t) = &tail {
                        stamp_literal_tail(target, t, clip, y, mode, eaw_wide);
                        x = clip.right();
                    }
                    truncated = true;
                    continue;
                }
                WrapMode::Wrap => {
                    y = y.saturating_add(1);
                    x = clip.left();
                    if y >= clip.bottom() {
                        return Position::new(x, y);
                    }
                    if x + w > clip.right() {
                        return Position::new(x, y);
                    }
                }
            }
        }
        if clip.contains(Position::new(x, y)) {
            // The surface already measured this cluster, so build the cell
            // directly: `Cell::new` would measure it a second time only for
            // the result to be overwritten here.
            let cell = Cell {
                content: Content::from(cluster),
                style: style.clone(),
                kind: if w == 2 { Kind::Wide } else { Kind::Narrow },
            };
            target.set_cell(Position::new(x, y), &cell);
        }
        x += w;
    }
    Position::new(x, y)
}

/// Stamp `tail` over the trailing `tail.width` columns of row `y`, ending at
/// `clip`'s right edge, painted literally with the tail's style.
fn stamp_literal_tail<S: SurfaceMut + ?Sized>(
    target: &mut S,
    tail: &LiteralTail<'_>,
    clip: Rect,
    y: u16,
    mode: WidthMode,
    eaw_wide: bool,
) {
    let tail_x = clip.right().saturating_sub(tail.width);
    let sub = Rect::new(tail_x, y, tail.width, 1).intersection(clip);
    paint_literal_inner(
        target,
        Position::new(tail_x, y),
        sub,
        tail.text,
        WrapMode::Truncate,
        mode,
        eaw_wide,
        tail.style,
        None,
    );
}
