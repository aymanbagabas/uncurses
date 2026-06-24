//! SGR (Select Graphic Rendition) sequence generation for styles.
//!
//! A style is rendered as a single `CSI … m` sequence with all parameters
//! joined by `;`. Foreground and background extended colors use the common
//! semicolon forms (`38;5;n`, `48;2;r;g;b`). Underline style and underline
//! color use colon subparameters (`4:2`, `58:2::r:g:b`) where required.
//!
//! ```text
//! ESC [ attr ; underline ; fg ; bg ; underline-color   m
//! └─┬─┘ └──────────────── parameters ───────────────┘ └┬┘
//!  CSI                                                 final
//! ```

use std::io::{self, Write};

use super::{AttrFlags, Style, UnderlineStyle};
use crate::color::Color;

/// SGR reset sequence (`ESC [ m`).
///
/// This clears SGR attributes and colors. It does not close OSC 8 hyperlinks.
pub(crate) const RESET: &[u8] = b"\x1b[m";

/// Fixed-capacity stack byte collector for short escape sequences.
///
/// Holds up to `N` bytes inline and ships the whole buffer with one
/// `write_all`. Used on the per-cell pen-update hot path so each styled
/// cell costs a single writer dispatch rather than three.
///
/// The collector is intentionally dumb: it does *not* know about CSI
/// introducers, separators, intermediates, or final bytes. Callers
/// stage every byte themselves (`extend_from_slice(b"\x1b[")` ...
/// `push(b'm')` ... `flush(w)`).
pub(super) struct SmallSeq<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> SmallSeq<N> {
    #[inline]
    pub(super) const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    #[inline]
    pub(super) fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub(super) fn push(&mut self, b: u8) {
        self.buf[self.len] = b;
        self.len += 1;
    }

    #[inline]
    pub(super) fn extend_from_slice(&mut self, s: &[u8]) {
        let end = self.len + s.len();
        self.buf[self.len..end].copy_from_slice(s);
        self.len = end;
    }

    #[inline]
    pub(super) fn flush<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(&self.buf[..self.len])
    }
}

/// SGR-shaped buffer big enough for the worst-case style emission:
/// `Style::default()` → fully decorated truecolor fg/bg/underline-color
/// plus all attrs and underline style, with introducer and final byte.
pub(super) type SgrSeq = SmallSeq<100>;

/// Push a `;` separator if any parameter bytes have already been
/// written past `body_start` (the offset at which parameters begin).
#[inline]
pub(super) fn push_sep<const N: usize>(buf: &mut SmallSeq<N>, body_start: usize) {
    if buf.len() > body_start {
        buf.push(b';');
    }
}

/// Write a style as one SGR `CSI … m` sequence.
///
/// All present attributes, underline shape, foreground color, background color,
/// and underline color are joined into a single parameter list. If the style
/// has no SGR-relevant state, this writes nothing at all. Hyperlinks are not
/// emitted here; the [`Display`](std::fmt::Display) implementation for
/// [`Style`](super::Style) handles OSC 8 wrapping around this sequence.
///
/// Returns any I/O error from `w` and does not panic.
pub(crate) fn write_style<W: Write>(w: &mut W, style: &Style) -> io::Result<()> {
    let mut seq = SgrSeq::new();
    seq.extend_from_slice(b"\x1b[");
    let body_start = seq.len();
    push_attr_params(&mut seq, body_start, style.attrs);
    push_underline_param(&mut seq, body_start, style.underline);
    if let Some(fg) = style.fg {
        push_sep(&mut seq, body_start);
        push_fg_params(&mut seq, fg);
    }
    if let Some(bg) = style.bg {
        push_sep(&mut seq, body_start);
        push_bg_params(&mut seq, bg);
    }
    if let Some(ul) = style.underline_color {
        push_sep(&mut seq, body_start);
        push_underline_color_params(&mut seq, ul);
    }
    if seq.len() == body_start {
        return Ok(());
    }
    seq.push(b'm');
    seq.flush(w)
}

fn push_u8<const N: usize>(buf: &mut SmallSeq<N>, n: u8) {
    if n >= 100 {
        buf.push(b'0' + (n / 100));
        buf.push(b'0' + ((n / 10) % 10));
        buf.push(b'0' + (n % 10));
    } else if n >= 10 {
        buf.push(b'0' + (n / 10));
        buf.push(b'0' + (n % 10));
    } else {
        buf.push(b'0' + n);
    }
}

fn push_attr_params<const N: usize>(buf: &mut SmallSeq<N>, body_start: usize, attrs: AttrFlags) {
    let codes: [(AttrFlags, &[u8]); 8] = [
        (AttrFlags::BOLD, b"1"),
        (AttrFlags::FAINT, b"2"),
        (AttrFlags::ITALIC, b"3"),
        (AttrFlags::SLOW_BLINK, b"5"),
        (AttrFlags::RAPID_BLINK, b"6"),
        (AttrFlags::REVERSE, b"7"),
        (AttrFlags::CONCEAL, b"8"),
        (AttrFlags::STRIKETHROUGH, b"9"),
    ];
    for (flag, code) in codes {
        if attrs.contains(flag) {
            push_sep(buf, body_start);
            buf.extend_from_slice(code);
        }
    }
}

fn push_underline_param<const N: usize>(
    buf: &mut SmallSeq<N>,
    body_start: usize,
    ul: UnderlineStyle,
) {
    let token: &[u8] = match ul {
        UnderlineStyle::None => return,
        UnderlineStyle::Single => b"4",
        UnderlineStyle::Double => b"4:2",
        UnderlineStyle::Curly => b"4:3",
        UnderlineStyle::Dotted => b"4:4",
        UnderlineStyle::Dashed => b"4:5",
    };
    push_sep(buf, body_start);
    buf.extend_from_slice(token);
}

pub(super) fn push_fg_params<const N: usize>(buf: &mut SmallSeq<N>, color: Color) {
    if let Some(i) = color.named_index() {
        let code = if i >= 8 { 90 + i - 8 } else { 30 + i };
        push_u8(buf, code);
        return;
    }
    match color {
        Color::Indexed(idx) => {
            buf.extend_from_slice(b"38;5;");
            push_u8(buf, idx);
        }
        Color::Rgb(r, g, b) => {
            buf.extend_from_slice(b"38;2;");
            push_u8(buf, r);
            buf.push(b';');
            push_u8(buf, g);
            buf.push(b';');
            push_u8(buf, b);
        }
        // Named colors are handled above via `named_index`.
        _ => {}
    }
}

pub(super) fn push_bg_params<const N: usize>(buf: &mut SmallSeq<N>, color: Color) {
    if let Some(i) = color.named_index() {
        let code = if i >= 8 { 100 + i - 8 } else { 40 + i };
        push_u8(buf, code);
        return;
    }
    match color {
        Color::Indexed(idx) => {
            buf.extend_from_slice(b"48;5;");
            push_u8(buf, idx);
        }
        Color::Rgb(r, g, b) => {
            buf.extend_from_slice(b"48;2;");
            push_u8(buf, r);
            buf.push(b';');
            push_u8(buf, g);
            buf.push(b';');
            push_u8(buf, b);
        }
        // Named colors are handled above via `named_index`.
        _ => {}
    }
}

pub(super) fn push_underline_color_params<const N: usize>(buf: &mut SmallSeq<N>, color: Color) {
    // ITU T.416 colon subparam form: terminals that don't recognise the
    // `58` extension treat the whole `58:…` token as a single unknown
    // param and skip it. The legacy semicolon form `58;5;n` would let
    // unsupporting parsers re-interpret `5`/`n` as standalone SGR
    // codes, leaking spurious attributes (slow blink, etc.).
    match color {
        Color::Indexed(idx) => {
            buf.extend_from_slice(b"58:5:");
            push_u8(buf, idx);
        }
        Color::Rgb(r, g, b) => {
            buf.extend_from_slice(b"58:2::");
            push_u8(buf, r);
            buf.push(b':');
            push_u8(buf, g);
            buf.push(b':');
            push_u8(buf, b);
        }
        // Named colors: emit their `0..=15` palette index in the `58:5:n` form.
        named => {
            buf.extend_from_slice(b"58:5:");
            push_u8(buf, named.named_index().unwrap_or(0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;

    #[test]
    fn test_write_empty_style_emits_nothing() {
        let mut buf = Vec::new();
        write_style(&mut buf, &Style::default()).unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn test_write_bold() {
        let mut buf = Vec::new();
        write_style(&mut buf, &Style::default().bold()).unwrap();
        assert_eq!(buf, b"\x1b[1m");
    }

    #[test]
    fn test_write_combined_single_csi() {
        let mut buf = Vec::new();
        let s = Style::default()
            .bold()
            .italic()
            .fg(Color::Red)
            .bg(Color::Indexed(42));
        write_style(&mut buf, &s).unwrap();
        // single CSI ... m with all params joined by ;
        assert_eq!(buf, b"\x1b[1;3;31;48;5;42m");
    }

    #[test]
    fn test_write_with_curly_underline_and_rgb_fg() {
        let mut buf = Vec::new();
        let s = Style::default()
            .underline_style(UnderlineStyle::Curly)
            .fg(Color::Rgb(10, 20, 30));
        write_style(&mut buf, &s).unwrap();
        assert_eq!(buf, b"\x1b[4:3;38;2;10;20;30m");
    }
}
