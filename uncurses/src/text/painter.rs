//! [`Painter`] — bind a target [`SurfaceMut`] and paint styled strings,
//! interpreting inline SGR (`CSI … m`) and OSC 8 hyperlinks in the
//! input.
//!
//! Construct a painter over any [`SurfaceMut`]:
//!
//! ```ignore
//! use uncurses::text::Painter;
//! use uncurses::style::Style;
//!
//! Painter::new(&mut buf, WidthMode::default(), false)
//!     .set_str((0, 0), "hello \x1b[1mworld\x1b[m", Style::default());
//! ```
//!
//! Each call takes a starting [`Style`]; inline SGR sequences then update
//! the painter's current [`Style`] and OSC 8 sequences attach a hyperlink
//! to subsequent cells via the same [`Style`]. The running style is
//! readable from [`Painter::style`] after a call, so a single painter can
//! stitch many styled segments together by feeding it back in. Call
//! [`Painter::reset`] to clear the style back to its empty value.

use crate::ansi::hyperlink::parse_hyperlink;
use crate::ansi::params::Params;
use crate::ansi::text::{Token, tokenize};
use crate::buffer::SurfaceMut;
use crate::cell::Cell;
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
/// painter — plus a *current* [`Style`] (including any active
/// hyperlink). Each [`set_str`](Self::set_str) /
/// [`set_str_rect`](Self::set_str_rect) call takes a starting style and
/// then mutates it in-place as the input yields `CSI … m` or
/// `OSC 8 ; … ; … ST` sequences; the resulting style is readable from
/// [`style`](Self::style) afterwards. Use [`reset`](Self::reset) to clear
/// it.
pub struct Painter<'s, S: SurfaceMut + ?Sized> {
    target: &'s mut S,
    /// Width measurement policy.
    pub mode: WidthMode,
    /// Whether East Asian Ambiguous characters are treated as wide.
    pub eaw_wide: bool,
    /// Current painting style.
    pub style: Style,
}

impl<'s, S: SurfaceMut + ?Sized> Painter<'s, S> {
    /// New painter writing into `target` with the given width-measurement
    /// policy and East-Asian Ambiguous policy. Starts with an empty
    /// current style.
    pub fn new(target: &'s mut S, mode: WidthMode, eaw_wide: bool) -> Self {
        Self {
            target,
            mode,
            eaw_wide,
            style: Style::default(),
        }
    }

    /// Clear the current style back to [`Style::default()`]. Returns
    /// `&mut self` for chaining.
    pub fn reset(&mut self) -> &mut Self {
        self.style = Style::default();
        self
    }

    /// Paint `s` starting at `pos`, clipped to the target's bounds, with
    /// `style` as the starting style (truncating at the right edge).
    ///
    /// `style` replaces the painter's current [`Style`] before painting;
    /// inline SGR (`CSI … m`) then updates it, and inline OSC 8 attaches
    /// a hyperlink to the same style. `\n` advances to the next row at
    /// the bounds' left edge; `\r` returns to that left edge on the
    /// current row. Use [`set_str_wrap`](Self::set_str_wrap) to control
    /// wrapping. After the call, the running style is readable from
    /// [`style`](Self::style) and can be fed into the next call to stitch
    /// styled segments together.
    ///
    /// Returns the position immediately after the last written cell.
    pub fn set_str(&mut self, pos: impl Into<Position>, s: &str, style: Style) -> Position {
        self.style = style;
        let clip = self.target.bounds();
        self.paint(pos.into(), clip, s, WrapMode::default())
    }

    /// Like [`set_str`](Self::set_str) but with an explicit [`WrapMode`]:
    /// [`WrapMode::Truncate`] stops at the right edge, [`WrapMode::Wrap`]
    /// continues on the next row until the bottom edge is reached.
    pub fn set_str_wrap(
        &mut self,
        pos: impl Into<Position>,
        s: &str,
        wrap: WrapMode,
        style: Style,
    ) -> Position {
        self.style = style;
        let clip = self.target.bounds();
        self.paint(pos.into(), clip, s, wrap)
    }

    /// Paint `s` into `rect`, clipped to `rect ∩ target.bounds()`, with
    /// `style` as the starting style (truncating at `rect`'s right edge).
    /// Painting starts at `rect`'s top-left.
    ///
    /// `\n` / `\r` use `rect`'s left edge as the carriage-return column.
    /// Use [`set_str_rect_wrap`](Self::set_str_rect_wrap) to control
    /// wrapping.
    pub fn set_str_rect(&mut self, rect: impl Into<Rect>, s: &str, style: Style) -> Position {
        self.style = style;
        let rect = rect.into();
        let clip = rect.intersection(self.target.bounds());
        self.paint(rect.position(), clip, s, WrapMode::default())
    }

    /// Like [`set_str_rect`](Self::set_str_rect) but with an explicit
    /// [`WrapMode`]: [`WrapMode::Wrap`] flows down inside `rect`,
    /// [`WrapMode::Truncate`] stops at `rect`'s right edge.
    pub fn set_str_rect_wrap(
        &mut self,
        rect: impl Into<Rect>,
        s: &str,
        wrap: WrapMode,
        style: Style,
    ) -> Position {
        self.style = style;
        let rect = rect.into();
        let clip = rect.intersection(self.target.bounds());
        self.paint(rect.position(), clip, s, wrap)
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
                    flush_pending(self.target, &mut pending, clip, &self.style);

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
                        flush_pending(self.target, &mut pending, clip, &self.style);
                        read_style(Params::from_raw(body), &mut self.style);
                    } else if let Some(body) = osc_body(seq)
                        && let Some((params, url)) = parse_hyperlink(body)
                    {
                        flush_pending(self.target, &mut pending, clip, &self.style);
                        self.style = self.style.clone().link(url, params);
                    }
                }
                Token::Control(0x0A) => {
                    flush_pending(self.target, &mut pending, clip, &self.style);
                    y = y.saturating_add(1);
                    x = clip.left();
                    if y >= clip.bottom() {
                        return Position::new(x, y);
                    }
                }
                Token::Control(0x0D) => {
                    flush_pending(self.target, &mut pending, clip, &self.style);
                    x = clip.left();
                }
                Token::Control(_) => {}
            }
        }
        flush_pending(self.target, &mut pending, clip, &self.style);
        Position::new(x, y)
    }
}

fn flush_pending<S: SurfaceMut + ?Sized>(
    target: &mut S,
    pending: &mut Option<(u16, u16, String, u8)>,
    clip: Rect,
    style: &Style,
) {
    if let Some((px, py, content, w)) = pending.take()
        && clip.contains(Position::new(px, py))
    {
        let cell = if w == 2 {
            Cell::wide(&*content)
        } else {
            Cell::narrow(&*content)
        };
        target.set_cell(Position::new(px, py), &cell.style(style.clone()));
    }
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

    fn link_of(s: &crate::style::Style) -> Option<(&str, &str)> {
        s.link
            .as_deref()
            .map(|l| (l.url.as_str(), l.params.as_str()))
    }

    #[test]
    fn plain_text() {
        let mut b = buf(10, 1);
        let end = Painter::new(&mut b, WidthMode::default(), false).set_str_wrap(
            (0, 0),
            "abc",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(end, Position::new(3, 0));
        assert_eq!(cell_at(&b, 0, 0).content(), "a");
        assert_eq!(cell_at(&b, 2, 0).content(), "c");
    }

    #[test]
    fn sgr_updates_style_mid_stream() {
        let mut b = buf(10, 1);
        let mut p = Painter::new(&mut b, WidthMode::default(), false);
        let end = p.set_str_wrap(
            (0, 0),
            "a\x1b[1mb\x1b[mc",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(end, Position::new(3, 0));
        let c0 = cell_at(&b, 0, 0);
        let c1 = cell_at(&b, 1, 0);
        let c2 = cell_at(&b, 2, 0);
        assert!(!c0.style.attrs.contains(AttrFlags::BOLD));
        assert!(c1.style.attrs.contains(AttrFlags::BOLD));
        assert!(!c2.style.attrs.contains(AttrFlags::BOLD));
    }

    #[test]
    fn sgr_color() {
        let mut b = buf(5, 1);
        Painter::new(&mut b, WidthMode::default(), false).set_str_wrap(
            (0, 0),
            "\x1b[31mr",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(
            cell_at(&b, 0, 0).style.fg,
            Some(Color::Basic(BasicColor::Red))
        );
    }

    #[test]
    fn osc8_toggles_link() {
        let mut b = buf(10, 1);
        Painter::new(&mut b, WidthMode::default(), false).set_str_wrap(
            (0, 0),
            "\x1b]8;;https://x\x1b\\a\x1b]8;;\x1b\\b",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(link_of(&cell_at(&b, 0, 0).style), Some(("https://x", "")));
        assert!(cell_at(&b, 1, 0).style.link.is_none());
    }

    #[test]
    fn osc8_malformed_ignored() {
        // Missing the second `;` -> not a valid OSC 8; should not affect
        // the currently active link.
        let mut b = buf(10, 1);
        let mut p = Painter::new(&mut b, WidthMode::default(), false);
        p.set_str_wrap(
            (0, 0),
            "\x1b]8;;https://x\x1b\\a\x1b]8;garbage\x1b\\b",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(link_of(&cell_at(&b, 0, 0).style), Some(("https://x", "")));
        assert_eq!(link_of(&cell_at(&b, 1, 0).style), Some(("https://x", "")));
    }

    #[test]
    fn newline_advances_row() {
        let mut b = buf(5, 3);
        let end = Painter::new(&mut b, WidthMode::default(), false).set_str_wrap(
            (0, 0),
            "ab\ncd",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(cell_at(&b, 0, 0).content(), "a");
        assert_eq!(cell_at(&b, 1, 0).content(), "b");
        assert_eq!(cell_at(&b, 0, 1).content(), "c");
        assert_eq!(cell_at(&b, 1, 1).content(), "d");
        assert_eq!(end, Position::new(2, 1));
    }

    #[test]
    fn cr_returns_to_left() {
        let mut b = buf(5, 1);
        Painter::new(&mut b, WidthMode::default(), false).set_str_wrap(
            (0, 0),
            "abc\rXY",
            WrapMode::Truncate,
            Style::default(),
        );
        // 'X' overwrites 'a', 'Y' overwrites 'b', 'c' remains.
        assert_eq!(cell_at(&b, 0, 0).content(), "X");
        assert_eq!(cell_at(&b, 1, 0).content(), "Y");
        assert_eq!(cell_at(&b, 2, 0).content(), "c");
    }

    #[test]
    fn newline_past_bottom_returns() {
        let mut b = buf(5, 2);
        let end = Painter::new(&mut b, WidthMode::default(), false).set_str_wrap(
            (0, 0),
            "a\nb\nc",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(end, Position::new(0, 2));
        assert_eq!(cell_at(&b, 0, 0).content(), "a");
        assert_eq!(cell_at(&b, 0, 1).content(), "b");
        // Row 2 is out of bounds; "c" never lands.
    }

    #[test]
    fn truncate_at_right_edge() {
        let mut b = buf(3, 1);
        let end = Painter::new(&mut b, WidthMode::default(), false).set_str_wrap(
            (0, 0),
            "abcdef",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(end, Position::new(3, 0));
        assert_eq!(cell_at(&b, 0, 0).content(), "a");
        assert_eq!(cell_at(&b, 2, 0).content(), "c");
    }

    #[test]
    fn wrap_at_right_edge() {
        let mut b = buf(3, 3);
        let end = Painter::new(&mut b, WidthMode::default(), false).set_str_wrap(
            (0, 0),
            "abcdef",
            WrapMode::Wrap,
            Style::default(),
        );
        assert_eq!(end, Position::new(3, 1));
        assert_eq!(cell_at(&b, 0, 0).content(), "a");
        assert_eq!(cell_at(&b, 2, 0).content(), "c");
        assert_eq!(cell_at(&b, 0, 1).content(), "d");
        assert_eq!(cell_at(&b, 2, 1).content(), "f");
    }

    #[test]
    fn rect_clip_and_origin() {
        let mut b = buf(10, 5);
        let end = Painter::new(&mut b, WidthMode::default(), false).set_str_rect_wrap(
            Rect::new(2, 1, 3, 2),
            "abcdef",
            WrapMode::Wrap,
            Style::default(),
        );
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
        Painter::new(&mut b, WidthMode::default(), false).set_str_rect_wrap(
            Rect::new(2, 1, 4, 3),
            "ab\ncd",
            WrapMode::Truncate,
            Style::default(),
        );
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
        Painter::new(&mut b, WidthMode::default(), false).set_str_wrap(
            (0, 0),
            "a",
            WrapMode::Truncate,
            Style::default().bold().link("https://x", ""),
        );
        assert!(cell_at(&b, 0, 0).style.attrs.contains(AttrFlags::BOLD));
        assert_eq!(link_of(&cell_at(&b, 0, 0).style), Some(("https://x", "")));
        // Second call with `_with` and an empty style must reset.
        Painter::new(&mut b, WidthMode::default(), false).set_str_wrap(
            (1, 0),
            "b",
            WrapMode::Truncate,
            Style::default(),
        );
        assert!(!cell_at(&b, 1, 0).style.attrs.contains(AttrFlags::BOLD));
        assert!(cell_at(&b, 1, 0).style.link.is_none());
    }

    #[test]
    fn running_style_is_reusable_across_calls() {
        let mut b = buf(10, 1);
        let mut p = Painter::new(&mut b, WidthMode::default(), false);
        p.set_str_wrap((0, 0), "\x1b[1ma", WrapMode::Truncate, Style::default());
        // Inline SGR leaves the running style bold; feed it back in to
        // continue the same style on the next call.
        let running = p.style.clone();
        p.set_str_wrap((1, 0), "b", WrapMode::Truncate, running);
        assert!(cell_at(&b, 0, 0).style.attrs.contains(AttrFlags::BOLD));
        assert!(cell_at(&b, 1, 0).style.attrs.contains(AttrFlags::BOLD));
    }

    #[test]
    fn reset_clears_style_and_link() {
        let mut b = buf(10, 1);
        let mut p = Painter::new(&mut b, WidthMode::default(), false);
        p.set_str_wrap(
            (0, 0),
            "\x1b[1m\x1b]8;;https://x\x1b\\a",
            WrapMode::Truncate,
            Style::default(),
        );
        assert!(p.style.attrs.contains(AttrFlags::BOLD));
        assert_eq!(link_of(&p.style), Some(("https://x", "")));
        p.reset();
        assert!(p.style.is_empty());
        assert!(p.style.link.is_none());
        // Subsequent paint reflects the reset.
        p.set_str_wrap((1, 0), "b", WrapMode::Truncate, Style::default());
        assert!(!cell_at(&b, 1, 0).style.attrs.contains(AttrFlags::BOLD));
        assert!(cell_at(&b, 1, 0).style.link.is_none());
    }

    #[test]
    fn position_and_rect_match_when_rect_covers_bounds() {
        let mut a = buf(5, 2);
        let mut b = buf(5, 2);
        let e1 = Painter::new(&mut a, WidthMode::default(), false).set_str_wrap(
            (0, 0),
            "abc",
            WrapMode::Truncate,
            Style::default(),
        );
        let e2 = Painter::new(&mut b, WidthMode::default(), false).set_str_rect_wrap(
            Rect::new(0, 0, 5, 2),
            "abc",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(e1, e2);
        assert_eq!(cell_at(&a, 2, 0).content(), cell_at(&b, 2, 0).content());
    }
}
