use crate::{ansi::graphics::KITTY_PLACEHOLDER_DIACRITICS, cell::Cell};

pub(super) fn is_placeholder(cell: &Cell) -> bool {
    cell.width() == 1 && cell.content().starts_with('\u{10eeee}')
}

fn coordinates(cell: &Cell) -> Option<(char, usize, char)> {
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

pub(super) fn compressed(row: &[Cell], x: usize) -> Option<Cell> {
    let current = row.get(x)?;
    let previous = row.get(x.checked_sub(1)?)?;
    let (cy, cx, ch) = coordinates(current)?;
    let (py, px, ph) = coordinates(previous)?;
    (cy == py
        && cx == px + 1
        && ch == ph
        && current.style.fg == previous.style.fg
        && current.style.underline_color == previous.style.underline_color)
        .then(|| Cell::new("\u{10eeee}", 1).style(&current.style))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;

    fn placeholder(y: usize, x: usize, high: Option<usize>) -> Cell {
        let table = KITTY_PLACEHOLDER_DIACRITICS;
        let mut text = format!("\u{10eeee}{}{}", table[y], table[x]);
        if let Some(high) = high {
            text.push(table[high]);
        }
        Cell::new(&text, 1)
    }

    #[test]
    fn only_matching_consecutive_coordinates_share_an_anchor() {
        let first = placeholder(2, 4, Some(3));
        let next = placeholder(2, 5, Some(3));
        assert_eq!(compressed(&[first.clone(), next.clone()], 0), None);
        assert_eq!(
            compressed(&[first.clone(), next.clone()], 1)
                .unwrap()
                .content(),
            "\u{10eeee}"
        );
        for previous in [
            Cell::BLANK,
            Cell::new("\u{10eeee}", 1),
            placeholder(1, 4, Some(3)),
            placeholder(2, 3, Some(3)),
            placeholder(2, 4, Some(2)),
            first
                .clone()
                .style(crate::style::Style::new().fg(Color::Red)),
            first.style(crate::style::Style::new().underline_color(Color::Red)),
        ] {
            assert_eq!(compressed(&[previous, next.clone()], 1), None);
        }
        assert!(compressed(&[placeholder(2, 4, None), placeholder(2, 5, Some(0))], 1).is_some());
    }

    #[test]
    fn sparse_edits_repair_the_next_anchor_and_preserve_desired_coordinates() {
        use crate::{
            buffer::Surface,
            renderer::{RenderBuffer, Renderer},
        };
        let mut renderer = Renderer::new();
        let mut front = RenderBuffer::new(4, 1);
        for x in 0..4 {
            front.set_cell((x, 0), &placeholder(0, x as usize, None));
        }
        renderer.sync_front(&mut front);
        assert_eq!(
            renderer.back_buf.cell((1, 0).into()).unwrap().content(),
            "\u{10eeee}"
        );
        renderer.back_buf.clear_touched();
        front.set_cell((0, 0), &Cell::new("X", 1));
        renderer.sync_front(&mut front);
        assert_eq!(
            renderer.back_buf.cell((1, 0).into()),
            Some(&placeholder(0, 1, None))
        );
        assert_eq!(
            renderer.back_buf.cell((2, 0).into()).unwrap().content(),
            "\u{10eeee}"
        );
        assert_eq!(front.cell((2, 0).into()), Some(&placeholder(0, 2, None)));
        front.set_cell((0, 0), &placeholder(0, 0, None));
        renderer.sync_front(&mut front);
        assert_eq!(
            renderer.back_buf.cell((1, 0).into()).unwrap().content(),
            "\u{10eeee}"
        );
    }
}
