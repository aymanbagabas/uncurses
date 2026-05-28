//! [`Painter`] — bind a target [`SurfaceMut`] and paint styled strings,
//! interpreting inline SGR (`CSI … m`) and OSC 8 hyperlinks in the
//! input.
//!
//! Construct a painter over any [`SurfaceMut`]:
//!
//! ```ignore
//! use uncurses::text::{Painter, WrapMode};
//!
//! Painter::new(&mut buf)
//!     .set_str((0, 0), "hello \x1b[1mworld\x1b[m", WrapMode::Truncate);
//! ```
//!
//! Inline SGR sequences update the painter's current [`Style`]; OSC 8
//! sequences update its current [`Link`]. The state persists across
//! calls so a single painter can stitch many styled segments together.
//! Call [`Painter::reset`] to clear both back to their empty values.

use crate::ansi::hyperlink::parse_hyperlink;
use crate::ansi::params::Params;
use crate::ansi::text::{Token, tokenize};
use crate::buffer::SurfaceMut;
use crate::cell::{Cell, Link};
use crate::layout::{Position, Rect};
use crate::style::{Style, read_style};

use super::WidthMode;

/// Behavior when a cluster would extend past the right edge of the
/// painter's clip rectangle.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum WrapMode {
    /// Stop painting at the right edge of the current row.
    #[default]
    Truncate,
    /// Continue on the next row at the left edge of the clip rect, until
    /// the bottom edge is reached.
    Wrap,
}

/// Paints strings into a [`SurfaceMut`], honouring inline SGR + OSC 8
/// sequences in the input.
///
/// The painter is parameterised by a [`WidthMode`] and an East-Asian
/// Ambiguous policy (`eaw_wide`) — both fixed for the lifetime of the
/// painter — plus a *current* [`Style`] and [`Link`] that are mutated
/// in-place when the input contains `CSI … m` or `OSC 8 ; … ; … ST`
/// sequences. The current state persists across [`set_str`](Self::set_str)
/// /[`set_str_rect`](Self::set_str_rect) calls; use [`reset`](Self::reset)
/// to clear it.
pub struct Painter<'s, S: SurfaceMut + ?Sized> {
    target: &'s mut S,
    mode: WidthMode,
    eaw_wide: bool,
    style: Style,
    link: Link,
}

impl<'s, S: SurfaceMut + ?Sized> Painter<'s, S> {
    /// New painter writing into `target` with default [`WidthMode`],
    /// `eaw_wide = false`, and empty current style + link.
    pub fn new(target: &'s mut S) -> Self {
        Self {
            target,
            mode: WidthMode::default(),
            eaw_wide: false,
            style: Style::EMPTY,
            link: Link::EMPTY,
        }
    }

    /// Override the width-measurement policy.
    pub fn with_mode(mut self, mode: WidthMode) -> Self {
        self.mode = mode;
        self
    }

    /// Override the East-Asian Ambiguous policy.
    pub fn with_eaw_wide(mut self, eaw_wide: bool) -> Self {
        self.eaw_wide = eaw_wide;
        self
    }

    /// Clear the current style and link back to [`Style::EMPTY`] /
    /// [`Link::EMPTY`]. Returns `&mut self` for chaining.
    pub fn reset(&mut self) -> &mut Self {
        self.style = Style::EMPTY;
        self.link = Link::EMPTY;
        self
    }

    /// The bound width-measurement policy.
    pub fn mode(&self) -> WidthMode {
        self.mode
    }

    /// The bound East-Asian Ambiguous policy.
    pub fn eaw_wide(&self) -> bool {
        self.eaw_wide
    }

    /// The painter's current style (mutated by inline SGR).
    pub fn style(&self) -> Style {
        self.style
    }

    /// The painter's current link (mutated by inline OSC 8).
    pub fn link(&self) -> &Link {
        &self.link
    }

    /// Paint `s` starting at `pos`, clipped to the target's bounds.
    ///
    /// Inline SGR (`CSI … m`) updates the painter's style; inline OSC 8
    /// updates its link. `\n` advances to the next row at the bounds'
    /// left edge; `\r` returns to that left edge on the current row.
    /// `wrap` controls behaviour when a cluster would cross the right
    /// edge: [`WrapMode::Truncate`] stops, [`WrapMode::Wrap`] continues
    /// on the next row until the bottom edge is reached.
    ///
    /// Returns the position immediately after the last written cell.
    pub fn set_str(&mut self, pos: impl Into<Position>, s: &str, wrap: WrapMode) -> Position {
        let clip = self.target.bounds();
        self.paint(pos.into(), clip, s, wrap)
    }

    /// Like [`set_str`](Self::set_str) but resets the painter's current
    /// style + link to `style` + `link` before painting. The painter's
    /// state may be further mutated by inline SGR/OSC 8 in `s`.
    pub fn set_str_with(
        &mut self,
        pos: impl Into<Position>,
        s: &str,
        wrap: WrapMode,
        style: Style,
        link: Link,
    ) -> Position {
        self.style = style;
        self.link = link;
        self.set_str(pos, s, wrap)
    }

    /// Paint `s` into `rect`, clipped to `rect ∩ target.bounds()`.
    /// Painting starts at `rect`'s top-left.
    ///
    /// `\n` / `\r` use `rect`'s left edge as the carriage-return column.
    /// `wrap` is explicit: [`WrapMode::Wrap`] flows down inside `rect`,
    /// [`WrapMode::Truncate`] stops at `rect`'s right edge.
    pub fn set_str_rect(&mut self, rect: impl Into<Rect>, s: &str, wrap: WrapMode) -> Position {
        let rect = rect.into();
        let clip = rect.intersection(self.target.bounds());
        self.paint(rect.position(), clip, s, wrap)
    }

    /// Like [`set_str_rect`](Self::set_str_rect) but resets the
    /// painter's current style + link to `style` + `link` before
    /// painting.
    pub fn set_str_rect_with(
        &mut self,
        rect: impl Into<Rect>,
        s: &str,
        wrap: WrapMode,
        style: Style,
        link: Link,
    ) -> Position {
        self.style = style;
        self.link = link;
        self.set_str_rect(rect, s, wrap)
    }

    fn paint(&mut self, start: Position, clip: Rect, s: &str, wrap: WrapMode) -> Position {
        if clip.is_empty() {
            return start;
        }
        let mut x = start.x;
        let mut y = start.y;
        let mut pending: Option<(u16, u16, String, u8)> = None;

        for tok in tokenize(s.as_bytes(), self.mode, self.eaw_wide) {
            match tok {
                Token::Text { text, width } => {
                    // SAFETY: input is `&str` (valid UTF-8) and the
                    // tokenizer cuts on grapheme-cluster boundaries, so
                    // `text` is always a valid UTF-8 sub-slice.
                    let g = unsafe { std::str::from_utf8_unchecked(text) };
                    let cw = width as u8;
                    if cw == 0 {
                        if let Some((_, _, ref mut content, _)) = pending {
                            content.push_str(g);
                        }
                        continue;
                    }
                    flush_pending(self.target, &mut pending, clip, &self.style, &self.link);

                    if x + cw as u16 > clip.right() {
                        match wrap {
                            WrapMode::Truncate => return Position::new(x, y),
                            WrapMode::Wrap => {
                                y = y.saturating_add(1);
                                x = clip.left();
                                if y >= clip.bottom() {
                                    return Position::new(x, y);
                                }
                                if x + cw as u16 > clip.right() {
                                    return Position::new(x, y);
                                }
                            }
                        }
                    }
                    pending = Some((x, y, g.to_string(), cw));
                    x += cw as u16;
                }
                Token::Escape(seq) => {
                    if seq.last() == Some(&b'm')
                        && let Some(body) = csi_body(seq)
                    {
                        flush_pending(self.target, &mut pending, clip, &self.style, &self.link);
                        read_style(Params::from_raw(body), &mut self.style);
                    } else if let Some(body) = osc_body(seq)
                        && let Some((params, url)) = parse_hyperlink(body)
                    {
                        flush_pending(self.target, &mut pending, clip, &self.style, &self.link);
                        self.link = if url.is_empty() {
                            Link::EMPTY
                        } else {
                            Link::new(url).with_params(params)
                        };
                    }
                }
                Token::Control(0x0A) => {
                    flush_pending(self.target, &mut pending, clip, &self.style, &self.link);
                    y = y.saturating_add(1);
                    x = clip.left();
                    if y >= clip.bottom() {
                        return Position::new(x, y);
                    }
                }
                Token::Control(0x0D) => {
                    flush_pending(self.target, &mut pending, clip, &self.style, &self.link);
                    x = clip.left();
                }
                Token::Control(_) => {}
            }
        }
        flush_pending(self.target, &mut pending, clip, &self.style, &self.link);
        Position::new(x, y)
    }
}

fn flush_pending<S: SurfaceMut + ?Sized>(
    target: &mut S,
    pending: &mut Option<(u16, u16, String, u8)>,
    clip: Rect,
    style: &Style,
    link: &Link,
) {
    if let Some((px, py, content, w)) = pending.take()
        && clip.contains(Position::new(px, py))
    {
        target.set_cell(Position::new(px, py), &make_cell(&content, w, style, link));
    }
}

fn make_cell(content: &str, width: u8, style: &Style, link: &Link) -> Cell {
    let mut cell = Cell::new(content, width).with_style(*style);
    if !link.is_empty() {
        cell = cell.with_link(link.clone());
    }
    cell
}

/// Return the body of a CSI sequence (between introducer and final byte).
///
/// Recognises both `\x1b[ … <final>` (7-bit) and `\x9b … <final>` (8-bit)
/// forms where `<final>` is in `0x40..=0x7e`. Returns `None` for any
/// other escape or for an incomplete sequence missing its final byte.
fn csi_body(seq: &[u8]) -> Option<&[u8]> {
    let body_start = if seq.len() >= 2 && seq[0] == 0x1b && seq[1] == b'[' {
        2
    } else if !seq.is_empty() && seq[0] == 0x9b {
        1
    } else {
        return None;
    };
    let last = *seq.last()?;
    if !(0x40..=0x7e).contains(&last) || seq.len() <= body_start {
        return None;
    }
    Some(&seq[body_start..seq.len() - 1])
}

/// Return the body of an OSC sequence (between introducer and string
/// terminator). Recognises `\x1b] … (BEL | ESC \\ | 0x9c)?` (7-bit) and
/// `\x9d … (BEL | 0x9c | ESC \\)?` (8-bit) forms. An incomplete sequence
/// missing its terminator still returns its content; a non-OSC sequence
/// returns `None`.
fn osc_body(seq: &[u8]) -> Option<&[u8]> {
    let body_start = if seq.len() >= 2 && seq[0] == 0x1b && seq[1] == b']' {
        2
    } else if !seq.is_empty() && seq[0] == 0x9d {
        1
    } else {
        return None;
    };
    if seq.len() <= body_start {
        return Some(&[]);
    }
    let end = if seq.ends_with(b"\x1b\\") {
        seq.len() - 2
    } else if matches!(seq.last(), Some(0x07 | 0x9c)) {
        seq.len() - 1
    } else {
        seq.len()
    };
    if end < body_start {
        return Some(&[]);
    }
    Some(&seq[body_start..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Buffer, Surface};
    use crate::color::{BasicColor, Color};
    use crate::style::AttrFlags;

    fn buf(width: u16, height: u16) -> Buffer {
        Buffer::new(width, height)
    }

    fn cell_at(b: &Buffer, x: u16, y: u16) -> Cell {
        b.cell(Position::new(x, y)).cloned().unwrap()
    }

    #[test]
    fn plain_text() {
        let mut b = buf(10, 1);
        let end = Painter::new(&mut b).set_str((0, 0), "abc", WrapMode::Truncate);
        assert_eq!(end, Position::new(3, 0));
        assert_eq!(cell_at(&b, 0, 0).content(), "a");
        assert_eq!(cell_at(&b, 2, 0).content(), "c");
    }

    #[test]
    fn sgr_updates_style_mid_stream() {
        let mut b = buf(10, 1);
        let mut p = Painter::new(&mut b);
        let end = p.set_str((0, 0), "a\x1b[1mb\x1b[mc", WrapMode::Truncate);
        assert_eq!(end, Position::new(3, 0));
        let c0 = cell_at(&b, 0, 0);
        let c1 = cell_at(&b, 1, 0);
        let c2 = cell_at(&b, 2, 0);
        assert!(!c0.style().attrs.contains(AttrFlags::BOLD));
        assert!(c1.style().attrs.contains(AttrFlags::BOLD));
        assert!(!c2.style().attrs.contains(AttrFlags::BOLD));
    }

    #[test]
    fn sgr_color() {
        let mut b = buf(5, 1);
        Painter::new(&mut b).set_str((0, 0), "\x1b[31mr", WrapMode::Truncate);
        assert_eq!(
            cell_at(&b, 0, 0).style().fg,
            Some(Color::Basic(BasicColor::Red))
        );
    }

    #[test]
    fn osc8_toggles_link() {
        let mut b = buf(10, 1);
        Painter::new(&mut b).set_str(
            (0, 0),
            "\x1b]8;;https://x\x1b\\a\x1b]8;;\x1b\\b",
            WrapMode::Truncate,
        );
        assert_eq!(cell_at(&b, 0, 0).link().url, "https://x");
        assert!(!cell_at(&b, 1, 0).has_link());
    }

    #[test]
    fn osc8_malformed_ignored() {
        // Missing the second `;` -> not a valid OSC 8; should not affect
        // the currently active link.
        let mut b = buf(10, 1);
        let mut p = Painter::new(&mut b);
        p.set_str(
            (0, 0),
            "\x1b]8;;https://x\x1b\\a\x1b]8;garbage\x1b\\b",
            WrapMode::Truncate,
        );
        assert_eq!(cell_at(&b, 0, 0).link().url, "https://x");
        assert_eq!(cell_at(&b, 1, 0).link().url, "https://x");
    }

    #[test]
    fn newline_advances_row() {
        let mut b = buf(5, 3);
        let end = Painter::new(&mut b).set_str((0, 0), "ab\ncd", WrapMode::Truncate);
        assert_eq!(cell_at(&b, 0, 0).content(), "a");
        assert_eq!(cell_at(&b, 1, 0).content(), "b");
        assert_eq!(cell_at(&b, 0, 1).content(), "c");
        assert_eq!(cell_at(&b, 1, 1).content(), "d");
        assert_eq!(end, Position::new(2, 1));
    }

    #[test]
    fn cr_returns_to_left() {
        let mut b = buf(5, 1);
        Painter::new(&mut b).set_str((0, 0), "abc\rXY", WrapMode::Truncate);
        // 'X' overwrites 'a', 'Y' overwrites 'b', 'c' remains.
        assert_eq!(cell_at(&b, 0, 0).content(), "X");
        assert_eq!(cell_at(&b, 1, 0).content(), "Y");
        assert_eq!(cell_at(&b, 2, 0).content(), "c");
    }

    #[test]
    fn newline_past_bottom_returns() {
        let mut b = buf(5, 2);
        let end = Painter::new(&mut b).set_str((0, 0), "a\nb\nc", WrapMode::Truncate);
        assert_eq!(end, Position::new(0, 2));
        assert_eq!(cell_at(&b, 0, 0).content(), "a");
        assert_eq!(cell_at(&b, 0, 1).content(), "b");
        // Row 2 is out of bounds; "c" never lands.
    }

    #[test]
    fn truncate_at_right_edge() {
        let mut b = buf(3, 1);
        let end = Painter::new(&mut b).set_str((0, 0), "abcdef", WrapMode::Truncate);
        assert_eq!(end, Position::new(3, 0));
        assert_eq!(cell_at(&b, 0, 0).content(), "a");
        assert_eq!(cell_at(&b, 2, 0).content(), "c");
    }

    #[test]
    fn wrap_at_right_edge() {
        let mut b = buf(3, 3);
        let end = Painter::new(&mut b).set_str((0, 0), "abcdef", WrapMode::Wrap);
        assert_eq!(end, Position::new(3, 1));
        assert_eq!(cell_at(&b, 0, 0).content(), "a");
        assert_eq!(cell_at(&b, 2, 0).content(), "c");
        assert_eq!(cell_at(&b, 0, 1).content(), "d");
        assert_eq!(cell_at(&b, 2, 1).content(), "f");
    }

    #[test]
    fn rect_clip_and_origin() {
        let mut b = buf(10, 5);
        let end =
            Painter::new(&mut b).set_str_rect(Rect::new(2, 1, 3, 2), "abcdef", WrapMode::Wrap);
        assert_eq!(end, Position::new(5, 2));
        assert_eq!(cell_at(&b, 2, 1).content(), "a");
        assert_eq!(cell_at(&b, 4, 1).content(), "c");
        assert_eq!(cell_at(&b, 2, 2).content(), "d");
        assert_eq!(cell_at(&b, 4, 2).content(), "f");
        // Outside the rect must remain blank.
        assert_eq!(cell_at(&b, 0, 0).content(), " ");
        assert_eq!(cell_at(&b, 5, 1).content(), " ");
    }

    #[test]
    fn rect_newline_uses_rect_left() {
        let mut b = buf(10, 5);
        Painter::new(&mut b).set_str_rect(Rect::new(2, 1, 4, 3), "ab\ncd", WrapMode::Truncate);
        assert_eq!(cell_at(&b, 2, 1).content(), "a");
        assert_eq!(cell_at(&b, 3, 1).content(), "b");
        // Newline returns x to rect.left() = 2, not to 0.
        assert_eq!(cell_at(&b, 2, 2).content(), "c");
        assert_eq!(cell_at(&b, 3, 2).content(), "d");
        assert_eq!(cell_at(&b, 0, 2).content(), " ");
    }

    #[test]
    fn with_resets_style_and_link() {
        let mut b = buf(10, 1);
        // First call: paint with bold + a link.
        Painter::new(&mut b).set_str_with(
            (0, 0),
            "a",
            WrapMode::Truncate,
            Style::EMPTY.bold(),
            Link::new("https://x"),
        );
        assert!(cell_at(&b, 0, 0).style().attrs.contains(AttrFlags::BOLD));
        assert_eq!(cell_at(&b, 0, 0).link().url, "https://x");
        // Second call with `_with` and empty style/link must reset.
        Painter::new(&mut b).set_str_with(
            (1, 0),
            "b",
            WrapMode::Truncate,
            Style::EMPTY,
            Link::EMPTY,
        );
        assert!(!cell_at(&b, 1, 0).style().attrs.contains(AttrFlags::BOLD));
        assert!(!cell_at(&b, 1, 0).has_link());
    }

    #[test]
    fn state_persists_across_calls() {
        let mut b = buf(10, 1);
        let mut p = Painter::new(&mut b);
        p.set_str((0, 0), "\x1b[1ma", WrapMode::Truncate);
        // Style mutation persists into the next call.
        p.set_str((1, 0), "b", WrapMode::Truncate);
        assert!(cell_at(&b, 0, 0).style().attrs.contains(AttrFlags::BOLD));
        assert!(cell_at(&b, 1, 0).style().attrs.contains(AttrFlags::BOLD));
    }

    #[test]
    fn reset_clears_style_and_link() {
        let mut b = buf(10, 1);
        let mut p = Painter::new(&mut b);
        p.set_str(
            (0, 0),
            "\x1b[1m\x1b]8;;https://x\x1b\\a",
            WrapMode::Truncate,
        );
        assert!(p.style().attrs.contains(AttrFlags::BOLD));
        assert_eq!(p.link().url, "https://x");
        p.reset();
        assert!(p.style().is_empty());
        assert!(p.link().is_empty());
        // Subsequent paint reflects the reset.
        p.set_str((1, 0), "b", WrapMode::Truncate);
        assert!(!cell_at(&b, 1, 0).style().attrs.contains(AttrFlags::BOLD));
        assert!(!cell_at(&b, 1, 0).has_link());
    }

    #[test]
    fn position_and_rect_match_when_rect_covers_bounds() {
        let mut a = buf(5, 2);
        let mut b = buf(5, 2);
        let e1 = Painter::new(&mut a).set_str((0, 0), "abc", WrapMode::Truncate);
        let e2 =
            Painter::new(&mut b).set_str_rect(Rect::new(0, 0, 5, 2), "abc", WrapMode::Truncate);
        assert_eq!(e1, e2);
        assert_eq!(cell_at(&a, 2, 0).content(), cell_at(&b, 2, 0).content());
    }
}
