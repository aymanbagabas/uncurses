//! [`Painter`] — styled string painting into a mutable surface.
//!
//! A painter owns no cells and no style. It temporarily binds a
//! [`SurfaceMut`](crate::buffer::SurfaceMut), a [`WidthMode`], and an
//! East-Asian Ambiguous policy. Calls to
//! [`set_str`](Painter::set_str) or [`set_str_rect`](Painter::set_str_rect)
//! tokenize the input into text clusters, inline escapes, and control bytes,
//! then write terminal cells into the target.
//!
//! Construct a painter over any [`TextSurface`]:
//!
//! ```rust,ignore
//! use uncurses::text::Painter;
//! use uncurses::style::Style;
//!
//! Painter::new(&mut surface)
//!     .set_str((0, 0), "hello \x1b[1mworld\x1b[m", Style::default());
//! ```
//!
//! ## Style and hyperlink state
//!
//! Each paint call takes a base [`Style`]. Inline SGR and OSC 8 sequences in
//! the input build a separate pen as the string is scanned, and each cell is
//! that pen inherited onto the base: the pen's own fields win, the base fills
//! anything the pen leaves unset. An inline reset (`\x1b[0m`) clears the pen,
//! so cells after it fall back to the base rather than to the terminal default.
//! The painter keeps no style of its own between calls: every call starts with
//! an empty pen over the base it is given, so calls are independent.
//!
//! ## Cells, clipping, and wrapping
//!
//! Non-zero-width grapheme clusters are written as one-cell or two-cell
//! [`Cell`](crate::cell::Cell) values. Two-cell clusters occupy a primary wide
//! cell plus the continuation cell maintained by the buffer layer. Zero-width
//! clusters are appended to the previous pending cluster before it is flushed.
//!
//! ```text
//! input clusters      pending cell       surface cells
//! ┌────┬──────┐       ┌────────────┐         ┌────┬────┬────┐
//! │ e  │ ◌́    │ ───▶  │ "e\u{301}" │ ─────▶  │ é  │    │    │
//! └────┴──────┘       └────────────┘         └────┴────┴────┘
//!
//! ┌────┐              ┌─────────┐        ┌────┬────┬────┐
//! │ 中 │ ─────────▶   │ width 2 │ ────▶  │ 中 │ ▶  │    │
//! └────┘              └─────────┘        └────┴────┴────┘
//! ```
//!
//! Painting is clipped to either the target bounds or the intersection of a
//! supplied rectangle with those bounds. [`WrapMode`] applies only when a
//! non-zero-width cluster would cross the right edge.

use crate::ansi::hyperlink::parse_hyperlink;
use crate::ansi::params::Params;
use crate::ansi::text::{Token, string_width, tokenize};
use crate::buffer::{Bounded, Surface, SurfaceMut};
use crate::cell::Cell;
use crate::layout::{Position, Rect};
use crate::style::{Style, read_style};

use super::{TextSurface, WidthMode, WrapMode};

/// Paint styled strings into a [`TextSurface`].
///
/// The painter snapshots its target's [`WidthMode`] and `eaw_wide` policy at
/// construction, both fixed for the painter's lifetime. It holds no style of
/// its own: each paint call starts with an empty pen over the base style it is
/// given, parses the input's inline SGR and OSC 8 sequences into that pen, and
/// writes each cell as the pen inherited onto the base. Text is written into
/// the borrowed target surface; dropping a painter has no side effects.
pub struct Painter<'s, S: TextSurface + ?Sized> {
    target: &'s mut S,
    /// Width measurement policy, snapshotted from the target at construction.
    mode: WidthMode,
    /// Whether East Asian Ambiguous characters are treated as wide,
    /// snapshotted from the target at construction.
    eaw_wide: bool,
}

impl<'s, S: TextSurface + ?Sized> Painter<'s, S> {
    /// Create a new painter over `target`.
    ///
    /// # Parameters
    ///
    /// * `target` — mutable surface receiving painted cells.
    ///
    /// # Returns
    ///
    /// A painter bound to `target`.
    ///
    /// # Errors and panics
    ///
    /// This constructor does not fail or intentionally panic.
    pub fn new(target: &'s mut S) -> Self {
        let mode = target.width_mode();
        let eaw_wide = target.eaw_wide();
        Self {
            target,
            mode,
            eaw_wide,
        }
    }

    /// Paint `s` with [`WrapMode::Truncate`], stamping `tail` on overflow.
    ///
    /// Falls back to a plain hard truncate when the tail is empty or cannot
    /// fit within `clip`.
    fn paint_truncate(
        &mut self,
        start: Position,
        clip: Rect,
        s: &str,
        tail_text: &str,
        tail_style: Style,
        style: Style,
    ) -> Position {
        if clip.is_empty() {
            return start;
        }
        let tail_w = string_width(tail_text.as_bytes(), self.mode, self.eaw_wide) as u16;
        let tail = if tail_w == 0 || tail_w > clip.width {
            None
        } else {
            Some(Tail {
                text: tail_text,
                style: &tail_style,
                width: tail_w,
            })
        };
        self.paint_inner(start, clip, s, WrapMode::Truncate, tail, style)
    }

    /// Stamp `tail` over the trailing `tail.width` columns of row `y`, ending
    /// at `clip`'s right edge, painted with the tail's starting style.
    fn paint_tail(&mut self, tail: Tail<'_>, clip: Rect, y: u16) {
        let tail_x = clip.right().saturating_sub(tail.width);
        let sub = Rect::new(tail_x, y, tail.width, 1).intersection(clip);
        self.paint_inner(
            Position::new(tail_x, y),
            sub,
            tail.text,
            WrapMode::Truncate,
            None,
            tail.style.clone(),
        );
    }

    fn paint(
        &mut self,
        start: Position,
        clip: Rect,
        s: &str,
        wrap: WrapMode,
        style: Style,
    ) -> Position {
        self.paint_inner(start, clip, s, wrap, None, style)
    }

    fn paint_inner(
        &mut self,
        start: Position,
        clip: Rect,
        s: &str,
        wrap: WrapMode,
        tail: Option<Tail<'_>>,
        base: Style,
    ) -> Position {
        if clip.is_empty() {
            return start;
        }
        let mut x = start.x;
        let mut y = start.y;
        // `pen` accumulates the inline SGR/OSC 8 state, starting empty; an
        // inline reset clears it, so the cells after a reset fall back to
        // `base`. `pending` is the cell currently being built: its origin,
        // text, and width. A trailing zero-width grapheme (a combining mark)
        // joins it instead of starting a new cell, so the cell is held until
        // the next non-zero-width token finalizes it.
        let mut pen = Style::default();
        let mut pending: Option<(u16, u16, String, u8)> = None;

        for tok in tokenize(s.as_bytes(), self.mode, self.eaw_wide) {
            // A zero-width grapheme appends to the pending cell without
            // finalizing it. Everything else finalizes the pending cell first,
            // writing it with the current style: the pen inherited onto base.
            if !matches!(tok, Token::Text { width: 0, .. })
                && let Some((px, py, content, w)) = pending.take()
                && clip.contains(Position::new(px, py))
            {
                let cell = if w == 2 {
                    Cell::wide(&content)
                } else {
                    Cell::narrow(&content)
                };
                self.target
                    .set_cell(Position::new(px, py), &cell.style(pen.inherit(&base)));
            }

            match tok {
                // SAFETY (all `from_utf8_unchecked`): the tokenizer cuts on
                // grapheme-cluster boundaries of a valid `&str`, so each slice
                // is valid UTF-8.
                Token::Text { text, width: 0 } => {
                    if let Some((_, _, ref mut content, _)) = pending {
                        content.push_str(unsafe { std::str::from_utf8_unchecked(text) });
                    }
                }
                Token::Text { text, width } => {
                    let g = unsafe { std::str::from_utf8_unchecked(text) };
                    let cw = width as u8;
                    if x + cw as u16 > clip.right() {
                        match wrap {
                            WrapMode::Truncate => {
                                if let Some(tail) = tail {
                                    self.paint_tail(tail, clip, y);
                                    return Position::new(clip.right(), y);
                                }
                                return Position::new(x, y);
                            }
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
                        read_style(Params::from_raw(body), &mut pen);
                    } else if let Some(body) = osc_body(seq)
                        && let Some((params, url)) = parse_hyperlink(body)
                    {
                        pen = pen.link(url, params);
                    }
                }
                Token::Control(0x0A) => {
                    y = y.saturating_add(1);
                    x = clip.left();
                    if y >= clip.bottom() {
                        return Position::new(x, y);
                    }
                }
                Token::Control(0x0D) => {
                    x = clip.left();
                }
                Token::Control(_) => {}
            }
        }

        // Finalize the last cell.
        if let Some((px, py, content, w)) = pending.take()
            && clip.contains(Position::new(px, py))
        {
            let cell = if w == 2 {
                Cell::wide(&content)
            } else {
                Cell::narrow(&content)
            };
            self.target
                .set_cell(Position::new(px, py), &cell.style(pen.inherit(&base)));
        }
        Position::new(x, y)
    }
}

impl<'s, S: TextSurface + ?Sized> Bounded for Painter<'s, S> {
    fn bounds(&self) -> Rect {
        self.target.bounds()
    }
}

impl<'s, S: TextSurface + ?Sized> Surface for Painter<'s, S> {
    fn cell(&self, pos: Position) -> Option<&Cell> {
        self.target.cell(pos)
    }
}

impl<'s, S: TextSurface + ?Sized> SurfaceMut for Painter<'s, S> {
    fn set_cell(&mut self, pos: Position, cell: &Cell) {
        self.target.set_cell(pos, cell);
    }

    fn cell_mut(&mut self, pos: Position) -> Option<&mut Cell> {
        self.target.cell_mut(pos)
    }

    fn insert_lines(&mut self, y: u16, count: u16, bounds_bottom: u16, fill: &Cell) {
        self.target.insert_lines(y, count, bounds_bottom, fill);
    }

    fn delete_lines(&mut self, y: u16, count: u16, bounds_bottom: u16, fill: &Cell) {
        self.target.delete_lines(y, count, bounds_bottom, fill);
    }

    fn insert_cells(&mut self, pos: Position, count: u16, bounds_right: u16, fill: &Cell) {
        self.target.insert_cells(pos, count, bounds_right, fill);
    }

    fn delete_cells(&mut self, pos: Position, count: u16, bounds_right: u16, fill: &Cell) {
        self.target.delete_cells(pos, count, bounds_right, fill);
    }
}

/// A [`Painter`] is itself a [`TextSurface`] whose `set_str` family recognizes
/// inline SGR and OSC 8 hyperlink sequences, updating the running
/// the running style as the input is parsed. This is the escape-aware
/// counterpart to the literal painting of the default [`TextSurface`] methods.
impl<'s, S: TextSurface + ?Sized> TextSurface for Painter<'s, S> {
    fn width_mode(&self) -> WidthMode {
        self.mode
    }

    fn eaw_wide(&self) -> bool {
        self.eaw_wide
    }

    /// Measure `s`, skipping recognized inline SGR and OSC 8 escape
    /// sequences so they contribute no width. This is the escape-aware
    /// counterpart to the literal default
    /// [`str_width`](crate::text::TextSurface::str_width).
    fn str_width(&self, s: &str) -> u16 {
        string_width(s.as_bytes(), self.mode, self.eaw_wide).min(u16::MAX as usize) as u16
    }

    /// Paint `s` starting at `pos`, clipped to the target bounds.
    ///
    /// The painter's running [`Style`] takes precedence and inherits any unset
    /// fields from `style`, so the running style carries across calls and
    /// `style` only fills in what it has not set. Inline SGR and OSC 8 sequences
    /// then update the running style as the input is processed. Newline advances
    /// to the next row at the bounds' left edge; carriage return returns to that
    /// left edge on the current row. Right-edge behavior is
    /// [`WrapMode::Truncate`].
    ///
    /// # Parameters
    ///
    /// * `pos` — starting cell position.
    /// * `s` — UTF-8 input string.
    /// * `style` — base style the running style inherits unset fields from.
    ///
    /// # Returns
    ///
    /// The cursor position immediately after the last written cell, or where
    /// painting stopped.
    ///
    /// # Errors and panics
    ///
    /// This method does not return errors and does not intentionally panic.
    fn set_str(&mut self, pos: impl Into<Position>, s: &str, style: impl Into<Style>) -> Position {
        let clip = self.target.bounds();
        self.paint(pos.into(), clip, s, WrapMode::default(), style.into())
    }

    /// Paint `s` starting at `pos` with explicit wrapping behavior.
    ///
    /// The target bounds are the clipping rectangle. [`WrapMode::Truncate`]
    /// stops at the right edge; [`WrapMode::Wrap`] continues on the next row
    /// at the bounds' left edge until the bottom edge is reached.
    ///
    /// # Parameters
    ///
    /// * `pos` — starting cell position.
    /// * `s` — UTF-8 input string.
    /// * `wrap` — right-edge behavior for non-zero-width clusters.
    /// * `style` — initial style for this call.
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
        let clip = self.target.bounds();
        self.paint(pos.into(), clip, s, wrap, style.into())
    }

    /// Paint `s` into `rect`, clipped to `rect ∩ target.bounds()`.
    ///
    /// Painting starts at `rect`'s top-left. Newline and carriage return use
    /// `rect`'s left edge as the return column. Right-edge behavior is
    /// [`WrapMode::Truncate`].
    ///
    /// # Parameters
    ///
    /// * `rect` — origin and clipping rectangle.
    /// * `s` — UTF-8 input string.
    /// * `style` — initial style for this call.
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
        let rect = rect.into();
        let clip = rect.intersection(self.target.bounds());
        self.paint(rect.position(), clip, s, WrapMode::default(), style.into())
    }

    /// Paint `s` into `rect` with explicit wrapping behavior.
    ///
    /// The clipping rectangle is `rect ∩ target.bounds()`. [`WrapMode::Wrap`]
    /// flows down inside `rect`; [`WrapMode::Truncate`] stops at `rect`'s
    /// right edge.
    ///
    /// # Parameters
    ///
    /// * `rect` — origin and clipping rectangle.
    /// * `s` — UTF-8 input string.
    /// * `wrap` — right-edge behavior for non-zero-width clusters.
    /// * `style` — initial style for this call.
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
        let rect = rect.into();
        let clip = rect.intersection(self.target.bounds());
        self.paint(rect.position(), clip, s, wrap, style.into())
    }

    /// Paint `s` starting at `pos`, truncating with a `tail` indicator.
    ///
    /// Text is painted across the target bounds. When a non-zero-width cluster
    /// would cross the right edge, painting stops and `tail` is stamped over
    /// the trailing columns so it ends exactly at the right edge. The tail
    /// appears only when the text actually overflows; text that fits is left
    /// untouched.
    ///
    /// `tail` is painted with `tail_style` as its starting style and may carry
    /// its own inline escape sequences, so it can be a single glyph (`"…"`), a
    /// word (`" more"`), or a multi-style span. If the tail is wider than the
    /// available space, it is dropped and the text is hard-truncated instead.
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
        tail_style: impl Into<Style>,
    ) -> Position {
        let clip = self.target.bounds();
        self.paint_truncate(
            pos.into(),
            clip,
            s,
            tail,
            tail_style.into(),
            Style::default(),
        )
    }

    /// Paint `s` inside `rect`, truncating with a `tail` indicator.
    ///
    /// This is the rectangular form of
    /// [`set_str_truncate`](Self::set_str_truncate): the clip rectangle is
    /// `rect ∩ target.bounds()`, and the tail is stamped at `rect`'s right
    /// edge when the text overflows it.
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
        tail_style: impl Into<Style>,
    ) -> Position {
        let rect = rect.into();
        let clip = rect.intersection(self.target.bounds());
        self.paint_truncate(
            rect.position(),
            clip,
            s,
            tail,
            tail_style.into(),
            Style::default(),
        )
    }
}

/// A truncation tail: borrowed indicator text, its starting style, and its
/// measured cell width. `Copy` so the overflow branch can hand it to
/// [`Painter::paint_tail`] without moving out of the `Option`.
#[derive(Clone, Copy)]
struct Tail<'a> {
    text: &'a str,
    style: &'a Style,
    width: u16,
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
    use crate::buffer::{Surface, TextBuffer};
    use crate::color::Color;
    use crate::style::AttrFlags;

    fn buf(width: u16, height: u16) -> TextBuffer {
        TextBuffer::new(width, height)
    }

    fn cell_at(b: &TextBuffer, x: u16, y: u16) -> Cell {
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
        let end =
            Painter::new(&mut b).set_str_wrap((0, 0), "abc", WrapMode::Truncate, Style::default());
        assert_eq!(end, Position::new(3, 0));
        assert_eq!(cell_at(&b, 0, 0).content(), "a");
        assert_eq!(cell_at(&b, 2, 0).content(), "c");
    }

    #[test]
    fn sgr_updates_style_mid_stream() {
        let mut b = buf(10, 1);
        let mut p = Painter::new(&mut b);
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
        Painter::new(&mut b).set_str_wrap(
            (0, 0),
            "\x1b[31mr",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(cell_at(&b, 0, 0).style.fg, Some(Color::Red));
    }

    #[test]
    fn osc8_toggles_link() {
        let mut b = buf(10, 1);
        Painter::new(&mut b).set_str_wrap(
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
        let mut p = Painter::new(&mut b);
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
        let end = Painter::new(&mut b).set_str_wrap(
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
        Painter::new(&mut b).set_str_wrap((0, 0), "abc\rXY", WrapMode::Truncate, Style::default());
        // 'X' overwrites 'a', 'Y' overwrites 'b', 'c' remains.
        assert_eq!(cell_at(&b, 0, 0).content(), "X");
        assert_eq!(cell_at(&b, 1, 0).content(), "Y");
        assert_eq!(cell_at(&b, 2, 0).content(), "c");
    }

    #[test]
    fn newline_past_bottom_returns() {
        let mut b = buf(5, 2);
        let end = Painter::new(&mut b).set_str_wrap(
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
        let end = Painter::new(&mut b).set_str_wrap(
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
        let end =
            Painter::new(&mut b).set_str_wrap((0, 0), "abcdef", WrapMode::Wrap, Style::default());
        assert_eq!(end, Position::new(3, 1));
        assert_eq!(cell_at(&b, 0, 0).content(), "a");
        assert_eq!(cell_at(&b, 2, 0).content(), "c");
        assert_eq!(cell_at(&b, 0, 1).content(), "d");
        assert_eq!(cell_at(&b, 2, 1).content(), "f");
    }

    #[test]
    fn rect_clip_and_origin() {
        let mut b = buf(10, 5);
        let end = Painter::new(&mut b).set_str_rect_wrap(
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
        Painter::new(&mut b).set_str_rect_wrap(
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
        Painter::new(&mut b).set_str_wrap(
            (0, 0),
            "a",
            WrapMode::Truncate,
            Style::default().bold().link("https://x", ""),
        );
        assert!(cell_at(&b, 0, 0).style.attrs.contains(AttrFlags::BOLD));
        assert_eq!(link_of(&cell_at(&b, 0, 0).style), Some(("https://x", "")));
        // Second call with `_with` and an empty style must reset.
        Painter::new(&mut b).set_str_wrap((1, 0), "b", WrapMode::Truncate, Style::default());
        assert!(!cell_at(&b, 1, 0).style.attrs.contains(AttrFlags::BOLD));
        assert!(cell_at(&b, 1, 0).style.link.is_none());
    }

    #[test]
    fn calls_start_from_their_own_base() {
        let mut b = buf(10, 1);
        let mut p = Painter::new(&mut b);
        // Inline SGR bolds within the first call only.
        p.set_str_wrap((0, 0), "\x1b[1ma", WrapMode::Truncate, Style::default());
        // The next call starts fresh from its base: no bold carries over.
        p.set_str_wrap((1, 0), "b", WrapMode::Truncate, Style::default());
        assert!(cell_at(&b, 0, 0).style.attrs.contains(AttrFlags::BOLD));
        assert!(!cell_at(&b, 1, 0).style.attrs.contains(AttrFlags::BOLD));
    }

    #[test]
    fn base_applies_and_inline_reset_returns_to_base() {
        let mut b = buf(10, 1);
        let base = Style::default().fg(Color::Red);
        // "a" gets the base red; the inline bold adds to "b"; the inline reset
        // clears only the inline state, so "c" falls back to the base red
        // rather than to a fully default style.
        Painter::new(&mut b).set_str_wrap((0, 0), "a\x1b[1mb\x1b[0mc", WrapMode::Truncate, base);
        let red = Some(Color::Red);
        assert_eq!(cell_at(&b, 0, 0).style.fg, red);
        assert!(!cell_at(&b, 0, 0).style.attrs.contains(AttrFlags::BOLD));
        assert_eq!(cell_at(&b, 1, 0).style.fg, red);
        assert!(cell_at(&b, 1, 0).style.attrs.contains(AttrFlags::BOLD));
        assert_eq!(cell_at(&b, 2, 0).style.fg, red);
        assert!(!cell_at(&b, 2, 0).style.attrs.contains(AttrFlags::BOLD));
    }

    #[test]
    fn position_and_rect_match_when_rect_covers_bounds() {
        let mut a = buf(5, 2);
        let mut b = buf(5, 2);
        let e1 =
            Painter::new(&mut a).set_str_wrap((0, 0), "abc", WrapMode::Truncate, Style::default());
        let e2 = Painter::new(&mut b).set_str_rect_wrap(
            Rect::new(0, 0, 5, 2),
            "abc",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(e1, e2);
        assert_eq!(cell_at(&a, 2, 0).content(), cell_at(&b, 2, 0).content());
    }

    fn row(b: &TextBuffer, y: u16) -> String {
        (0..b.width())
            .map(|x| cell_at(b, x, y).content().to_string())
            .collect()
    }

    #[test]
    fn truncate_tail_not_shown_when_text_fits() {
        let mut b = buf(5, 1);
        let end = Painter::new(&mut b).set_str_truncate((0, 0), "abc", "…", Style::default());
        // "abc" fits in 5 columns, so no tail is stamped.
        assert_eq!(row(&b, 0), "abc  ");
        assert_eq!(end, Position::new(3, 0));
    }

    #[test]
    fn truncate_single_cell_tail_on_overflow() {
        let mut b = buf(5, 1);
        let end = Painter::new(&mut b).set_str_truncate((0, 0), "abcdefgh", "…", Style::default());
        // 5 columns: 4 text columns + the 1-wide tail at the right edge.
        assert_eq!(row(&b, 0), "abcd…");
        assert_eq!(end, Position::new(5, 0));
    }

    #[test]
    fn truncate_multi_cell_tail_reserves_its_width() {
        let mut b = buf(8, 1);
        Painter::new(&mut b).set_str_truncate((0, 0), "abcdefghij", " more", Style::default());
        // 8 columns: 3 text columns + the 5-wide " more" tail.
        assert_eq!(row(&b, 0), "abc more");
    }

    #[test]
    fn truncate_tail_carries_its_style() {
        let mut b = buf(5, 1);
        Painter::new(&mut b).set_str_truncate(
            (0, 0),
            "abcdefgh",
            "…",
            Style::default().fg(Color::Red),
        );
        // The tail cell gets the supplied base style.
        assert_eq!(cell_at(&b, 4, 0).style.fg, Some(Color::Red));
    }

    #[test]
    fn truncate_tail_inline_escapes_apply() {
        let mut b = buf(5, 1);
        Painter::new(&mut b).set_str_truncate((0, 0), "abcdefgh", "\x1b[1m…", Style::default());
        // The tail's inline SGR bolds it even though the base style is empty.
        assert!(cell_at(&b, 4, 0).style.attrs.contains(AttrFlags::BOLD));
    }

    #[test]
    fn truncate_wide_tail_too_big_hard_truncates() {
        let mut b = buf(3, 1);
        let end =
            Painter::new(&mut b).set_str_truncate((0, 0), "abcdef", " more", Style::default());
        // " more" is 5 wide but the clip is only 3, so it is dropped and the
        // text is hard-truncated with no tail.
        assert_eq!(row(&b, 0), "abc");
        assert_eq!(end, Position::new(3, 0));
    }

    #[test]
    fn truncate_tail_overwrites_split_wide_cell() {
        // A wide cluster sits where the tail's left edge lands; stamping the
        // tail must blank the dangling wide primary.
        let mut b = buf(5, 1);
        Painter::new(&mut b).set_str_truncate((0, 0), "ab中def", "…", Style::default());
        // "ab" + wide "中" fills columns 0..4; the tail overwrites column 4,
        // which is the continuation of "中", so the wide primary at 3 must be
        // blanked rather than left dangling.
        assert_eq!(cell_at(&b, 4, 0).content(), "…");
        assert!(!cell_at(&b, 3, 0).is_wide());
    }
}
