//! Packed cell grids, portable rows, and clipped surface access.

mod line;
mod ops;
mod surface;
#[cfg(test)]
mod tests;
mod text_buffer;
mod view;
mod window;

pub(crate) use line::{CellRow, CellRowMut};
pub use line::{Line, blank_line, fill_line};
pub use surface::{Bounded, Surface, SurfaceMut};
pub use text_buffer::TextBuffer;
pub use view::View;
pub use window::Window;

#[cfg(test)]
use crate::cell::Cell;
use crate::cell::CellRef;
use crate::cell::storage::{PackedCell, Resources, StyleParts};
use crate::layout::{Position, Rect, overruns};
use crate::style::Style;
use crate::text::{TextSurface, WidthMode};

/// An owned, text-capable grid with a width policy and private cell resources.
///
/// The grid stores sixteen bytes per column. Complex content and style
/// extras share private resources within this buffer. Borrowed reads do
/// not allocate. Clone makes an independent resource owner.
/// These sixteen-byte slots are internal storage; [`crate::cell::Cell`]
/// remains the portable owned value for imports and snapshots.
///
/// [`TextSurface`] uses this buffer's width policy for new text.
/// [`Surface::new_cell`] validates one nonzero-width grapheme and measures it once.
/// Copies, imports, and resize preserve stored widths.
///
/// Reads return [`CellRef`] views. Writes import their content and styles
/// into this buffer's resources, so the source can be dropped after the write.
/// Hyperlinks retain their original shared allocation.
///
/// Inherent insert/delete methods rotate stored columns and rows.
/// [`TextBuffer`] uses the [`SurfaceMut`] defaults, which snapshot primaries
/// and replay cell writes. At a span that cuts through a wide cell,
/// these paths can affect different boundary columns. In particular,
/// default deletion clears a primary before the span when its continuation
/// lies inside the span; an inherent rotation retains that primary.
///
/// ```
/// use uncurses::buffer::{Buffer, Surface};
/// use uncurses::layout::Position;
/// use uncurses::style::Style;
/// let mut buffer = Buffer::new(4, 1);
/// let cell = buffer.new_cell("中", Style::new()).unwrap();
/// buffer.set_cell(Position::new(0, 0), cell.as_ref());
/// assert!(buffer.cell(Position::new(1, 0)).unwrap().is_continuation());
/// ```
pub struct Buffer {
    cells: Vec<PackedCell>,
    resources: Resources,
    width: u16,
    height: u16,
    width_mode: WidthMode,
    eaw_wide: bool,
}

impl Clone for Buffer {
    fn clone(&self) -> Self {
        let resources = self.resources.clone();
        let cells = self.cells.clone();
        Self {
            cells,
            resources,
            width: self.width,
            height: self.height,
            width_mode: self.width_mode,
            eaw_wide: self.eaw_wide,
        }
    }
}
impl std::fmt::Debug for Buffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Buffer")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("width_mode", &self.width_mode)
            .field("eaw_wide", &self.eaw_wide)
            .finish_non_exhaustive()
    }
}

impl Buffer {
    /// Create a blank grid with the default width policy.
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            cells: vec![PackedCell::BLANK; width as usize * height as usize],
            resources: Resources::default(),
            width,
            height,
            width_mode: WidthMode::Wc,
            eaw_wide: false,
        }
    }
    /// Return the column count.
    pub fn width(&self) -> u16 {
        self.width
    }
    /// Return the row count.
    pub fn height(&self) -> u16 {
        self.height
    }
    /// Select the width policy.
    pub fn with_width_mode(mut self, mode: WidthMode) -> Self {
        self.set_width_mode(mode);
        self
    }
    /// Select the ambiguous-width policy.
    pub fn with_eaw_wide(mut self, wide: bool) -> Self {
        self.set_eaw_wide(wide);
        self
    }
    /// Set the width policy for subsequent text.
    ///
    /// Existing cells retain their stored widths, including during copies.
    pub fn set_width_mode(&mut self, mode: WidthMode) {
        self.width_mode = mode;
    }
    /// Set the ambiguous-width policy for subsequent text.
    ///
    /// Existing cells retain their stored widths.
    pub fn set_eaw_wide(&mut self, wide: bool) {
        self.eaw_wide = wide;
    }
    /// Borrow the underlying buffer.
    pub fn buffer(&self) -> &Self {
        self
    }
    /// Borrow the underlying buffer mutably.
    pub fn buffer_mut(&mut self) -> &mut Self {
        self
    }
    /// Return the underlying buffer.
    pub fn into_buffer(self) -> Self {
        self
    }
    /// Write a portable or borrowed cell.
    ///
    /// Pass a [`CellRef`] or a reference to an owned [`crate::cell::Cell`].
    /// This imports resources and preserves the supplied width.
    /// See [`Self::set`] for clipping and continuation behavior.
    pub fn set_cell<'a>(&mut self, pos: Position, cell: impl Into<CellRef<'a>>) {
        self.set(pos, cell);
    }
    /// Fill the buffer with a portable or borrowed cell.
    pub fn fill<'a>(&mut self, cell: impl Into<CellRef<'a>>) {
        SurfaceMut::fill(self, cell.into());
    }
    /// Fill a rectangle with a portable or borrowed cell.
    pub fn fill_rect<'a>(&mut self, rect: Rect, cell: impl Into<CellRef<'a>>) {
        SurfaceMut::fill_rect(self, rect, cell.into());
    }
    /// Iterate over one row without an allocation.
    ///
    /// Returns `None` when `y` is outside the grid. The iterator yields
    /// one [`CellRef`] per column, including continuations, in column order.
    /// It supports exact lengths, reverse iteration, and cloning.
    /// Use `.map(|cell| cell.to_owned())` to collect portable cells.
    pub fn line(
        &self,
        y: u16,
    ) -> Option<impl ExactSizeIterator<Item = CellRef<'_>> + DoubleEndedIterator + Clone> {
        self.row(y).map(CellRow::iter)
    }
    pub(crate) fn row(&self, y: u16) -> Option<CellRow<'_>> {
        if y >= self.height {
            return None;
        }
        let start = y as usize * self.width as usize;
        Some(CellRow::Packed(
            &self.cells[start..start + self.width as usize],
            &self.resources,
        ))
    }
    pub(crate) fn row_mut(&mut self, y: u16) -> Option<CellRowMut<'_>> {
        if y >= self.height {
            return None;
        }
        let start = y as usize * self.width as usize;
        Some(CellRowMut {
            cells: &mut self.cells[start..start + self.width as usize],
            resources: &mut self.resources,
        })
    }
    fn swap_rows(&mut self, a: usize, b: usize) {
        if a == b {
            return;
        }
        let w = self.width as usize;
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        let (left, right) = self.cells.split_at_mut(hi * w);
        left[lo * w..(lo + 1) * w].swap_with_slice(&mut right[..w]);
    }
    /// Write a primary cell and update its continuation columns.
    ///
    /// A continuation input leaves the grid unchanged. A clipped primary
    /// becomes a blank with the incoming style.
    pub fn set<'a>(&mut self, pos: impl Into<Position>, cell: impl Into<CellRef<'a>>) {
        let pos = pos.into();
        let cell = cell.into();
        if pos.x >= self.width || pos.y >= self.height || cell.is_continuation() {
            return;
        }
        let incoming = self.resources.import(cell);
        self.set_packed(pos, incoming);
        self.resources.release(incoming);
    }
    fn replace_packed(&mut self, index: usize, incoming: PackedCell) {
        self.resources.retain(incoming);
        self.resources
            .release(std::mem::replace(&mut self.cells[index], incoming));
    }
    fn blank_span(&mut self, start: usize, end: usize, source: PackedCell) {
        let blank = self.resources.blank(source);
        for i in start..end {
            self.replace_packed(i, blank);
        }
        self.resources.release(blank);
    }
    fn set_packed(&mut self, pos: Position, incoming: PackedCell) {
        let width = self.width as usize;
        let base = pos.y as usize * width;
        let x = pos.x as usize;
        if incoming.width() == 1 && self.cells[base + x].width() == 1 {
            self.replace_packed(base + x, incoming);
            return;
        }
        if x > 0 && self.cells[base + x].is_continuation() {
            let mut primary = x - 1;
            while primary > 0 && self.cells[base + primary].is_continuation() {
                primary -= 1;
            }
            let old = self.cells[base + primary];
            if old.is_wide() {
                self.blank_span(
                    base + primary,
                    base + (primary + old.width() as usize).min(width),
                    old,
                );
            } else if old.is_continuation() {
                self.replace_packed(base + x, PackedCell::BLANK);
            }
        }
        let old = self.cells[base + x];
        if old.is_wide() {
            self.blank_span(
                base + x + 1,
                base + (x + old.width() as usize).min(width),
                old,
            );
        }
        let end = x + incoming.width() as usize;
        if incoming.is_wide() {
            if end > width {
                self.blank_span(base + x, base + x + 1, incoming);
                return;
            }
            let continuation = self.resources.continuation(incoming);
            for i in x + 1..end {
                let old = self.cells[base + i];
                if old.is_wide() {
                    self.blank_span(
                        base + i + 1,
                        base + (i + old.width() as usize).min(width),
                        old,
                    );
                }
                self.replace_packed(base + i, continuation);
            }
            self.resources.release(continuation);
        }
        self.replace_packed(base + x, incoming);
    }
    /// Resize the grid and transfer the overlapping cells without remeasurement.
    ///
    /// A primary at a clipped edge keeps its explicit width.
    pub fn resize(&mut self, width: u16, height: u16) {
        if self.width == width && self.height == height {
            return;
        }
        if self.width == width {
            let total = width as usize * height as usize;
            for &cell in &self.cells[total.min(self.cells.len())..] {
                self.resources.release(cell);
            }
            self.cells.resize(total, PackedCell::BLANK);
            self.height = height;
            return;
        }
        let mut cells = vec![PackedCell::BLANK; width as usize * height as usize];
        for (index, cell) in self.cells.drain(..).enumerate() {
            let x = index % self.width as usize;
            let y = index / self.width as usize;
            if x < width as usize && y < height as usize {
                cells[y * width as usize + x] = cell;
            } else {
                self.resources.release(cell);
            }
        }
        self.cells = cells;
        self.width = width;
        self.height = height;
    }
}
impl Bounded for Buffer {
    fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }
}
impl Surface for Buffer {
    #[inline]
    fn cell(&self, pos: Position) -> Option<CellRef<'_>> {
        if pos.x >= self.width || pos.y >= self.height {
            return None;
        }
        Some(CellRef::packed(
            &self.cells[pos.y as usize * self.width as usize + pos.x as usize],
            &self.resources,
        ))
    }
    fn width_mode(&self) -> WidthMode {
        self.width_mode
    }
    fn eaw_wide(&self) -> bool {
        self.eaw_wide
    }
}
impl SurfaceMut for Buffer {
    fn set_cell(&mut self, pos: Position, cell: CellRef<'_>) {
        self.set(pos, cell);
    }
    fn set_style(&mut self, pos: Position, style: Style) {
        if pos.x >= self.width || pos.y >= self.height {
            return;
        }
        let index = pos.y as usize * self.width as usize + pos.x as usize;
        self.resources
            .set_style(&mut self.cells[index], StyleParts::from_style(&style));
    }
    fn fill_rect(&mut self, rect: Rect, cell: CellRef<'_>) {
        let clipped = self.bounds().intersection(rect);
        if clipped.is_empty() {
            return;
        }
        let incoming = self.resources.import(cell);
        let blank =
            self.resources
                .intern_parts(crate::cell::storage::Glyph::Scalar(' '), 1, cell.parts());
        let step = cell.width().max(1) as u16;
        for y in clipped.top()..clipped.bottom() {
            let mut x = clipped.left();
            while !overruns(x, step, clipped.right()) {
                self.set_packed(Position::new(x, y), incoming);
                x += step;
            }
            while x < clipped.right() {
                self.set_packed(Position::new(x, y), blank);
                x += 1;
            }
        }
        self.resources.release(blank);
        self.resources.release(incoming);
    }
    fn set_style_rect(&mut self, rect: Rect, style: Style) {
        let rect = self.bounds().intersection(rect);
        if rect.is_empty() {
            return;
        }
        let hold = self.resources.intern_parts(
            crate::cell::storage::Glyph::Scalar(' '),
            1,
            StyleParts::from_style(&style),
        );
        for y in rect.top()..rect.bottom() {
            for x in rect.left()..rect.right() {
                let i = y as usize * self.width as usize + x as usize;
                self.resources.copy_style(&mut self.cells[i], hold);
            }
        }
        self.resources.release(hold);
    }
    fn insert_lines(&mut self, y: u16, n: u16, bottom: u16, fill: CellRef<'_>) {
        Buffer::insert_lines(self, y, n, bottom, fill);
    }
    fn delete_lines(&mut self, y: u16, n: u16, bottom: u16, fill: CellRef<'_>) {
        Buffer::delete_lines(self, y, n, bottom, fill);
    }
    fn insert_cells(&mut self, pos: Position, n: u16, right: u16, fill: CellRef<'_>) {
        Buffer::insert_cells(self, pos, n, right, fill);
    }
    fn delete_cells(&mut self, pos: Position, n: u16, right: u16, fill: CellRef<'_>) {
        Buffer::delete_cells(self, pos, n, right, fill);
    }
}
impl TextSurface for Buffer {}
