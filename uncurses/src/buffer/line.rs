//! Owned-row helpers and the [`Line`] type alias used by callers that
//! need an allocated row (rather than a slice into a [`Buffer`]).

use crate::cell::Cell;

/// A single line of cells, used by helpers that allocate owned rows
/// (e.g. [`fill_line`]). Buffer accessors return slices (`&[Cell]`) so
/// callers don't pay an extra dereference through a per-row `Vec`.
pub type Line = Vec<Cell>;

/// Fill an existing row slot in place with `fill`. Wide fills
/// (`fill.width() > 1`) lay down primary + continuation pairs; any
/// trailing slot too narrow to fit another pair is a plain blank.
pub(crate) fn fill_line_into(slot: &mut [Cell], fill: &Cell) {
    let width = slot.len();
    if fill.width() <= 1 {
        slot.fill(fill.clone());
        return;
    }
    let step = fill.width() as usize;
    let mut x = 0;
    while x + step <= width {
        slot[x] = fill.clone();
        for i in 1..step {
            slot[x + i] = Cell::continuation();
        }
        x += step;
    }
    while x < width {
        slot[x] = Cell::BLANK;
        x += 1;
    }
}

/// Create a new line of the given `width` filled with `fill`. Wide
/// fills (`fill.width() > 1`) lay down primary + continuation pairs; any
/// trailing slot too narrow to fit another pair is a plain blank.
pub fn fill_line(width: u16, fill: &Cell) -> Line {
    let mut line = vec![Cell::BLANK; width as usize];
    fill_line_into(&mut line, fill);
    line
}

/// Create a new blank line of the given width.
pub fn blank_line(width: u16) -> Line {
    vec![Cell::BLANK; width as usize]
}
