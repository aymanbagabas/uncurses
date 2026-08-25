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
use crate::cell::Style as CellStyle;
use crate::cell::{Cell, Content, Kind};
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
        tail_style: CellStyle,
        style: CellStyle,
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
        style: CellStyle,
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
        base: CellStyle,
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
        // `pen` accumulates the inline SGR/OSC 8 state, starting empty; an
        // inline reset clears it, so the cells after a reset fall back to
        // `base`. `pending` is the cell currently being built: its origin,
        // text, and width. A trailing zero-width grapheme (a combining mark)
        // joins it instead of starting a new cell, so the cell is held until
        // the next non-zero-width token finalizes it.
        let mut pen = Style::default();
        // OSC 8 state is tracked separately from SGR: hyperlinks live on the
        // cell, not on `Style`. The pen's link opens the span, and inline
        // OSC 8 in the text can retarget or close it.
        let mut pen_link = base.link.clone();
        let mut pending: Option<(u16, u16, String, u8)> = None;
        // Truncation is per row: once a row overflows, clusters are dropped
        // until `\n` or `\r` puts the cursor back inside the clip. Escapes
        // still run, so the pen carries over to the next row.
        let mut truncated = false;

        for tok in tokenize(s.as_bytes(), self.mode, self.eaw_wide) {
            // A zero-width grapheme appends to the pending cell without
            // finalizing it. Everything else finalizes the pending cell first,
            // writing it with the current style: the pen inherited onto base.
            if !matches!(tok, Token::Text { width: 0, .. })
                && let Some((px, py, content, w)) = pending.take()
                && clip.contains(Position::new(px, py))
            {
                // The tokenizer measured this cluster, so its footprint is
                // authoritative rather than `Cell::new`'s own measurement.
                let cell = Cell {
                    content: Content::from(content.as_str()),
                    style: CellStyle {
                        style: pen.inherit(base.style),
                        link: pen_link.clone(),
                    },
                    kind: if w == 2 { Kind::Wide } else { Kind::Narrow },
                };
                self.target.set_cell(Position::new(px, py), &cell);
            }

            match tok {
                // SAFETY (all `from_utf8_unchecked`): the tokenizer cuts on
                // grapheme-cluster boundaries of a valid `&str`, so each slice
                // is valid UTF-8. Checked under `debug_assert!` for the same
                // reason as `ansi::wrap::bs` - the invariant is the tokenizer's
                // to keep, and when it stopped keeping it this was UB.
                Token::Text { text, width: 0 } => {
                    if let Some((_, _, ref mut content, _)) = pending {
                        debug_assert!(std::str::from_utf8(text).is_ok());
                        content.push_str(unsafe { std::str::from_utf8_unchecked(text) });
                    }
                }
                Token::Text { text, width } => {
                    debug_assert!(std::str::from_utf8(text).is_ok());
                    if truncated {
                        continue;
                    }
                    let g = unsafe { std::str::from_utf8_unchecked(text) };
                    let cw = width as u8;
                    if x + cw as u16 > clip.right() {
                        match wrap {
                            WrapMode::Truncate => {
                                if let Some(tail) = tail {
                                    self.paint_tail(tail, clip, y);
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
                        pen_link = (!url.is_empty()).then(|| {
                            std::sync::Arc::new(crate::style::Link {
                                url: url.to_owned(),
                                params: params.to_owned(),
                            })
                        });
                    }
                }
                Token::Control(0x0A) => {
                    y = y.saturating_add(1);
                    x = clip.left();
                    truncated = false;
                    if y >= clip.bottom() {
                        return Position::new(x, y);
                    }
                }
                Token::Control(0x0D) => {
                    x = clip.left();
                    truncated = false;
                }
                Token::Control(_) => {}
            }
        }

        // Finalize the last cell.
        if let Some((px, py, content, w)) = pending.take()
            && clip.contains(Position::new(px, py))
        {
            let cell = Cell {
                content: Content::from(content.as_str()),
                style: CellStyle {
                    style: pen.inherit(base.style),
                    link: pen_link.clone(),
                },
                kind: if w == 2 { Kind::Wide } else { Kind::Narrow },
            };
            self.target.set_cell(Position::new(px, py), &cell);
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
    fn cell(&self, pos: Position) -> Option<Cell> {
        self.target.cell(pos)
    }
}

impl<'s, S: TextSurface + ?Sized> SurfaceMut for Painter<'s, S> {
    fn set_cell(&mut self, pos: Position, cell: &Cell) {
        self.target.set_cell(pos, cell);
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
    fn set_str(
        &mut self,
        pos: impl Into<Position>,
        s: &str,
        style: impl Into<CellStyle>,
    ) -> Position {
        let clip = self.target.bounds();
        self.paint(pos.into(), clip, s, WrapMode::default(), style.into())
    }

    /// Paint `s` starting at `pos` with explicit wrapping behavior.
    ///
    /// The target bounds are the clipping rectangle. [`WrapMode::Truncate`]
    /// drops the rest of the row at the right edge and resumes on the next
    /// row; [`WrapMode::Wrap`] continues on the next row
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
        style: impl Into<CellStyle>,
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
        style: impl Into<CellStyle>,
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
        style: impl Into<CellStyle>,
    ) -> Position {
        let rect = rect.into();
        let clip = rect.intersection(self.target.bounds());
        self.paint(rect.position(), clip, s, wrap, style.into())
    }

    /// Paint `s` starting at `pos`, truncating with a `tail` indicator.
    ///
    /// Text is painted across the target bounds. When a non-zero-width cluster
    /// would cross the right edge, the rest of that row is dropped and `tail`
    /// is stamped over its trailing columns so it ends exactly at the right
    /// edge. Painting resumes on the next row if the text continues past a
    /// newline, so a multi-line `s` can stamp one tail per overflowing row.
    /// The tail appears only on rows that actually overflow; a row that fits
    /// is left untouched.
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
        tail_style: impl Into<CellStyle>,
    ) -> Position {
        let clip = self.target.bounds();
        self.paint_truncate(
            pos.into(),
            clip,
            s,
            tail,
            tail_style.into(),
            CellStyle::default(),
        )
    }

    /// Paint `s` inside `rect`, truncating with a `tail` indicator.
    ///
    /// This is the rectangular form of
    /// [`set_str_truncate`](Self::set_str_truncate): the clip rectangle is
    /// `rect ∩ target.bounds()`, and a tail is stamped at `rect`'s right
    /// edge on each row that overflows it.
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
        let rect = rect.into();
        let clip = rect.intersection(self.target.bounds());
        self.paint_truncate(
            rect.position(),
            clip,
            s,
            tail,
            tail_style.into(),
            CellStyle::default(),
        )
    }
}

/// A truncation tail: borrowed indicator text, its starting style, and its
/// measured cell width. `Copy` so the overflow branch can hand it to
/// [`Painter::paint_tail`] without moving out of the `Option`.
#[derive(Clone, Copy)]
struct Tail<'a> {
    text: &'a str,
    style: &'a CellStyle,
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
        b.cell(Position::new(x, y)).unwrap()
    }

    fn link_of(c: &Cell) -> Option<(&str, &str)> {
        c.style
            .link
            .as_ref()
            .map(|l| (l.url.as_str(), l.params.as_str()))
    }

    #[test]
    fn plain_text() {
        let mut b = buf(10, 1);
        let end =
            Painter::new(&mut b).set_str_wrap((0, 0), "abc", WrapMode::Truncate, Style::default());
        assert_eq!(end, Position::new(3, 0));
        assert_eq!(cell_at(&b, 0, 0).content.char(), Some('a'));
        assert_eq!(cell_at(&b, 2, 0).content.char(), Some('c'));
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
        assert!(!c0.style.style.attrs.contains(AttrFlags::BOLD));
        assert!(c1.style.style.attrs.contains(AttrFlags::BOLD));
        assert!(!c2.style.style.attrs.contains(AttrFlags::BOLD));
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
        assert_eq!(cell_at(&b, 0, 0).style.style.fg, Some(Color::Red));
    }

    /// The painter's `from_utf8_unchecked` on a text token, driven by the
    /// sequences that used to break the tokenizer's promise.
    ///
    /// Every other painter test uses ASCII-only escape payloads, so reverting
    /// the scanner fix left them all green while the painter took ill-formed
    /// bytes on trust. `\u{2705}` is `E2 9C 85` and carries an 8-bit ST byte;
    /// `\u{9c}` and `\u{9d}` encode the ST and OSC bytes themselves.
    #[test]
    fn utf8_payloads_in_sequences_paint_the_text_after_them() {
        for input in [
            "\x1b]0;\u{2705}\x07ab",
            "\x1b]0;x\u{9c}y\x07ab",
            "\x1b]0;x\u{9d}y\x07ab",
            "\x1bP1$r\u{2705}\x1b\\ab",
            "\x1b_G\u{2705}\x1b\\ab",
            "\x1b#\u{2705}ab",
        ] {
            let mut b = buf(10, 1);
            let end = Painter::new(&mut b).set_str_wrap(
                (0, 0),
                input,
                WrapMode::Truncate,
                Style::default(),
            );
            assert_eq!(
                end,
                Position::new(2, 0),
                "{input:?} painted the wrong width"
            );
            assert_eq!(cell_at(&b, 0, 0).content.char(), Some('a'), "{input:?}");
            assert_eq!(cell_at(&b, 1, 0).content.char(), Some('b'), "{input:?}");
        }
    }

    /// An OSC 8 whose URL carries a C1 continuation byte still yields a link
    /// with the whole URL, and the styled text after it.
    #[test]
    fn osc8_with_a_utf8_url() {
        let mut b = buf(10, 1);
        Painter::new(&mut b).set_str_wrap(
            (0, 0),
            "\x1b]8;;https://x/\u{2705}\x1b\\a\x1b]8;;\x1b\\b",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(
            link_of(&cell_at(&b, 0, 0)),
            Some(("https://x/\u{2705}", ""))
        );
        assert_eq!(cell_at(&b, 0, 0).content.char(), Some('a'));
        assert!(cell_at(&b, 1, 0).style.link.is_none());
        assert_eq!(cell_at(&b, 1, 0).content.char(), Some('b'));
    }

    #[test]
    fn pen_link_applies_to_every_painted_cell() {
        let mut b = buf(10, 1);
        Painter::new(&mut b).set_str_wrap(
            (0, 0),
            "hi",
            WrapMode::Truncate,
            CellStyle::new().link("https://x", "id=7"),
        );
        assert_eq!(link_of(&cell_at(&b, 0, 0)), Some(("https://x", "id=7")));
        assert_eq!(link_of(&cell_at(&b, 1, 0)), Some(("https://x", "id=7")));
    }

    #[test]
    fn inline_osc8_overrides_the_pen_link() {
        let mut b = buf(10, 1);
        Painter::new(&mut b).set_str_wrap(
            (0, 0),
            "a\x1b]8;;\x1b\\b",
            WrapMode::Truncate,
            CellStyle::new().link("https://x", ""),
        );
        assert_eq!(link_of(&cell_at(&b, 0, 0)), Some(("https://x", "")));
        assert!(cell_at(&b, 1, 0).style.link.is_none());
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
        assert_eq!(link_of(&cell_at(&b, 0, 0)), Some(("https://x", "")));
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
        assert_eq!(link_of(&cell_at(&b, 0, 0)), Some(("https://x", "")));
        assert_eq!(link_of(&cell_at(&b, 1, 0)), Some(("https://x", "")));
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
        assert_eq!(cell_at(&b, 0, 0).content.char(), Some('a'));
        assert_eq!(cell_at(&b, 1, 0).content.char(), Some('b'));
        assert_eq!(cell_at(&b, 0, 1).content.char(), Some('c'));
        assert_eq!(cell_at(&b, 1, 1).content.char(), Some('d'));
        assert_eq!(end, Position::new(2, 1));
    }

    #[test]
    fn cr_returns_to_left() {
        let mut b = buf(5, 1);
        Painter::new(&mut b).set_str_wrap((0, 0), "abc\rXY", WrapMode::Truncate, Style::default());
        // 'X' overwrites 'a', 'Y' overwrites 'b', 'c' remains.
        assert_eq!(cell_at(&b, 0, 0).content.char(), Some('X'));
        assert_eq!(cell_at(&b, 1, 0).content.char(), Some('Y'));
        assert_eq!(cell_at(&b, 2, 0).content.char(), Some('c'));
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
        assert_eq!(cell_at(&b, 0, 0).content.char(), Some('a'));
        assert_eq!(cell_at(&b, 0, 1).content.char(), Some('b'));
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
        assert_eq!(cell_at(&b, 0, 0).content.char(), Some('a'));
        assert_eq!(cell_at(&b, 2, 0).content.char(), Some('c'));
    }

    #[test]
    fn truncate_resumes_on_next_row() {
        let mut b = buf(3, 2);
        let end = Painter::new(&mut b).set_str_wrap(
            (0, 0),
            "abcdef\nxy",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(cell_at(&b, 2, 0).content.char(), Some('c'));
        assert_eq!(cell_at(&b, 0, 1).content.char(), Some('x'));
        assert_eq!(cell_at(&b, 1, 1).content.char(), Some('y'));
        assert_eq!(end, Position::new(2, 1));
    }

    #[test]
    fn truncate_tail_stamped_per_row() {
        let mut b = buf(4, 2);
        Painter::new(&mut b).set_str_truncate((0, 0), "abcdef\nghijkl", "…", Style::default());
        assert_eq!(cell_at(&b, 3, 0).content.char(), Some('…'));
        assert_eq!(cell_at(&b, 0, 1).content.char(), Some('g'));
        assert_eq!(cell_at(&b, 3, 1).content.char(), Some('…'));
    }

    #[test]
    fn literal_truncate_resumes_on_next_row() {
        let mut b = buf(3, 2);
        b.set_str((0, 0), "abcdef\nxy", Style::default());
        assert_eq!(cell_at(&b, 2, 0).content.char(), Some('c'));
        assert_eq!(cell_at(&b, 0, 1).content.char(), Some('x'));
        assert_eq!(cell_at(&b, 1, 1).content.char(), Some('y'));
    }

    #[test]
    fn cr_clears_truncation_for_the_row() {
        let mut b = buf(3, 1);
        Painter::new(&mut b).set_str_wrap(
            (0, 0),
            "abcdef\rXY",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(cell_at(&b, 0, 0).content.char(), Some('X'));
        assert_eq!(cell_at(&b, 1, 0).content.char(), Some('Y'));
        assert_eq!(cell_at(&b, 2, 0).content.char(), Some('c'));
    }

    #[test]
    fn overflow_drops_rest_of_row_without_backfill() {
        let mut b = buf(3, 1);
        let end = Painter::new(&mut b).set_str_wrap(
            (0, 0),
            "ab中c",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(cell_at(&b, 0, 0).content.char(), Some('a'));
        assert_eq!(cell_at(&b, 1, 0).content.char(), Some('b'));
        // "中" needs two columns and only one is left; "c" must not slot into
        // the gap ahead of it.
        assert_eq!(cell_at(&b, 2, 0).content.char(), Some(' '));
        assert_eq!(end, Position::new(2, 0));
    }

    #[test]
    fn literal_crlf_breaks_the_line() {
        // Extended grapheme segmentation joins CR LF into one zero-width
        // cluster, so it has to be matched explicitly to break the line.
        let mut b = buf(3, 2);
        b.set_str((0, 0), "abcdef\r\nxy", Style::default());
        assert_eq!(cell_at(&b, 2, 0).content.char(), Some('c'));
        assert_eq!(cell_at(&b, 0, 1).content.char(), Some('x'));
        assert_eq!(cell_at(&b, 1, 1).content.char(), Some('y'));
    }

    #[test]
    fn crlf_breaks_the_line() {
        let mut b = buf(3, 2);
        Painter::new(&mut b).set_str_wrap(
            (0, 0),
            "abcdef\r\nxy",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(cell_at(&b, 2, 0).content.char(), Some('c'));
        assert_eq!(cell_at(&b, 0, 1).content.char(), Some('x'));
        assert_eq!(cell_at(&b, 1, 1).content.char(), Some('y'));
    }

    #[test]
    fn escapes_still_apply_across_a_truncated_row() {
        // The pen keeps advancing through the dropped part of the row, so a
        // style opened there lands on the next row.
        let mut b = buf(2, 2);
        Painter::new(&mut b).set_str_wrap(
            (0, 0),
            "ab\x1b[1mcd\x1b]8;;https://x\x1b\\\nz",
            WrapMode::Truncate,
            Style::default(),
        );
        assert!(
            !cell_at(&b, 0, 0)
                .style
                .style
                .attrs
                .contains(AttrFlags::BOLD)
        );
        let z = cell_at(&b, 0, 1);
        assert_eq!(z.content.char(), Some('z'));
        assert!(z.style.style.attrs.contains(AttrFlags::BOLD));
        assert_eq!(link_of(&z), Some(("https://x", "")));
    }

    #[test]
    fn start_below_clip_paints_nothing() {
        let mut b = buf(3, 2);
        let end = Painter::new(&mut b).set_str_wrap(
            (0, 5),
            "abcdef\nghi",
            WrapMode::Truncate,
            Style::default(),
        );
        assert_eq!(end, Position::new(0, 5));
        for y in 0..2 {
            for x in 0..3 {
                assert_eq!(cell_at(&b, x, y).content.char(), Some(' '));
            }
        }
    }

    #[test]
    fn literal_start_below_clip_paints_nothing() {
        let mut b = buf(3, 2);
        let end = b.set_str((0, 5), "abcdef\nghi", Style::default());
        assert_eq!(end, Position::new(0, 5));
        assert_eq!(cell_at(&b, 0, 0).content.char(), Some(' '));
        assert_eq!(cell_at(&b, 0, 1).content.char(), Some(' '));
    }

    #[test]
    fn wrap_breaks_on_crlf() {
        // The CRLF fix is shared with WrapMode::Wrap: a joined cluster has to
        // break the line there too, not read as zero-width filler.
        let mut b = buf(4, 3);
        Painter::new(&mut b).set_str_wrap((0, 0), "ab\r\ncd", WrapMode::Wrap, Style::default());
        assert_eq!(cell_at(&b, 0, 0).content.char(), Some('a'));
        assert_eq!(cell_at(&b, 1, 0).content.char(), Some('b'));
        assert_eq!(cell_at(&b, 0, 1).content.char(), Some('c'));
        assert_eq!(cell_at(&b, 1, 1).content.char(), Some('d'));
    }

    #[test]
    fn literal_wrap_breaks_on_crlf() {
        let mut b = buf(4, 3);
        b.set_str_wrap((0, 0), "ab\r\ncd", WrapMode::Wrap, Style::default());
        assert_eq!(cell_at(&b, 0, 0).content.char(), Some('a'));
        assert_eq!(cell_at(&b, 1, 0).content.char(), Some('b'));
        assert_eq!(cell_at(&b, 0, 1).content.char(), Some('c'));
        assert_eq!(cell_at(&b, 1, 1).content.char(), Some('d'));
    }

    #[test]
    fn wrap_at_right_edge() {
        let mut b = buf(3, 3);
        let end =
            Painter::new(&mut b).set_str_wrap((0, 0), "abcdef", WrapMode::Wrap, Style::default());
        assert_eq!(end, Position::new(3, 1));
        assert_eq!(cell_at(&b, 0, 0).content.char(), Some('a'));
        assert_eq!(cell_at(&b, 2, 0).content.char(), Some('c'));
        assert_eq!(cell_at(&b, 0, 1).content.char(), Some('d'));
        assert_eq!(cell_at(&b, 2, 1).content.char(), Some('f'));
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
        assert_eq!(cell_at(&b, 2, 1).content.char(), Some('a'));
        assert_eq!(cell_at(&b, 4, 1).content.char(), Some('c'));
        assert_eq!(cell_at(&b, 2, 2).content.char(), Some('d'));
        assert_eq!(cell_at(&b, 4, 2).content.char(), Some('f'));
        // Outside the rect must remain blank.
        assert_eq!(cell_at(&b, 0, 0).content.char(), Some(' '));
        assert_eq!(cell_at(&b, 5, 1).content.char(), Some(' '));
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
        assert_eq!(cell_at(&b, 2, 1).content.char(), Some('a'));
        assert_eq!(cell_at(&b, 3, 1).content.char(), Some('b'));
        // Newline returns x to rect.left() = 2, not to 0.
        assert_eq!(cell_at(&b, 2, 2).content.char(), Some('c'));
        assert_eq!(cell_at(&b, 3, 2).content.char(), Some('d'));
        assert_eq!(cell_at(&b, 0, 2).content.char(), Some(' '));
    }

    #[test]
    fn with_resets_style_and_link() {
        let mut b = buf(10, 1);
        // First call: paint with bold plus a link carried by the pen.
        Painter::new(&mut b).set_str_wrap(
            (0, 0),
            "a",
            WrapMode::Truncate,
            CellStyle::from(Style::default().bold()).link("https://x", ""),
        );
        assert!(
            cell_at(&b, 0, 0)
                .style
                .style
                .attrs
                .contains(AttrFlags::BOLD)
        );
        assert_eq!(link_of(&cell_at(&b, 0, 0)), Some(("https://x", "")));
        // A second call starts from a clean pen: no bold, no open link.
        Painter::new(&mut b).set_str_wrap((1, 0), "b", WrapMode::Truncate, Style::default());
        assert!(
            !cell_at(&b, 1, 0)
                .style
                .style
                .attrs
                .contains(AttrFlags::BOLD)
        );
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
        assert!(
            cell_at(&b, 0, 0)
                .style
                .style
                .attrs
                .contains(AttrFlags::BOLD)
        );
        assert!(
            !cell_at(&b, 1, 0)
                .style
                .style
                .attrs
                .contains(AttrFlags::BOLD)
        );
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
        assert_eq!(cell_at(&b, 0, 0).style.style.fg, red);
        assert!(
            !cell_at(&b, 0, 0)
                .style
                .style
                .attrs
                .contains(AttrFlags::BOLD)
        );
        assert_eq!(cell_at(&b, 1, 0).style.style.fg, red);
        assert!(
            cell_at(&b, 1, 0)
                .style
                .style
                .attrs
                .contains(AttrFlags::BOLD)
        );
        assert_eq!(cell_at(&b, 2, 0).style.style.fg, red);
        assert!(
            !cell_at(&b, 2, 0)
                .style
                .style
                .attrs
                .contains(AttrFlags::BOLD)
        );
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
        assert_eq!(cell_at(&a, 2, 0).to_string(), cell_at(&b, 2, 0).to_string());
    }

    fn row(b: &TextBuffer, y: u16) -> String {
        (0..b.width())
            .map(|x| cell_at(b, x, y).to_string())
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
        assert_eq!(cell_at(&b, 4, 0).style.style.fg, Some(Color::Red));
    }

    #[test]
    fn truncate_tail_inline_escapes_apply() {
        let mut b = buf(5, 1);
        Painter::new(&mut b).set_str_truncate((0, 0), "abcdefgh", "\x1b[1m…", Style::default());
        // The tail's inline SGR bolds it even though the base style is empty.
        assert!(
            cell_at(&b, 4, 0)
                .style
                .style
                .attrs
                .contains(AttrFlags::BOLD)
        );
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
        assert_eq!(cell_at(&b, 4, 0).content.char(), Some('…'));
        assert!(!cell_at(&b, 3, 0).is_wide());
    }
}
