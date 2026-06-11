//! SGR (Select Graphic Rendition) parameter parsing.
//!
//! Decodes a sequence of SGR parameters (the numbers in `CSI ... m`)
//! into a [`Style`], handling all of:
//!
//! * Reset / individual attribute set+clear (0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 21–29)
//! * Underline styles via `4:n` colon subparameters
//! * 8-color fg/bg (30–37, 40–47) and bright variants (90–97, 100–107)
//! * Default fg/bg/underline (39, 49, 59)
//! * Extended color (`38`, `48`, `58`) with both semicolon and colon syntaxes:
//!   * `38;5;n` / `38:5:n` indexed
//!   * `38;2;r;g;b` / `38:2:r:g:b` / `38:2::r:g:b` (with optional colorspace id) truecolor
//!
//! Input is a lazy [`Params`] walker over the raw `CSI ... m` body. The
//! `Option<u32>` slot convention applies: `None` marks an omitted slot
//! (e.g. `CSI ;5 m` or `38:2::r:g:b`), which SGR treats as the default
//! value `0`.

use super::{AttrFlags, Style, UnderlineStyle};
use crate::ansi::params::{Group, Params};
use crate::color::{BasicColor, Color};

/// Read SGR parameters into a mutable style.
///
/// Modifies `style` in-place. A `0` parameter (or an empty parameter
/// list) resets the style to [`Style::EMPTY`].
pub fn read_style(params: Params<'_>, style: &mut Style) {
    if params.is_empty() {
        *style = Style::EMPTY;
        return;
    }

    let mut groups = params.iter();
    while let Some(g) = groups.next() {
        let main = sgr_main(g);
        match main {
            0 => *style = Style::EMPTY,
            1 => style.attrs |= AttrFlags::BOLD,
            2 => style.attrs |= AttrFlags::FAINT,
            3 => style.attrs |= AttrFlags::ITALIC,
            4 => {
                // Either `4` (single underline) or `4:n` for styled underline.
                style.underline = if let Some(sub) = g.nth(1) {
                    underline_from_value(sub)
                } else {
                    UnderlineStyle::Single
                };
            }
            5 => style.attrs |= AttrFlags::SLOW_BLINK,
            6 => style.attrs |= AttrFlags::RAPID_BLINK,
            7 => style.attrs |= AttrFlags::REVERSE,
            8 => style.attrs |= AttrFlags::CONCEAL,
            9 => style.attrs |= AttrFlags::STRIKETHROUGH,
            21 => style.underline = UnderlineStyle::Double,
            22 => style.attrs.remove(AttrFlags::BOLD | AttrFlags::FAINT),
            23 => style.attrs.remove(AttrFlags::ITALIC),
            24 => style.underline = UnderlineStyle::None,
            25 => style
                .attrs
                .remove(AttrFlags::SLOW_BLINK | AttrFlags::RAPID_BLINK),
            27 => style.attrs.remove(AttrFlags::REVERSE),
            28 => style.attrs.remove(AttrFlags::CONCEAL),
            29 => style.attrs.remove(AttrFlags::STRIKETHROUGH),
            30..=37 => {
                style.fg = Some(Color::Basic(
                    BasicColor::from_u8((main - 30) as u8).unwrap(),
                ));
            }
            38 => style.fg = read_extended_color(g, &mut groups),
            39 => style.fg = None,
            40..=47 => {
                style.bg = Some(Color::Basic(
                    BasicColor::from_u8((main - 40) as u8).unwrap(),
                ));
            }
            48 => style.bg = read_extended_color(g, &mut groups),
            49 => style.bg = None,
            58 => style.underline_color = read_extended_color(g, &mut groups),
            59 => style.underline_color = None,
            90..=97 => {
                style.fg = Some(Color::Basic(
                    BasicColor::from_u8((main - 90 + 8) as u8).unwrap(),
                ));
            }
            100..=107 => {
                style.bg = Some(Color::Basic(
                    BasicColor::from_u8((main - 100 + 8) as u8).unwrap(),
                ));
            }
            _ => {} // unknown — ignore
        }
    }
}

/// Parse just an extended color (`38`, `48`, or `58`) starting at the
/// group at index `idx`. Returns `(parsed color, number of top-level
/// groups consumed)`.
pub fn read_style_color(params: Params<'_>, idx: usize) -> (Option<Color>, usize) {
    let Some(g) = params.group(idx) else {
        return (None, 1);
    };
    let before = remaining_groups(params, idx + 1);
    let mut rest = params.slice_from(idx + 1).iter();
    let color = read_extended_color(g, &mut rest);
    let after = rest.count();
    (color, 1 + (before - after))
}

fn remaining_groups(params: Params<'_>, start: usize) -> usize {
    params.slice_from(start).iter().count()
}

/// Main parameter value of an SGR group. Omitted slot → `0` per
/// ECMA-48 §8.3.117.
fn sgr_main(g: Group<'_>) -> u32 {
    g.first().unwrap_or(0)
}

fn underline_from_value(v: u32) -> UnderlineStyle {
    match v {
        0 => UnderlineStyle::None,
        1 => UnderlineStyle::Single,
        2 => UnderlineStyle::Double,
        3 => UnderlineStyle::Curly,
        4 => UnderlineStyle::Dotted,
        5 => UnderlineStyle::Dashed,
        _ => UnderlineStyle::Single,
    }
}

/// Read an extended-color sequence for the leading group `g`
/// (`38`/`48`/`58`). For the semicolon form, additional values are
/// consumed from `rest`.
///
/// Handles both forms:
/// * Colon form: a single group like `[38, 5, n]` or
///   `[38, 2, 0, r, g, b]` (with the empty colorspace slot).
/// * Semicolon form: subsequent groups are separate entries —
///   `38;5;n` or `38;2;r;g;b`.
fn read_extended_color<'a, I>(g: Group<'a>, rest: &mut I) -> Option<Color>
where
    I: Iterator<Item = Group<'a>>,
{
    let len = g.len();
    if len > 1 {
        // Colon-subparameter form.
        let kind = g.nth(1).unwrap_or(0);
        return match kind {
            5 => Some(index_to_color(g.nth(2).unwrap_or(0) as u8)),
            2 => {
                // Colon form may include an optional "colorspace id" slot at index 2.
                //   38:2:r:g:b           -> values = [38, 2, r, g, b]      (len 5)
                //   38:2::r:g:b          -> values = [38, 2, None, r, g, b] (len 6)
                let (r, g_, b) = if len >= 6 {
                    (
                        g.nth(3).unwrap_or(0) as u8,
                        g.nth(4).unwrap_or(0) as u8,
                        g.nth(5).unwrap_or(0) as u8,
                    )
                } else {
                    (
                        g.nth(2).unwrap_or(0) as u8,
                        g.nth(3).unwrap_or(0) as u8,
                        g.nth(4).unwrap_or(0) as u8,
                    )
                };
                Some(Color::Rgb(r, g_, b))
            }
            _ => None,
        };
    }

    // Semicolon form: consume following top-level groups. Omitted
    // slots (e.g. `38;;5;n`) decode as `0`.
    let kind = rest.next().map(|g| g.first().unwrap_or(0)).unwrap_or(0);
    match kind {
        5 => {
            let n = rest.next().map(|g| g.first().unwrap_or(0)).unwrap_or(0) as u8;
            Some(index_to_color(n))
        }
        2 => {
            let r = rest.next().map(|g| g.first().unwrap_or(0)).unwrap_or(0) as u8;
            let gn = rest.next().map(|g| g.first().unwrap_or(0)).unwrap_or(0) as u8;
            let b = rest.next().map(|g| g.first().unwrap_or(0)).unwrap_or(0) as u8;
            Some(Color::Rgb(r, gn, b))
        }
        _ => None,
    }
}

fn index_to_color(n: u8) -> Color {
    if let Some(b) = BasicColor::from_u8(n) {
        Color::Basic(b)
    } else {
        Color::Indexed(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rs(body: &[u8]) -> Style {
        let mut s = Style::EMPTY;
        read_style(Params::from_raw(body), &mut s);
        s
    }

    #[test]
    fn reset() {
        let mut s = Style::EMPTY.bold();
        read_style(Params::from_raw(b"0"), &mut s);
        assert!(s.is_empty());
    }

    #[test]
    fn bold_italic() {
        let s = rs(b"1;3");
        assert!(s.attrs.contains(AttrFlags::BOLD));
        assert!(s.attrs.contains(AttrFlags::ITALIC));
    }

    #[test]
    fn underline_styles() {
        assert_eq!(rs(b"4:3").underline, UnderlineStyle::Curly);
        assert_eq!(rs(b"4").underline, UnderlineStyle::Single);

        let mut s = Style::EMPTY.underline();
        read_style(Params::from_raw(b"24"), &mut s);
        assert_eq!(s.underline, UnderlineStyle::None);
    }

    #[test]
    fn basic_fg_bg() {
        let s = rs(b"31;42");
        assert_eq!(s.fg, Some(Color::Basic(BasicColor::Red)));
        assert_eq!(s.bg, Some(Color::Basic(BasicColor::Green)));
    }

    #[test]
    fn bright_fg_bg() {
        let s = rs(b"91;107");
        assert_eq!(s.fg, Some(Color::Basic(BasicColor::BrightRed)));
        assert_eq!(s.bg, Some(Color::Basic(BasicColor::BrightWhite)));
    }

    #[test]
    fn defaults_clear() {
        let mut s = Style::EMPTY
            .fg(Color::Basic(BasicColor::Red))
            .bg(Color::Basic(BasicColor::Blue))
            .underline_color(Color::Basic(BasicColor::Green));
        read_style(Params::from_raw(b"39;49;59"), &mut s);
        assert_eq!(s.fg, None);
        assert_eq!(s.bg, None);
        assert_eq!(s.underline_color, None);
    }

    #[test]
    fn fg_256_semicolon() {
        assert_eq!(rs(b"38;5;200").fg, Some(Color::Indexed(200)));
    }

    #[test]
    fn fg_truecolor_semicolon() {
        assert_eq!(rs(b"38;2;255;100;50").fg, Some(Color::Rgb(255, 100, 50)));
    }

    #[test]
    fn fg_truecolor_colon() {
        assert_eq!(rs(b"38:2:255:100:50").fg, Some(Color::Rgb(255, 100, 50)));
    }

    #[test]
    fn fg_truecolor_colon_with_empty_colorspace() {
        // `38:2::r:g:b` — six values, omitted colorspace slot.
        assert_eq!(rs(b"38:2::255:100:50").fg, Some(Color::Rgb(255, 100, 50)));
    }

    #[test]
    fn fg_256_colon() {
        assert_eq!(rs(b"38:5:200").fg, Some(Color::Indexed(200)));
    }

    #[test]
    fn bg_256_semicolon() {
        // 0..=15 promotes to BasicColor.
        assert_eq!(rs(b"48;5;7").bg, Some(Color::Basic(BasicColor::White)));
    }

    #[test]
    fn underline_color_truecolor() {
        assert_eq!(
            rs(b"58:2:10:20:30").underline_color,
            Some(Color::Rgb(10, 20, 30))
        );
    }

    #[test]
    fn clear_bold_keeps_faint_off() {
        // `22` should clear both bold and faint.
        let mut s = Style::EMPTY.bold().faint();
        read_style(Params::from_raw(b"22"), &mut s);
        assert!(!s.attrs.contains(AttrFlags::BOLD));
        assert!(!s.attrs.contains(AttrFlags::FAINT));
    }

    #[test]
    fn empty_params_resets() {
        let mut s = Style::EMPTY.bold();
        read_style(Params::EMPTY, &mut s);
        assert!(s.is_empty());
    }

    #[test]
    fn from_subparam_body() {
        let s = rs(b"1;31");
        assert!(s.attrs.contains(AttrFlags::BOLD));
        assert_eq!(s.fg, Some(Color::Basic(BasicColor::Red)));
    }

    #[test]
    fn omitted_top_level_param_acts_as_reset() {
        // `CSI ; m` — leading empty slot decodes as `0` ⇒ reset.
        let mut s = Style::EMPTY.bold();
        read_style(Params::from_raw(b";"), &mut s);
        assert!(s.is_empty());
    }
}
