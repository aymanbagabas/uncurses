pub mod ops;
pub mod surface;
pub mod view;
pub mod window;

mod line;
#[cfg(test)]
mod tests;

pub use line::{Line, blank_line, fill_line, fill_line_into};
pub use surface::{Bounded, Surface, SurfaceMut};
pub use view::View;
pub use window::Window;

use crate::cell::{Cell, CellKind};
use crate::layout::{Position, Rect};
use crate::style::Style;

/// A 2D grid of terminal cells stored in row-major order with a fixed
/// stride of `width`. Row `y` lives at `cells[y * width..(y + 1) * width]`.
/// The flat layout collapses what was a `Vec<Vec<Cell>>` into a single
/// heap allocation: one indirection for the whole grid instead of one
/// per row, contiguous cells across rows for the prefetcher, and a
/// cheaper `Clone` impl on `Buffer`.
#[derive(Debug, Clone)]
pub struct Buffer {
    cells: Vec<Cell>,
    width: u16,
    height: u16,
}

impl Buffer {
    /// Create a new buffer filled with blank cells.
    pub fn new(width: u16, height: u16) -> Self {
        let total = (width as usize) * (height as usize);
        let cells = vec![Cell::BLANK; total];
        Self {
            cells,
            width,
            height,
        }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    /// Borrow row `y` as a slice. Returns `None` for `y >= height`.
    #[inline]
    pub fn line(&self, y: u16) -> Option<&[Cell]> {
        if y >= self.height {
            return None;
        }
        let w = self.width as usize;
        let start = (y as usize) * w;
        Some(&self.cells[start..start + w])
    }

    /// Mutably borrow row `y` as a slice. Returns `None` for `y >= height`.
    #[inline]
    pub fn line_mut(&mut self, y: u16) -> Option<&mut [Cell]> {
        if y >= self.height {
            return None;
        }
        let w = self.width as usize;
        let start = (y as usize) * w;
        Some(&mut self.cells[start..start + w])
    }

    /// Swap the contents of rows `a` and `b` in place.
    fn swap_rows(&mut self, a: usize, b: usize) {
        if a == b {
            return;
        }
        let w = self.width as usize;
        if w == 0 {
            return;
        }
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        let split = hi * w;
        let (left, right) = self.cells.split_at_mut(split);
        left[lo * w..(lo + 1) * w].swap_with_slice(&mut right[..w]);
    }

    /// Mutable handle to the cell at `pos`. Returns `None` for out-of-bounds positions.
    pub fn cell_mut(&mut self, pos: Position) -> Option<&mut Cell> {
        if pos.y >= self.height || pos.x >= self.width {
            return None;
        }
        let w = self.width as usize;
        Some(&mut self.cells[(pos.y as usize) * w + (pos.x as usize)])
    }

    /// Set a cell at the given position, handling wide-character
    /// placeholders and rect-anchor body propagation.
    ///
    /// - Writing a wide primary stamps the right-half
    ///   [`CellKind::Continuation`] placeholder automatically.
    /// - Writing a rect anchor (a `Rect`-kind cell at the rect's
    ///   top-left) stamps body placeholders across the rect's bounds.
    /// - Overwriting a rect cell with a rect of a *different* identity
    ///   blanks the old rect first so its body cells don't outlive
    ///   their anchor. Overwriting a rect cell with a non-rect cell
    ///   leaves the rest of the rect alone — callers may punch a
    ///   character through one cell of an image without destroying
    ///   the rest of it.
    pub fn set(&mut self, pos: impl Into<Position>, cell: &Cell) {
        let pos = pos.into();
        let y = pos.y as usize;
        let x = pos.x as usize;
        let width = self.width as usize;

        if pos.y >= self.height || x >= width {
            return;
        }

        // Rect anchor write: stamping a rect at its top-left first
        // blanks every cell of the area with a space carrying the
        // anchor's style (so any overlapping older rect is cleared,
        // any wide-cell pair straddling the edges is broken, and the
        // surrounding background colour matches the anchor's). Then
        // write the anchor and propagate body placeholders.
        if let CellKind::Rect(area) = cell.kind()
            && pos.x == area.x
            && pos.y == area.y
        {
            let styled_blank = Cell::narrow(" ").with_style(cell.style().clone());
            <Self as crate::buffer::surface::SurfaceMut>::fill_rect(self, area, &styled_blank);
            let line_start = (pos.y as usize) * width;
            self.cells[line_start + pos.x as usize] = cell.clone();
            self.stamp_rect_body(area);
            return;
        }

        // Rect cleanup: a rect of a different identity moving into a
        // cell currently occupied by a rect blanks the old rect first.
        // Non-rect overwrites leave the rest of the old rect alone
        // (orphan body cells — renderer treats them as opaque
        // occupied cells until something fully clears them).
        let existing_kind = self.cells[y * width + x].kind();
        if let CellKind::Rect(old_area) = existing_kind {
            let new_rect_area = match cell.kind() {
                CellKind::Rect(area) => Some(area),
                _ => None,
            };
            if matches!(new_rect_area, Some(area) if area != old_area) {
                self.blank_rect_raw(old_area);
            }
        }

        let line = &mut self.cells[y * width..(y + 1) * width];

        // If we're overwriting a wide cell's continuation, blank the primary
        // cell that owns it — but only when the *incoming* cell is not itself
        // a continuation. A continuation-to-continuation write is a no-op on
        // the cell's identity (e.g. it happens when a render buffer mirrors
        // a model buffer cell-by-cell after the primary wide cell has just
        // been written one column to the left) and must not destroy the
        // wide cell we are in the middle of mirroring.
        if x > 0 && line[x].is_continuation() && !cell.is_continuation() {
            // Walk backward to find the primary cell
            let mut pc = x - 1;
            while pc > 0 && line[pc].is_continuation() {
                pc -= 1;
            }
            // Verify we found a wide primary (only Wide cells own the
            // continuations to their right).
            if line[pc].is_wide() {
                let pw = line[pc].width() as usize;
                let end = (pc + pw).min(width);
                line[pc..end].fill(Cell::BLANK);
            } else if !line[pc].is_continuation() {
                // Buffer is corrupted — just blank the single cell
                line[x] = Cell::BLANK;
            }
        }

        // If we're overwriting the primary cell of a wide char, blank its
        // continuations. Rect cells own their body slots structurally
        // (via `stamp_rect_body`); leave those intact here.
        if line[x].is_wide() {
            let w = line[x].width() as usize;
            let end = (x + w).min(width);
            line[x + 1..end].fill(Cell::BLANK);
        }

        // Rect anchors don't share the wide-cell continuation discipline:
        // the body cells are stamped via `stamp_rect_body`, not as
        // `CellKind::Continuation` placeholders. Place the anchor and
        // return.
        if let CellKind::Rect(area) = cell.kind() {
            line[x] = cell.clone();
            if pos.x == area.x && pos.y == area.y {
                self.stamp_rect_body(area);
            }
            return;
        }

        let cell_width = cell.width() as usize;

        // If the new cell is wide, blank cells it will cover
        if cell_width > 1 {
            for i in x + 1..x + cell_width {
                if i < width {
                    // If we'd overwrite a wide cell's primary, blank its continuations
                    if line[i].is_wide() {
                        let w = line[i].width() as usize;
                        let end = (i + w).min(width);
                        line[i + 1..end].fill(Cell::BLANK);
                    }
                    line[i] = Cell::continuation();
                }
            }

            // Truncate at end of line: if wide cell doesn't fit, replace with blank
            if x + cell_width > width {
                line[x] = Cell::BLANK;
                return;
            }
        }

        line[x] = cell.clone();
    }

    /// Blank every cell of `area` via raw slice access. Used during
    /// rect cleanup to clear a departed rect's footprint without
    /// triggering recursive cleanup paths.
    fn blank_rect_raw(&mut self, area: Rect) {
        let width = self.width as usize;
        let buf_bounds = Rect::new(0, 0, self.width, self.height);
        let clipped = buf_bounds.intersection(area);
        for ry in clipped.top()..clipped.bottom() {
            let row_start = (ry as usize) * width;
            for rx in clipped.left()..clipped.right() {
                self.cells[row_start + rx as usize] = Cell::BLANK;
            }
        }
    }

    /// Stamp rect-body placeholders for every cell in `area` other
    /// Stamp rect-body placeholders across every cell of `area` other
    /// than the anchor at `(area.x, area.y)`. Body cells carry the
    /// anchor's style so the addon-owned region presents a consistent
    /// background under the renderer's ECH wipe. Cells underneath the
    /// new body (wide-cell halves, foreign rect cells, etc.) are
    /// blanked via raw slice access to avoid recursive set() calls.
    fn stamp_rect_body(&mut self, area: Rect) {
        let width = self.width as usize;
        let buf_bounds = Rect::new(0, 0, self.width, self.height);
        let clipped = buf_bounds.intersection(area);
        let anchor_style = self
            .cell(Position::new(area.x, area.y))
            .map(|c| c.style().clone())
            .unwrap_or(Style::EMPTY);
        let body = Cell::rect(area, "", anchor_style);
        for ry in clipped.top()..clipped.bottom() {
            let row_start = (ry as usize) * width;
            for rx in clipped.left()..clipped.right() {
                if rx == area.x && ry == area.y {
                    continue;
                }
                self.cells[row_start + rx as usize] = body.clone();
            }
        }
    }

    /// Collect unique rect areas of any rect cells inside `region`.
    /// Used by bulk fill paths to fully blank rects whose footprint
    /// extends beyond the fill region, and by render-buffer touch
    /// tracking to mark every row a rect spans.
    pub fn unique_rects_in(&self, region: Rect) -> Vec<Rect> {
        let mut areas: Vec<Rect> = Vec::new();
        let clipped = self.bounds().intersection(region);
        if clipped.is_empty() {
            return areas;
        }
        let width = self.width as usize;
        for ry in clipped.top()..clipped.bottom() {
            let row_start = (ry as usize) * width;
            for rx in clipped.left()..clipped.right() {
                if let CellKind::Rect(area) = self.cells[row_start + rx as usize].kind()
                    && !areas.contains(&area)
                {
                    areas.push(area);
                }
            }
        }
        areas
    }

    /// Resize the buffer, filling new cells with blanks.
    pub fn resize(&mut self, width: u16, height: u16) {
        if width == self.width && height == self.height {
            return;
        }

        let new_total = (width as usize) * (height as usize);
        let mut new_cells = vec![Cell::BLANK; new_total];

        let copy_w = width.min(self.width) as usize;
        let copy_h = height.min(self.height) as usize;
        let old_w = self.width as usize;
        let new_w = width as usize;
        for y in 0..copy_h {
            let src = y * old_w;
            let dst = y * new_w;
            new_cells[dst..dst + copy_w].clone_from_slice(&self.cells[src..src + copy_w]);
        }

        self.cells = new_cells;
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
    fn cell(&self, pos: Position) -> Option<&Cell> {
        if pos.y >= self.height || pos.x >= self.width {
            return None;
        }
        let w = self.width as usize;
        Some(&self.cells[(pos.y as usize) * w + (pos.x as usize)])
    }
}

impl SurfaceMut for Buffer {
    fn set_cell(&mut self, pos: Position, cell: &Cell) {
        self.set(pos, cell);
    }

    fn cell_mut(&mut self, pos: Position) -> Option<&mut Cell> {
        Buffer::cell_mut(self, pos)
    }

    /// Fill the clipped intersection of `rect` with `cell`. For
    /// width-1 fills this collapses the trait default's per-cell
    /// `set_cell` loop into one `slice::fill` per row, with explicit
    /// wide-cell edge-straddle cleanup at the left and right boundaries
    /// so any wide cell crossing the fill region leaves no orphan
    /// primary or continuation behind. Wide fills (`cell.width() > 1`)
    /// stay on the stepped `set_cell` path so primary/continuation
    /// pairing and the trailing-partial-slot blank are placed by the
    /// same wide-cell handling that `set` already implements.
    ///
    /// Any rect-anchored regions that intersect the fill are fully
    /// blanked first (including cells outside the fill region), so a
    /// bulk fill never leaves orphan rect bodies behind. Filling with
    /// a `Rect`-kind cell itself is not supported — those should be
    /// placed through `set` so anchor propagation can run.
    fn fill_rect(&mut self, rect: Rect, cell: &Cell) {
        let clipped = self.bounds().intersection(rect);
        if clipped.is_empty() {
            return;
        }

        debug_assert!(
            !cell.is_rect(),
            "fill_rect with a Rect-kind cell is not supported; use set()"
        );

        // Blank any rect-anchored regions that touch the fill so we
        // don't leave orphan body cells in rows the fill itself
        // doesn't visit.
        for area in self.unique_rects_in(clipped) {
            self.blank_rect_raw(area);
        }

        let step = cell.width().max(1);
        if step > 1 {
            // Stepped wide-cell fill: identical to the trait default.
            // Inlined here so the SurfaceMut::fill_rect dispatch goes
            // through this impl in both arms.
            for y in clipped.top()..clipped.bottom() {
                let mut x = clipped.left();
                while x + step <= clipped.right() {
                    self.set(Position::new(x, y), cell);
                    x += step;
                }
                while x < clipped.right() {
                    self.set(Position::new(x, y), &Cell::BLANK);
                    x += 1;
                }
            }
            return;
        }

        let lo = clipped.left() as usize;
        let hi = clipped.right() as usize;
        let row_width = self.width as usize;

        for y in clipped.top()..clipped.bottom() {
            let row_start = (y as usize) * row_width;
            let line = &mut self.cells[row_start..row_start + row_width];

            // Left-edge straddle: a wide cell whose primary sits
            // before `lo` and whose continuation falls at `lo`. Walk
            // back to the primary and blank it (and any siblings
            // outside the fill region) before the bulk fill so the
            // primary isn't left dangling once its continuations get
            // overwritten.
            if lo > 0 && line[lo].is_continuation() {
                let mut p = lo;
                while p > 0 && line[p].is_continuation() {
                    p -= 1;
                }
                if line[p].is_wide() {
                    let pw = line[p].width() as usize;
                    let end = (p + pw).min(lo);
                    for slot in &mut line[p..end] {
                        *slot = Cell::BLANK;
                    }
                }
            }

            // Right-edge straddle: a wide cell whose primary sits at
            // some `p` in `[lo, hi)` and whose continuations extend
            // past `hi - 1`. The primary itself will be overwritten by
            // the bulk fill, but the orphan continuations at
            // `[hi, p + pw)` need explicit blanking.
            if hi < row_width && line[hi].is_continuation() {
                let mut p = hi - 1;
                while p > lo && line[p].is_continuation() {
                    p -= 1;
                }
                if line[p].is_wide() && p + (line[p].width() as usize) > hi {
                    let pw = line[p].width() as usize;
                    let end = (p + pw).min(row_width);
                    for slot in &mut line[hi..end] {
                        *slot = Cell::BLANK;
                    }
                }
            }

            line[lo..hi].fill(cell.clone());
        }
    }

    fn insert_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        Buffer::insert_lines(self, y, n, bounds_bottom, fill);
    }

    fn delete_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        Buffer::delete_lines(self, y, n, bounds_bottom, fill);
    }

    fn insert_cells(&mut self, pos: Position, n: u16, bounds_right: u16, fill: &Cell) {
        Buffer::insert_cells(self, pos, n, bounds_right, fill);
    }

    fn delete_cells(&mut self, pos: Position, n: u16, bounds_right: u16, fill: &Cell) {
        Buffer::delete_cells(self, pos, n, bounds_right, fill);
    }
}
