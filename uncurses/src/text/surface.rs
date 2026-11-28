//! [`TextSurface`] — text drawing as an extension of any mutable surface.
//!
//! [`SurfaceMut`](crate::buffer::SurfaceMut) reads and writes cells.
//! [`Surface`](crate::buffer::Surface) supplies the width policy.
//! `TextSurface` adds text helpers; the default `set_str`
//! family segments the input into grapheme clusters and paints each with the
//! given style.
//! [`crate::buffer::Buffer`] implements this trait and owns its width policy.
//! [`crate::buffer::TextBuffer`] forwards that policy to its underlying buffer.
//! Structural cell copies preserve stored widths.
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
//! ```rust
//! use uncurses::layout::Position;
//! use uncurses::style::Style;
//! use uncurses::text::TextSurface;
//!
//! fn label(surface: &mut impl TextSurface, at: Position, text: &str, style: Style) {
//!     surface.set_str(at, text, style);
//! }
//! ```

use crate::buffer::SurfaceMut;
use crate::cell::storage::{PackedCell, Resources};
use crate::cell::{BorrowedCell, Cell, CellRef};
use crate::layout::{Position, Rect, overruns};
use crate::style::Style;

use super::{WidthMode, WrapMode, grapheme_cells};

/// A [`SurfaceMut`] with a text-measurement policy and string-painting helpers.
///
/// Implement this for surface types that can accept styled text.
/// Text helpers use the width policy from [`Surface`](crate::buffer::Surface).
///
/// The default `set_str` family paints text literally: each grapheme cluster is
/// drawn with the given style and inline SGR / OSC 8 escapes are not
/// interpreted. Use [`Painter`](super::Painter), which is itself a
/// `TextSurface`, to parse inline style and hyperlink escapes. Newline and
/// carriage return move the paint cursor within the active clip rectangle.
pub trait TextSurface: SurfaceMut {
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
    fn set_str(&mut self, pos: impl Into<Position>, s: &str, style: impl Into<Style>) -> Position {
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
        style: impl Into<Style>,
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
        style: impl Into<Style>,
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
        style: impl Into<Style>,
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
    /// painted literally with `tail_style`; `s` uses `s_style`. Use
    /// [`Painter`](super::Painter) to interpret inline escapes in either
    /// string. A tail wider than the surface is dropped in favor of a hard
    /// truncate.
    ///
    /// # Parameters
    ///
    /// * `pos` — starting cell position.
    /// * `s` — UTF-8 string to paint.
    /// * `s_style` — style for the main text.
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
        s_style: impl Into<Style>,
        tail: &str,
        tail_style: impl Into<Style>,
    ) -> Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide());
        let clip = self.bounds();
        paint_literal_truncate(
            self,
            pos.into(),
            clip,
            s,
            &s_style.into(),
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
    /// * `s_style` — style for the main text.
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
        s_style: impl Into<Style>,
        tail: &str,
        tail_style: impl Into<Style>,
    ) -> Position {
        let (mode, eaw) = (self.width_mode(), self.eaw_wide());
        let rect = rect.into();
        let clip = rect.intersection(self.bounds());
        paint_literal_truncate(
            self,
            rect.position(),
            clip,
            s,
            &s_style.into(),
            tail,
            &tail_style.into(),
            mode,
            eaw,
        )
    }

    /// Measure the display width of `s` in terminal columns.
    ///
    /// The measurement segments `s` into grapheme clusters under this
    /// surface's [`width_mode`](crate::buffer::Surface::width_mode) and
    /// [`eaw_wide`](crate::buffer::Surface::eaw_wide) policy and sums their widths. Like the
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
    style: &'a Style,
    width: u16,
}

/// Paint `s` literally, clipped to `clip`, with `wrap` behavior at the right
/// edge. Each grapheme cluster is drawn with `style`; inline escapes are not
/// interpreted (they are segmented and drawn like any other text). Newline and
/// carriage return reposition within `clip`.
/// A single visible scalar that fits inside the clip bypasses segmentation.
#[allow(clippy::too_many_arguments)]
fn paint_literal<S: SurfaceMut + ?Sized>(
    target: &mut S,
    start: Position,
    clip: Rect,
    s: &str,
    wrap: WrapMode,
    mode: WidthMode,
    eaw_wide: bool,
    style: &Style,
) -> Position {
    // Test the whole input, not its first byte: ASCII can start a combining grapheme.
    if s.len() == 1 && (b' '..=b'~').contains(&s.as_bytes()[0]) && clip.contains(start) {
        let ascii = char::from(s.as_bytes()[0]);
        if let Some(cell) = PackedCell::from_scalar(Some(ascii), 1, style) {
            target.set_cell(start, CellRef::packed(&cell, Resources::empty_ref()));
        } else {
            target.set_cell(start, Cell::new(s, 1).style(style.clone()).as_ref());
        }
        return Position::new(start.x + 1, start.y);
    }
    paint_literal_unicode(target, start, clip, s, wrap, mode, eaw_wide, style)
}

// Keep segmentation and width measurement outside the ASCII write path.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn paint_literal_unicode<S: SurfaceMut + ?Sized>(
    target: &mut S,
    start: Position,
    clip: Rect,
    s: &str,
    wrap: WrapMode,
    mode: WidthMode,
    eaw_wide: bool,
    style: &Style,
) -> Position {
    if let Some(scalar) = s.chars().next()
        && scalar.len_utf8() == s.len()
        && clip.contains(start)
    {
        let width = mode.grapheme_width(s, eaw_wide);
        if width != 0 && !overruns(start.x, u16::from(width), clip.right()) {
            if let Some(cell) = PackedCell::from_scalar(Some(scalar), width, style) {
                target.set_cell(start, CellRef::packed(&cell, Resources::empty_ref()));
            } else {
                target.set_cell(start, BorrowedCell::new(s, width, style).as_ref());
            }
            return Position::new(start.x + u16::from(width), start.y);
        }
    }
    paint_literal_inner(target, start, clip, s, wrap, mode, eaw_wide, style, None)
}

/// Paint `s` literally with [`WrapMode::Truncate`], stamping `tail` on overflow.
///
/// The main text is drawn with `s_style`; the tail is drawn with
/// `tail_style`. The tail is dropped (hard truncate) when it is empty or wider
/// than the clip.
#[allow(clippy::too_many_arguments)]
fn paint_literal_truncate<S: SurfaceMut + ?Sized>(
    target: &mut S,
    start: Position,
    clip: Rect,
    s: &str,
    s_style: &Style,
    tail_text: &str,
    tail_style: &Style,
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
        s_style,
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
    style: &Style,
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
        let cw = w;
        let w = u16::from(w);
        if overruns(x, w, clip.right()) {
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
                    if overruns(x, w, clip.right()) {
                        return Position::new(x, y);
                    }
                }
            }
        }
        if clip.contains(Position::new(x, y)) {
            target.set_cell(
                Position::new(x, y),
                BorrowedCell::new(cluster, cw, style).as_ref(),
            );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Buffer, Surface, TextBuffer, View};
    use crate::color::Color;
    use crate::style::Style;

    #[test]
    fn single_ascii_matches_general_paint_at_clip_edges() {
        let style = Style::new().fg(Color::Red).bg(Color::Blue).bold();
        for clip in [Rect::new(2, 1, 3, 2), Rect::new(2, 1, 0, 2)] {
            for start in [
                (2, 1),
                (4, 1),
                (5, 1),
                (5, 2),
                (1, 1),
                (2, 0),
                (2, 3),
                (u16::MAX, u16::MAX),
            ] {
                for wrap in [WrapMode::Truncate, WrapMode::Wrap] {
                    let mut fast = Buffer::new(8, 4);
                    let mut general = fast.clone();
                    let actual = View::new(&mut fast, clip).set_str_wrap(start, "A", wrap, &style);
                    let expected = paint_literal_inner(
                        &mut general,
                        start.into(),
                        clip,
                        "A",
                        wrap,
                        WidthMode::Wc,
                        false,
                        &style,
                        None,
                    );
                    assert_eq!(actual, expected, "{clip:?}, {start:?}, {wrap:?}");
                    for y in 0..4 {
                        assert_eq!(fast.row(y), general.row(y));
                    }
                }
            }
        }
    }

    #[test]
    fn unicode_literal_matches_segmented_cell_writes() {
        use crate::cell::Cell;

        let plain = Style::new().fg(Color::Indexed(0)).bg(Color::Rgb(3, 5, 7));
        let rich = plain
            .clone()
            .underline_color(Color::Green)
            .link("https://example.test/unicode", "id=unicode");
        let clip = Rect::new(2, 1, 3, 2);
        for text in [
            "é",
            "界",
            "…",
            "🇯",
            "\u{301}",
            "\u{200d}",
            "\r",
            "\n",
            "\r\n",
            "\x1b",
            "e\u{301}",
            "👩‍💻",
            "✋\u{fe0e}",
            "",
        ] {
            for mode in [WidthMode::Wc, WidthMode::Grapheme] {
                for eaw in [false, true] {
                    for style in [&plain, &rich] {
                        for start in [(2, 1), (4, 1), (5, 1), (4, 2), (1, 1), (2, 0), (2, 3)] {
                            for wrap in [WrapMode::Truncate, WrapMode::Wrap] {
                                let mut actual =
                                    Buffer::new(8, 4).with_width_mode(mode).with_eaw_wide(eaw);
                                let mut expected = actual.clone();
                                let result = View::new(&mut actual, clip)
                                    .set_str_wrap(start, text, wrap, style);
                                // Keep portable Cell construction as an independent
                                // oracle for the borrowed import path.
                                struct Owned<'a>(&'a mut Buffer);
                                impl crate::buffer::Bounded for Owned<'_> {
                                    fn bounds(&self) -> Rect {
                                        self.0.bounds()
                                    }
                                }
                                impl Surface for Owned<'_> {
                                    fn cell(&self, pos: Position) -> Option<CellRef<'_>> {
                                        self.0.cell(pos)
                                    }
                                }
                                impl SurfaceMut for Owned<'_> {
                                    fn set_cell(&mut self, pos: Position, cell: CellRef<'_>) {
                                        let value = Cell::new(cell.content(), cell.width())
                                            .style(cell.to_style());
                                        self.0.set(pos, &value);
                                    }
                                    fn set_style(&mut self, pos: Position, style: Style) {
                                        self.0.set_style(pos, style);
                                    }
                                }
                                let position = paint_literal_inner(
                                    &mut Owned(&mut expected),
                                    start.into(),
                                    clip,
                                    text,
                                    wrap,
                                    mode,
                                    eaw,
                                    style,
                                    None,
                                );
                                assert_eq!(
                                    result, position,
                                    "{text:?}, {start:?}, {mode:?}, {eaw}"
                                );
                                for y in 0..4 {
                                    assert_eq!(actual.row(y), expected.row(y), "{text:?}");
                                }
                            }
                        }
                    }
                }
            }
        }
        for (text, mode, eaw, width) in [
            ("é", WidthMode::Wc, false, 1),
            ("界", WidthMode::Grapheme, false, 2),
            ("…", WidthMode::Wc, true, 2),
            ("…", WidthMode::Grapheme, false, 1),
            ("🇯", WidthMode::Wc, false, 1),
            ("🇯", WidthMode::Grapheme, false, 2),
        ] {
            let mut buffer = Buffer::new(4, 1).with_width_mode(mode).with_eaw_wide(eaw);
            assert_eq!(
                buffer.set_str((0, 0), text, &plain),
                Position::new(width, 0)
            );
            assert_eq!(buffer.cell((0, 0).into()).unwrap().content(), text);
        }
    }

    #[test]
    fn literal_short_inputs_preserve_styles_and_whole_graphemes() {
        let rich = Style::new()
            .fg(Color::Rgb(3, 5, 7))
            .underline_color(Color::Green)
            .link("https://example.test/literal", "id=literal");
        let blank = Style::new().bg(Color::Blue).italic();
        for (text, style) in [
            ("A", &rich),
            (" ", &blank),
            ("e\u{301}", &rich),
            ("\u{301}", &blank),
            ("\r\nA", &rich),
            ("\x1b", &blank),
            ("界", &rich),
        ] {
            for mode in [WidthMode::Wc, WidthMode::Grapheme] {
                let mut fast = Buffer::new(4, 2).with_width_mode(mode).with_eaw_wide(true);
                let mut general = fast.clone();
                let original = style.clone();
                let actual = fast.set_str((0, 0), text, style);
                let expected = paint_literal_inner(
                    &mut general,
                    Position::new(0, 0),
                    Rect::new(0, 0, 4, 2),
                    text,
                    WrapMode::Truncate,
                    mode,
                    true,
                    style,
                    None,
                );
                assert_eq!(actual, expected, "{text:?}, {mode:?}");
                for y in 0..2 {
                    assert_eq!(fast.row(y), general.row(y), "{text:?}, {mode:?}");
                }
                assert_eq!(*style, original);
                if text == "A" || text == " " {
                    assert!(fast.cell(Position::new(0, 0)).unwrap().style_eq(style));
                }
            }
        }
    }

    /// `right()` is exclusive and saturates at `u16::MAX`, so a row that ends
    /// at the top of the address space can leave `x` equal to it. Adding the
    /// next cluster's width there overflowed and panicked.
    #[test]
    fn a_row_ending_at_the_last_addressable_column_truncates_instead_of_panicking() {
        let mut buf = TextBuffer::new(u16::MAX, 1);
        let s: String = "a".repeat(usize::from(u16::MAX) + 8);
        buf.set_str((0, 0), &s, Style::default());

        // A cluster that claims many columns has a wider window in which to
        // overflow, so check it lands on the same truncating path.
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
        let mut wide = TextBuffer::new(u16::MAX, 1);
        wide.set_str(
            (0, 0),
            &"a".repeat(usize::from(u16::MAX) - 2),
            Style::default(),
        );
        wide.set_str((u16::MAX - 2, 0), family, Style::default());
    }
}
