//! Portable owned rows and internal borrowed row iterators.
//!
//! [`Line`] is an allocated collection of owned [`Cell`] values.
//! [`Buffer::line`](super::Buffer::line) yields borrowed [`CellRef`] values
//! over the buffer's storage.

use crate::cell::Cell;
use crate::cell::CellRef;
use crate::cell::storage::{ImportCache, PackedCell, Resources};
use std::ops::{Bound, RangeBounds};

/// A borrowed row in packed storage or a portable owned row.
#[derive(Clone, Copy)]
pub(crate) enum CellRow<'a> {
    Packed(&'a [PackedCell], &'a Resources),
    Owned(&'a [Cell]),
}

fn range(range: impl RangeBounds<usize>, len: usize) -> std::ops::Range<usize> {
    let start = match range.start_bound() {
        Bound::Included(&n) => n,
        Bound::Excluded(&n) => n.checked_add(1).expect("row index overflow"),
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound() {
        Bound::Included(&n) => n.checked_add(1).expect("row index overflow"),
        Bound::Excluded(&n) => n,
        Bound::Unbounded => len,
    };
    assert!(start <= end && end <= len, "row range out of bounds");
    start..end
}

impl<'a> CellRow<'a> {
    pub(crate) fn from_cells(cells: &'a [Cell]) -> Self {
        Self::Owned(cells)
    }
    pub(crate) fn len(self) -> usize {
        match self {
            Self::Packed(cells, _) => cells.len(),
            Self::Owned(cells) => cells.len(),
        }
    }
    pub(crate) fn is_empty(self) -> bool {
        self.len() == 0
    }
    #[inline]
    pub(crate) fn get(self, index: usize) -> Option<CellRef<'a>> {
        match self {
            Self::Packed(cells, resources) => cells
                .get(index)
                .map(|cell| CellRef::packed(cell, resources)),
            Self::Owned(cells) => cells.get(index).map(Cell::as_ref),
        }
    }
    #[inline]
    pub(crate) fn at(self, index: usize) -> CellRef<'a> {
        self.get(index).expect("row index out of bounds")
    }
    pub(crate) fn iter(self) -> CellIter<'a> {
        CellIter {
            row: self,
            indices: 0..self.len(),
        }
    }
    pub(crate) fn slice(self, bounds: impl RangeBounds<usize>) -> Self {
        let r = range(bounds, self.len());
        match self {
            Self::Packed(cells, owner) => Self::Packed(&cells[r], owner),
            Self::Owned(cells) => Self::Owned(&cells[r]),
        }
    }
    pub(crate) fn first_difference(self, other: Self) -> Option<usize> {
        match (self, other) {
            (Self::Packed(cells, owner), Self::Packed(others, other_owner)) => {
                cells.iter().enumerate().find_map(|(i, &cell)| {
                    others
                        .get(i)
                        .is_none_or(|&other| !owner.value_eq(cell, other_owner, other))
                        .then_some(i)
                })
            }
            _ => self
                .iter()
                .enumerate()
                .find_map(|(i, cell)| (other.get(i) != Some(cell)).then_some(i)),
        }
    }
}
impl PartialEq for CellRow<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.first_difference(*other).is_none()
    }
}
impl Eq for CellRow<'_> {}
impl std::fmt::Debug for CellRow<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

#[derive(Clone)]
pub(crate) struct CellIter<'a> {
    row: CellRow<'a>,
    indices: std::ops::Range<usize>,
}
impl<'a> Iterator for CellIter<'a> {
    type Item = CellRef<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        self.indices.next().map(|i| self.row.at(i))
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        self.indices.nth(n).map(|i| self.row.at(i))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.indices.size_hint()
    }
}
impl DoubleEndedIterator for CellIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.indices.next_back().map(|i| self.row.at(i))
    }
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        self.indices.nth_back(n).map(|i| self.row.at(i))
    }
}
impl ExactSizeIterator for CellIter<'_> {}

pub(crate) struct CellRowMut<'a> {
    pub(super) cells: &'a mut [PackedCell],
    pub(super) resources: &'a mut Resources,
}
impl CellRowMut<'_> {
    pub(crate) fn as_row(&self) -> CellRow<'_> {
        CellRow::Packed(self.cells, self.resources)
    }
    pub(crate) fn len(&self) -> usize {
        self.cells.len()
    }
    pub(crate) fn slice(&mut self, bounds: impl RangeBounds<usize>) -> CellRowMut<'_> {
        let r = range(bounds, self.len());
        CellRowMut {
            cells: &mut self.cells[r],
            resources: self.resources,
        }
    }
    #[cfg(test)]
    pub(crate) fn set(&mut self, index: usize, cell: CellRef<'_>) {
        let incoming = self.resources.import(cell);
        self.resources
            .release(std::mem::replace(&mut self.cells[index], incoming));
    }
    pub(crate) fn fill(&mut self, cell: CellRef<'_>) {
        let incoming = self.resources.import(cell);
        for slot in self.cells.iter_mut() {
            self.resources.retain(incoming);
            self.resources.release(std::mem::replace(slot, incoming));
        }
        self.resources.release(incoming);
    }
    pub(crate) fn fill_line(&mut self, cell: CellRef<'_>) {
        if cell.width() <= 1 {
            return self.fill(cell);
        }
        let incoming = self.resources.import(cell);
        self.fill_line_packed(incoming);
        self.resources.release(incoming);
    }
    pub(super) fn fill_line_packed(&mut self, incoming: PackedCell) {
        let continuation = self.resources.continuation(incoming);
        let step = incoming.width().max(1) as usize;
        let complete = self.len() / step * step;
        for (i, slot) in self.cells.iter_mut().enumerate() {
            let value = if i >= complete {
                PackedCell::BLANK
            } else if i % step == 0 {
                incoming
            } else {
                continuation
            };
            self.resources.retain(value);
            self.resources.release(std::mem::replace(slot, value));
        }
        self.resources.release(continuation);
    }
    pub(crate) fn copy_from(&mut self, source: CellRow<'_>) {
        assert_eq!(self.len(), source.len());
        if let CellRow::Packed(cells, resources) = source
            && resources.is_empty()
            && self.resources.is_empty()
        {
            self.cells.copy_from_slice(cells);
            return;
        }
        let mut translations = ImportCache::default();
        for i in 0..self.len() {
            let incoming = match source {
                CellRow::Packed(cells, resources) => {
                    self.resources
                        .import_packed(resources, cells[i], &mut translations)
                }
                CellRow::Owned(cells) => self.resources.import(cells[i].as_ref()),
            };
            self.resources
                .release(std::mem::replace(&mut self.cells[i], incoming));
        }
        self.resources.release_imports(translations);
    }
    pub(crate) fn rotate_left(&mut self, n: usize) {
        self.cells.rotate_left(n);
    }
    pub(crate) fn rotate_right(&mut self, n: usize) {
        self.cells.rotate_right(n);
    }
}

/// A single line of cells, used by helpers that allocate owned rows
/// (e.g. [`fill_line`]).
///
/// [`Buffer::line`](super::Buffer::line) returns an iterator of borrowed
/// [`CellRef`] values. Collect each value's [`CellRef::to_owned`] result
/// when a portable allocated row is required.
pub type Line = Vec<Cell>;

/// Fill an existing row slot in place with `fill`. Wide fills
/// (`fill.width() > 1`) lay down primary + continuation pairs whose
/// continuation carries the fill's style; any trailing slot too narrow to
/// fit another pair is a plain blank.
pub(crate) fn fill_line_into(slot: &mut [Cell], fill: &Cell) {
    let width = slot.len();
    if fill.width() <= 1 {
        slot.fill(fill.clone());
        return;
    }
    let step = fill.width() as usize;
    // A continuation is the rest of the cell that owns it, so it answers
    // for that cell's colors too. Leaving it default paints a hole in the
    // second column of every pair when the fill carries a background.
    let cont = Cell::CONTINUATION.style(fill.style.clone());
    let mut x = 0;
    while x + step <= width {
        slot[x] = fill.clone();
        for i in 1..step {
            slot[x + i] = cont.clone();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::color::Color;
    use crate::style::Style;

    #[test]
    fn packed_row_copy_preserves_values_after_resource_reclamation() {
        let style = Style::new().fg(Color::Red).underline_color(Color::Red);
        let cells = [
            Cell::new("λ", 1).style(&style),
            Cell::new("中", 3).style(&style),
            Cell::CONTINUATION.style(Style::new().fg(Color::Indexed(1))),
            Cell::new("", 2),
        ];
        let mut source = Buffer::new(4, 1);
        source
            .row_mut(0)
            .unwrap()
            .copy_from(CellRow::from_cells(&cells));
        assert!(source.resources.is_empty());
        let mut target = Buffer::new(4, 1);
        target.row_mut(0).unwrap().fill(
            Cell::new("e\u{301}", 1)
                .style(Style::new().link("https://example.com", ""))
                .as_ref(),
        );
        target.row_mut(0).unwrap().copy_from(source.row(0).unwrap());
        assert!(target.resources.is_empty());
        let mut copy = Buffer::new(4, 1);
        copy.row_mut(0).unwrap().copy_from(target.row(0).unwrap());
        for buffer in [&source, &target, &copy] {
            buffer.resources.audit(&buffer.cells);
            assert_eq!(buffer.row(0).unwrap(), CellRow::from_cells(&cells));
        }
        let row = source.row(0).unwrap();
        assert_eq!(row.first_difference(copy.row(0).unwrap()), None);
        assert_eq!(row.first_difference(row), None);
        assert_eq!(row.first_difference(row.slice(..2)), Some(2));
        target
            .row_mut(0)
            .unwrap()
            .set(2, Cell::CONTINUATION.as_ref());
        assert_eq!(row.first_difference(target.row(0).unwrap()), Some(2));
    }

    #[test]
    fn row_iterator_skips_within_the_remaining_range_and_exhausts() {
        let cells: Vec<_> = "abcdef"
            .chars()
            .map(|c| Cell::new(c.to_string(), 1))
            .collect();
        let row = CellRow::from_cells(&cells);
        let mut iter = row.iter();
        assert_eq!(iter.nth(1).unwrap().content(), "b");
        assert_eq!(iter.nth_back(1).unwrap().content(), "e");
        assert_eq!(iter.len(), 2);
        let mut copy = iter.clone();
        assert_eq!(copy.nth(1).unwrap().content(), "d");
        assert_eq!(copy.next(), None);
        assert_eq!(iter.next_back().unwrap().content(), "d");
        assert_eq!(iter.nth(usize::MAX), None);
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.next_back(), None);
        let mut reverse = row.iter();
        assert_eq!(reverse.nth_back(usize::MAX), None);
        assert_eq!(reverse.size_hint(), (0, Some(0)));
        assert_eq!(reverse.next(), None);
    }
}
