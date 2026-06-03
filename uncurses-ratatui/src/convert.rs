use ratatui::style::{Color as RtColor, Modifier, Style as RtStyle};
use uncurses::cell::Cell as CzCell;
use uncurses::color::{BasicColor, Color as CzColor};
use uncurses::style::{AttrFlags, Style as CzStyle, UnderlineStyle};

/// Convert a ratatui color into a uncurses color. `Reset` maps to `None`.
pub fn to_uncurses_color(c: RtColor) -> Option<CzColor> {
    Some(match c {
        RtColor::Reset => return None,
        RtColor::Black => BasicColor::Black.into(),
        RtColor::Red => BasicColor::Red.into(),
        RtColor::Green => BasicColor::Green.into(),
        RtColor::Yellow => BasicColor::Yellow.into(),
        RtColor::Blue => BasicColor::Blue.into(),
        RtColor::Magenta => BasicColor::Magenta.into(),
        RtColor::Cyan => BasicColor::Cyan.into(),
        RtColor::Gray => BasicColor::White.into(),
        RtColor::DarkGray => BasicColor::BrightBlack.into(),
        RtColor::LightRed => BasicColor::BrightRed.into(),
        RtColor::LightGreen => BasicColor::BrightGreen.into(),
        RtColor::LightYellow => BasicColor::BrightYellow.into(),
        RtColor::LightBlue => BasicColor::BrightBlue.into(),
        RtColor::LightMagenta => BasicColor::BrightMagenta.into(),
        RtColor::LightCyan => BasicColor::BrightCyan.into(),
        RtColor::White => BasicColor::BrightWhite.into(),
        RtColor::Rgb(r, g, b) => CzColor::Rgb(r, g, b),
        RtColor::Indexed(i) => CzColor::Indexed(i),
    })
}

fn to_attrs(m: Modifier) -> AttrFlags {
    let mut a = AttrFlags::empty();
    if m.contains(Modifier::BOLD) {
        a |= AttrFlags::BOLD;
    }
    if m.contains(Modifier::DIM) {
        a |= AttrFlags::FAINT;
    }
    if m.contains(Modifier::ITALIC) {
        a |= AttrFlags::ITALIC;
    }
    if m.contains(Modifier::SLOW_BLINK) {
        a |= AttrFlags::SLOW_BLINK;
    }
    if m.contains(Modifier::RAPID_BLINK) {
        a |= AttrFlags::RAPID_BLINK;
    }
    if m.contains(Modifier::REVERSED) {
        a |= AttrFlags::REVERSE;
    }
    if m.contains(Modifier::HIDDEN) {
        a |= AttrFlags::CONCEAL;
    }
    if m.contains(Modifier::CROSSED_OUT) {
        a |= AttrFlags::STRIKETHROUGH;
    }
    a
}

/// Convert a ratatui style into a uncurses style.
///
/// Ratatui models underline as a modifier flag; the result uses
/// [`UnderlineStyle::Single`] when underlined and [`UnderlineStyle::None`]
/// otherwise.
pub fn to_uncurses_style(s: RtStyle) -> CzStyle {
    let attrs = to_attrs(s.add_modifier);
    let underline = if s.add_modifier.contains(Modifier::UNDERLINED) {
        UnderlineStyle::Single
    } else {
        UnderlineStyle::None
    };
    let mut style = CzStyle::EMPTY.with_underline_style(underline);
    if let Some(fg) = s.fg.and_then(to_uncurses_color) {
        style = style.with_fg(fg);
    }
    if let Some(bg) = s.bg.and_then(to_uncurses_color) {
        style = style.with_bg(bg);
    }
    if let Some(uc) = s.underline_color.and_then(to_uncurses_color) {
        style = style.with_underline_color(uc);
    }
    style.with_attrs(attrs)
}

/// Halfwidth Katakana Voiced Sound Mark (dakuten).
const HALFWIDTH_KATAKANA_VOICED_SOUND_MARK: char = '\u{FF9E}';
/// Halfwidth Katakana Semi-Voiced Sound Mark (handakuten).
const HALFWIDTH_KATAKANA_SEMI_VOICED_SOUND_MARK: char = '\u{FF9F}';

/// Display width of `s` in terminal cells, matching ratatui's `CellWidth` for
/// `str`: includes a fast path for single-byte ASCII and a `+1` compensation
/// for each halfwidth dakuten/handakuten that `unicode-width` reports as zero.
fn str_cell_width(s: &str) -> u16 {
    use unicode_width::UnicodeWidthStr;
    if s.len() == 1 {
        1
    } else {
        let width = s.width() as u16;
        let extra = s
            .chars()
            .filter(|c| {
                matches!(
                    *c,
                    HALFWIDTH_KATAKANA_VOICED_SOUND_MARK
                        | HALFWIDTH_KATAKANA_SEMI_VOICED_SOUND_MARK
                )
            })
            .count() as u16;
        width.saturating_add(extra)
    }
}

pub(crate) fn cell_from_ratatui(rc: &ratatui::buffer::Cell) -> CzCell {
    let style = RtStyle {
        fg: Some(rc.fg),
        bg: Some(rc.bg),
        underline_color: Some(rc.underline_color),
        add_modifier: rc.modifier,
        sub_modifier: Modifier::empty(),
    };
    let style = to_uncurses_style(style);
    let symbol = rc.symbol();
    let width = str_cell_width(symbol).max(1).min(u8::MAX as u16) as u8;
    let cell = match width {
        2 => CzCell::wide(symbol),
        _ => CzCell::narrow(symbol),
    };
    cell.with_style(style)
}
