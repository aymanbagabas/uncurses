//! Conversion helpers from widget-library buffer data to uncurses cells.
//!
//! ## Color and style mapping
//!
//! Public helpers convert individual colors and styles into uncurses values.
//! `Reset` colors become `None`, named colors become the closest ANSI basic
//! color, indexed colors keep their palette index, and RGB colors keep their
//! three components. Style conversion copies supported additive modifiers into
//! [`AttrFlags`](uncurses::style::AttrFlags), maps underline to
//! [`UnderlineStyle::Single`], and ignores subtractive modifiers because a
//! concrete buffer cell already carries its effective style.
//!
//! ## Cell conversion
//!
//! Backend drawing uses the private cell converter to combine symbol width and
//! style conversion before writing into the screen buffer.
//!
//! ```text
//! ratatui Cell
//! ├─ symbol ───────────────► width check ───► narrow / wide uncurses Cell
//! └─ fg/bg/underline/style ─► style mapper ─► uncurses Style
//!                                            └► styled Cell
//! ```

use ratatui::style::{Color as RtColor, Modifier, Style as RtStyle};
use uncurses::cell::Cell as CzCell;
use uncurses::color::Color as CzColor;
use uncurses::style::{AttrFlags, Style as CzStyle, UnderlineStyle};

/// Convert a widget-library color into an uncurses color.
///
/// ## Parameters
///
/// * `c` - the source color to encode.
///
/// ## Returns
///
/// `None` for [`RtColor::Reset`], because reset means "no explicit color" in
/// the target style. All other variants return `Some`: named colors map to
/// named [`CzColor`] values, [`RtColor::Indexed`] keeps the same palette index,
/// and [`RtColor::Rgb`] keeps the same red, green, and blue channels.
///
/// ## Errors
///
/// This conversion is infallible.
///
/// ## Panics
///
/// Does not panic.
///
/// ## Usage note
///
/// This helper does not downsample colors for terminal capability profiles;
/// that happens later in the uncurses renderer.
pub fn to_uncurses_color(c: RtColor) -> Option<CzColor> {
    Some(match c {
        RtColor::Reset => return None,
        RtColor::Black => CzColor::Black,
        RtColor::Red => CzColor::Red,
        RtColor::Green => CzColor::Green,
        RtColor::Yellow => CzColor::Yellow,
        RtColor::Blue => CzColor::Blue,
        RtColor::Magenta => CzColor::Magenta,
        RtColor::Cyan => CzColor::Cyan,
        RtColor::Gray => CzColor::White,
        RtColor::DarkGray => CzColor::BrightBlack,
        RtColor::LightRed => CzColor::BrightRed,
        RtColor::LightGreen => CzColor::BrightGreen,
        RtColor::LightYellow => CzColor::BrightYellow,
        RtColor::LightBlue => CzColor::BrightBlue,
        RtColor::LightMagenta => CzColor::BrightMagenta,
        RtColor::LightCyan => CzColor::BrightCyan,
        RtColor::White => CzColor::BrightWhite,
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

/// Convert a widget-library style into an uncurses style.
///
/// ## Parameters
///
/// * `s` - the source style. The conversion reads `fg`, `bg`,
///   `underline_color`, and `add_modifier`.
///
/// ## Returns
///
/// A [`CzStyle`] with foreground, background, and underline colors converted by
/// [`to_uncurses_color`]. Additive modifiers map as follows: bold, dim, italic,
/// slow blink, rapid blink, reversed, hidden, and crossed-out become the
/// corresponding uncurses attribute flags. Underline becomes
/// [`UnderlineStyle::Single`] when [`Modifier::UNDERLINED`] is present, and
/// [`UnderlineStyle::None`] otherwise.
///
/// ## Errors
///
/// This conversion is infallible.
///
/// ## Panics
///
/// Does not panic.
///
/// ## Usage note
///
/// `sub_modifier` is ignored. The backend converts concrete buffer cells, whose
/// style has already been resolved by the widget library.
pub fn to_uncurses_style(s: RtStyle) -> CzStyle {
    let attrs = to_attrs(s.add_modifier);
    let underline = if s.add_modifier.contains(Modifier::UNDERLINED) {
        UnderlineStyle::Single
    } else {
        UnderlineStyle::None
    };
    let mut style = CzStyle::default().underline_style(underline);
    if let Some(fg) = s.fg.and_then(to_uncurses_color) {
        style = style.fg(fg);
    }
    if let Some(bg) = s.bg.and_then(to_uncurses_color) {
        style = style.bg(bg);
    }
    if let Some(uc) = s.underline_color.and_then(to_uncurses_color) {
        style = style.underline_color(uc);
    }
    style.attrs(attrs)
}

/// Halfwidth Katakana Voiced Sound Mark (dakuten).
const HALFWIDTH_KATAKANA_VOICED_SOUND_MARK: char = '\u{FF9E}';
/// Halfwidth Katakana Semi-Voiced Sound Mark (handakuten).
const HALFWIDTH_KATAKANA_SEMI_VOICED_SOUND_MARK: char = '\u{FF9F}';

/// Display width of `s` in terminal cells, matching the widget library's cell
/// width for strings. Includes a fast path for single-byte ASCII and a `+1`
/// compensation for each halfwidth dakuten/handakuten that `unicode-width`
/// reports as zero.
///
/// The count is returned whole. A caller that stores it in a narrower type
/// has to bound it there, where the bound can be applied to the real width
/// rather than to a wrapped one.
fn str_cell_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    if s.len() == 1 {
        1
    } else {
        let width = s.width();
        let extra = s
            .chars()
            .filter(|c| {
                matches!(
                    *c,
                    HALFWIDTH_KATAKANA_VOICED_SOUND_MARK
                        | HALFWIDTH_KATAKANA_SEMI_VOICED_SOUND_MARK
                )
            })
            .count();
        width + extra
    }
}

/// Convert a concrete buffer cell into the uncurses cell staged in the buffer.
///
/// The cell is credited with the number of columns its symbol measures, so a
/// cluster wider than two columns keeps every column it takes. A symbol that
/// measures nothing still occupies the one column ratatui gave it. The source
/// cell's foreground, background, underline color, and modifiers are converted
/// through [`to_uncurses_style`].
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
    // A ratatui cell carries whatever symbol the caller stored, and a cell
    // here is credited with the columns that symbol measures. Clamping the
    // count would let the two grids disagree about where the next column
    // starts. The floor of one keeps a zero-width symbol occupying the
    // column ratatui gave it.
    let width = str_cell_width(symbol).clamp(1, u8::MAX as usize) as u8;
    let cell = CzCell::new(symbol, width);
    cell.style(style)
}

#[cfg(test)]
mod tests {
    use super::cell_from_ratatui;

    fn width_of(symbol: &str) -> u8 {
        let mut rc = ratatui::buffer::Cell::default();
        rc.set_symbol(symbol);
        cell_from_ratatui(&rc).width()
    }

    #[test]
    fn a_converted_cell_keeps_the_columns_its_symbol_measures() {
        assert_eq!(width_of("a"), 1);
        assert_eq!(width_of("世"), 2);
        // ratatui lets a caller store any string in a cell. Capping the
        // count would leave this grid crediting the symbol with fewer
        // columns than the terminal advances, and every later column on the
        // row would disagree.
        assert_eq!(width_of("abc"), 3);
    }

    #[test]
    fn a_symbol_too_wide_for_a_cell_keeps_the_widest_count_one_can_hold() {
        // A cell records its width in a `u8`, so a symbol measuring more
        // columns than that takes the largest count the type holds. Reading
        // the width into a narrower type first would wrap it: 65,536
        // columns would come back as none at all, and the cap meant to
        // catch that would read it as a single column instead.
        let huge = "\u{4e16}".repeat(32_768);
        assert_eq!(super::str_cell_width(&huge), 65_536);
        assert_eq!(width_of(&huge), u8::MAX);
    }

    #[test]
    fn a_symbol_that_measures_nothing_still_holds_its_column() {
        // ratatui gave the symbol a cell, so it keeps one here too.
        assert_eq!(width_of("\u{200b}"), 1);
    }
}
