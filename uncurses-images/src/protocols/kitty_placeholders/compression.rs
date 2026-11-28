use uncurses::{ansi::graphics::KITTY_PLACEHOLDER_DIACRITICS, cell::Cell};

#[inline]
pub(super) fn is_placeholder(cell: &Cell) -> bool {
    cell.width() == 1 && cell.content().starts_with('\u{10eeee}')
}

pub(super) fn coordinates(cell: &Cell) -> Option<(char, usize, char)> {
    if !is_placeholder(cell) {
        return None;
    }
    let mut chars = cell.content().chars().skip(1);
    let row = chars.next()?;
    let column = KITTY_PLACEHOLDER_DIACRITICS
        .binary_search(&chars.next()?)
        .ok()?;
    let high = chars.next().unwrap_or(KITTY_PLACEHOLDER_DIACRITICS[0]);
    (chars.next().is_none()
        && KITTY_PLACEHOLDER_DIACRITICS.binary_search(&row).is_ok()
        && KITTY_PLACEHOLDER_DIACRITICS[..256]
            .binary_search(&high)
            .is_ok())
    .then_some((row, column, high))
}

pub(crate) fn compressed(previous: &Cell, current: &Cell) -> Option<Cell> {
    let (cy, cx, ch) = coordinates(current)?;
    let (py, px, ph) = coordinates(previous)?;
    (cy == py
        && cx == px + 1
        && ch == ph
        && current.style().fg == previous.style().fg
        && current.style().underline_color == previous.style().underline_color)
        .then(|| Cell::new("\u{10eeee}", 1, current.style()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uncurses::color::Color;
    use uncurses::style::Style;

    fn placeholder(y: usize, x: usize, high: Option<usize>) -> Cell {
        let table = KITTY_PLACEHOLDER_DIACRITICS;
        let mut text = format!("\u{10eeee}{}{}", table[y], table[x]);
        if let Some(high) = high {
            text.push(table[high]);
        }
        Cell::new(&text, 1, Style::EMPTY)
    }

    #[test]
    fn only_matching_consecutive_coordinates_share_an_anchor() {
        let first = placeholder(2, 4, Some(3));
        let next = placeholder(2, 5, Some(3));
        assert_eq!(compressed(&first, &next).unwrap().content(), "\u{10eeee}");
        let mut colored = first.clone();
        colored.set_style(Style::new().fg(Color::Red));
        let mut underlined = first.clone();
        underlined.set_style(Style::new().underline_color(Color::Red));
        for previous in [
            Cell::BLANK,
            Cell::new("\u{10eeee}", 1, Style::EMPTY),
            placeholder(1, 4, Some(3)),
            placeholder(2, 3, Some(3)),
            placeholder(2, 4, Some(2)),
            colored,
            underlined,
        ] {
            assert_eq!(compressed(&previous, &next), None);
        }
        assert!(compressed(&placeholder(2, 4, None), &placeholder(2, 5, Some(0))).is_some());
    }
}
