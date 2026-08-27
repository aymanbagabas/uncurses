//! Hash-based scroll detection.
//!
//! Detects scrolled content by matching line content hashes between
//! old and new buffers. Finds unique 1:1 matches and grows them into
//! contiguous hunks. When the renderer has a previous-frame buffer,
//! growth past hash mismatches is allowed whenever the per-cell cost
//! of scrolling-then-patching beats redrawing in place -- but only
//! under synchronized output.
//!
//! A terminal scroll is always full width, so growing a hunk through a
//! row the scroll gets wrong means emitting the scroll and then
//! repainting the cells it moved but shouldn't have. Inside a
//! synchronized frame that correction is invisible and the byte saving
//! is free. Outside one it is on screen for a moment: a fixed sidebar
//! beside a scrolling pane visibly slides and snaps back.
//!
//! So without synchronized output a row joins a hunk only when the
//! scroll delivers it exactly, which [`Renderer::delivers_exactly`]
//! decides by comparing live cells. Row hashes cannot answer that: they
//! cover content alone, so a cell differing only in style hashes equal,
//! and [`prepare`](crate::renderer::frame) leaves a row's hash stale
//! when the new frame does not touch it. The hash still runs first as a
//! cheap filter -- equal cells always hash equal, so it never rejects a
//! row the cell comparison would have accepted.

use crate::cell::Cell;
use crate::renderer::Renderer;
use crate::renderer::buffer::RenderBuffer;

/// Sentinel for "no mapping" in the `oldnum` table.
const NO_MAP: i32 = -1;

/// Entry in the linear-probe matching table. `hash == 0` flags an
/// unused slot; a row whose content hashes to exactly 0 (vanishingly
/// rare) is silently skipped — the worst case is one missed scroll
/// opportunity.
#[derive(Default, Clone)]
pub(crate) struct HashEntry {
    pub(crate) hash: u64,
    pub(crate) old_count: u16,
    pub(crate) new_count: u16,
    pub(crate) old_idx: i32,
    pub(crate) new_idx: i32,
}

/// Linear-probe lookup from slot 0. Returns the first slot whose
/// hash matches `hashval`, or the first empty slot, or `table.len()`
/// if the table is full.
fn probe(table: &[HashEntry], hashval: u64) -> usize {
    let mut idx = 0;
    while idx < table.len() && table[idx].hash != 0 {
        if table[idx].hash == hashval {
            return idx;
        }
        idx += 1;
    }
    idx
}

impl Renderer {
    /// Build the `oldnum` mapping: `oldnum[new_y] = old_y`, or `-1`
    /// if no source row maps to `new_y`. Reads `self.old_hashes` /
    /// `self.new_hashes`, writes `self.oldnum`.
    ///
    /// When `self.cur_buf` is present, growth may extend through hash
    /// mismatches if the per-cell cost analysis shows scrolling still
    /// wins; without it, only hash matches grow hunks.
    pub(crate) fn update_hashmap(&mut self, new_buf: &RenderBuffer, height: usize) {
        self.oldnum.clear();
        self.oldnum.resize(height, NO_MAP);

        if self.old_hashes.len() != height || self.new_hashes.len() != height {
            return;
        }

        // Linear-probe scratch table sized so even the worst case of
        // 2*H distinct hashes fits with slack. For typical terminal
        // heights this beats a HashMap because u64 comparisons cost
        // a couple of cycles vs HashMap's bucket math and SipHash.
        let table_size = (height + 1) * 2;
        self.hashtab.clear();
        self.hashtab.resize(table_size, HashEntry::default());

        for (i, &hashval) in self.old_hashes.iter().enumerate() {
            if hashval == 0 {
                continue;
            }
            let idx = probe(&self.hashtab, hashval);
            if idx >= self.hashtab.len() {
                continue;
            }
            let entry = &mut self.hashtab[idx];
            entry.hash = hashval;
            entry.old_count += 1;
            entry.old_idx = i as i32;
        }
        for (i, &hashval) in self.new_hashes.iter().enumerate() {
            if hashval == 0 {
                continue;
            }
            let idx = probe(&self.hashtab, hashval);
            if idx >= self.hashtab.len() {
                continue;
            }
            let entry = &mut self.hashtab[idx];
            entry.hash = hashval;
            entry.new_count += 1;
            entry.new_idx = i as i32;
        }

        // All used slots are contiguous from index 0 (linear probe
        // fills empties in order), so we can stop at the first empty
        // slot rather than walking the whole table.
        for entry in self.hashtab.iter() {
            if entry.hash == 0 {
                break;
            }
            // Skip identity mappings (same hash at same row): they
            // contribute no scroll work and would just be wasted
            // walks in grow_hunks.
            if entry.old_count == 1 && entry.new_count == 1 && entry.old_idx != entry.new_idx {
                let new_idx = entry.new_idx as usize;
                let old_idx = entry.old_idx;
                // A seed is trusted on its hash alone under synchronized
                // output. Without it, confirm against live cells: the hash
                // is content-only and can be stale.
                if new_idx < height
                    && (self.sync_output
                        || self.delivers_exactly(new_buf, old_idx as usize, new_idx))
                {
                    self.oldnum[new_idx] = old_idx;
                }
            }
        }

        self.grow_hunks(new_buf, height);

        // Eliminate hunks that aren't worth scrolling. Two checks per
        // hunk (a contiguous run of rows with the same shift):
        //   1. Tiny hunks (`size < 3`) cost more to scroll than to
        //      redraw.
        //   2. A small hunk being shifted a long distance ("destroying
        //      more than carrying") is invalidated.
        invalidate_bad_hunks(&mut self.oldnum, height);

        // Re-grow after invalidation: hunks adjacent to now-invalidated
        // entries may extend further.
        self.grow_hunks(new_buf, height);
    }

    /// Expand matched rows into contiguous hunks, walking backward
    /// then forward from each seed with anti-overlap limits. A row
    /// is added when either hashes match or the cell-aware
    /// [`Renderer::cost_effective`] check says the post-scroll cost
    /// is no worse than a redraw.
    fn grow_hunks(&mut self, new_buf: &RenderBuffer, height: usize) {
        let h = height as i32;
        let mut back_limit: i32 = 0;
        let mut back_ref_limit: i32 = 0;
        let mut i: i32 = 0;

        while i < h && self.oldnum[i as usize] == NO_MAP {
            i += 1;
        }

        while i < h {
            let start = i;
            let shift = self.oldnum[start as usize] - start;

            i = start + 1;
            while i < h && self.oldnum[i as usize] != NO_MAP && self.oldnum[i as usize] - i == shift
            {
                i += 1;
            }
            let end = i;

            while i < h && self.oldnum[i as usize] == NO_MAP {
                i += 1;
            }
            let next_hunk = i;

            let forward_limit = i;
            let forward_ref_limit = if i >= h || self.oldnum[i as usize] >= i {
                i
            } else {
                self.oldnum[i as usize]
            };

            // Grow back from start - 1, bounded by back_limit
            // (adjusted upward when shift is negative so a scroll-up
            // hunk can't claim rows that the previous hunk's
            // destination already covers).
            let mut j = start - 1;
            let bl = if shift < 0 {
                back_ref_limit + (-shift)
            } else {
                back_limit
            };
            while j >= bl {
                let target = j + shift;
                if target < 0 || target >= h {
                    break;
                }
                let hash_match = self.new_hashes[j as usize] == self.old_hashes[target as usize];
                let ok = if self.sync_output {
                    hash_match
                        || self.cost_effective(new_buf, target as usize, j as usize, shift < 0)
                } else {
                    hash_match && self.delivers_exactly(new_buf, target as usize, j as usize)
                };
                if !ok {
                    break;
                }
                self.oldnum[j as usize] = target;
                j -= 1;
            }

            // Grow forward from end, bounded by forward_limit
            // (reduced when shift is positive for the symmetric
            // scroll-down case).
            j = end;
            let fl = if shift > 0 {
                forward_ref_limit - shift
            } else {
                forward_limit
            };
            while j < fl {
                let target = j + shift;
                if target < 0 || target >= h {
                    break;
                }
                let hash_match = self.new_hashes[j as usize] == self.old_hashes[target as usize];
                let ok = if self.sync_output {
                    hash_match
                        || self.cost_effective(new_buf, target as usize, j as usize, shift > 0)
                } else {
                    hash_match && self.delivers_exactly(new_buf, target as usize, j as usize)
                };
                if !ok {
                    break;
                }
                self.oldnum[j as usize] = target;
                j += 1;
            }

            back_limit = j;
            back_ref_limit = back_limit;
            if shift > 0 {
                back_ref_limit += shift;
            }

            i = next_hunk;
        }
    }

    /// Whether scrolling content from old-row `from` to new-row `to`
    /// is cheaper than redrawing both rows in place. `blank`
    /// indicates the destination of the move will leave a blank row
    /// that BCE can fill, changing the accounting on the source side.
    ///
    /// Whether scrolling the row now at `from` to position `to` lands
    /// exactly the cells this frame wants there, leaving nothing to
    /// correct afterwards.
    ///
    /// This is the condition for moving a row with no visible artifact:
    /// the scroll puts the right cells in place on the first try, so no
    /// repaint follows it and there is no intermediate state to see.
    ///
    /// Compares live [`Cell`]s rather than row hashes, which answer a
    /// weaker question -- they cover content alone, and go stale for rows
    /// the new frame does not touch. Stops at the first mismatch.
    fn delivers_exactly(&self, new_buf: &RenderBuffer, from: usize, to: usize) -> bool {
        let Some(old) = self.cur_buf.as_ref() else {
            return false;
        };
        match (old.line(from as u16), new_buf.line(to as u16)) {
            (Some(a), Some(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x == y),
            _ => false,
        }
    }

    /// Returns `false` whenever the renderer has no previous frame
    /// to compare against (the cost analysis needs both buffers).
    fn cost_effective(
        &mut self,
        new_buf: &RenderBuffer,
        from: usize,
        to: usize,
        blank: bool,
    ) -> bool {
        if from == to {
            return false;
        }
        if self.cur_buf.is_none() {
            return false;
        }

        let width = new_buf.width() as usize;
        // Split-borrow: `self.cur` is disjoint from `self.cur_buf` and
        // `self.oldnum`, so the blank template ref stays live across
        // the cost-helper calls below.
        let clear_blank: &Cell = self.cur.current_blank();
        let old_buf = self.cur_buf.as_ref().unwrap();

        let new_from_signed = self.oldnum.get(from).copied().unwrap_or(NO_MAP);
        let new_from = if new_from_signed == NO_MAP {
            from
        } else {
            new_from_signed as usize
        };

        // Cost before moving: repaint the destination in place (or
        // from blank when the source row would become blank), plus
        // repaint the source row in place.
        let mut cost_before = if blank {
            update_cost_blank(clear_blank, new_buf.line(to as u16), width)
        } else {
            update_cost(old_buf.line(to as u16), new_buf.line(to as u16), width)
        };
        cost_before += update_cost(
            old_buf.line(new_from as u16),
            new_buf.line(from as u16),
            width,
        );

        // Cost after moving: source either becomes blank (when
        // nothing else maps onto it) or gets the next mapped row's
        // content; destination gets the scrolled source content.
        let mut cost_after = if new_from == from {
            update_cost_blank(clear_blank, new_buf.line(from as u16), width)
        } else {
            update_cost(
                old_buf.line(new_from as u16),
                new_buf.line(from as u16),
                width,
            )
        };
        cost_after += update_cost(old_buf.line(from as u16), new_buf.line(to as u16), width);

        cost_before >= cost_after
    }
}

/// Invalidate hunks that are too small or shifted too far. A "hunk"
/// here is a maximal contiguous run of rows that all share the same
/// shift (`oldnum[i] - i`). Identity mappings (`oldnum[i] == i`) are
/// filtered upstream in [`Renderer::update_hashmap`], so every hunk
/// reaching this pass has a non-zero shift.
///
/// A hunk is invalidated when either:
///   - it is shorter than 3 rows, or
///   - `size + min(size/8, 2) < abs(shift)` — i.e. the shift distance
///     exceeds what the hunk size can justify.
fn invalidate_bad_hunks(oldnum: &mut [i32], height: usize) {
    let mut i = 0;
    while i < height {
        if oldnum[i] == NO_MAP {
            i += 1;
            continue;
        }

        let start = i;
        let shift = oldnum[start] - start as i32;
        i += 1;
        while i < height && oldnum[i] != NO_MAP && oldnum[i] - i as i32 == shift {
            i += 1;
        }
        let size = (i - start) as i32;

        let cushion = (size / 8).min(2);
        if size < 3 || size + cushion < shift.abs() {
            oldnum[start..i].fill(NO_MAP);
        }
    }
}

/// Cell-level cost of repainting `to` from `from`: count mismatched
/// columns between the two rows up to the buffer width. Two missing
/// cells at the same index are treated as equal.
fn update_cost(old_line: Option<&[Cell]>, new_line: Option<&[Cell]>, width: usize) -> usize {
    let old: &[Cell] = old_line.unwrap_or(&[]);
    let new: &[Cell] = new_line.unwrap_or(&[]);
    let common = old.len().min(new.len()).min(width);
    let mut cost = 0;
    for i in 0..common {
        if old[i] != new[i] {
            cost += 1;
        }
    }
    // Positions where one side is shorter than the other count as a
    // mismatch each; positions past both sides cost nothing.
    let max_present = old.len().max(new.len()).min(width);
    cost + (max_present - common)
}

/// Cell-level cost of repainting `to` from a blank line, with the
/// renderer's current clear-blank as the implicit source. Counts
/// cells in `to` that differ from `clear_blank`.
fn update_cost_blank(clear_blank: &Cell, to_line: Option<&[Cell]>, width: usize) -> usize {
    let to: &[Cell] = to_line.unwrap_or(&[]);
    let common = to.len().min(width);
    let mut cost = 0;
    for c in &to[..common] {
        if c != clear_blank {
            cost += 1;
        }
    }
    // Positions past the line's length have no cell and count as a
    // mismatch (the original loop treated `Some(c) if c == blank` as
    // free and every other arm as +1).
    cost + (width - common)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::Renderer;

    /// A renderer carrying synthetic hashes and no `cur_buf`, for testing
    /// the mapping algorithm itself. Runs as if synchronized, because the
    /// unsynchronized path decides from live cells and there are none to
    /// compare here.
    fn renderer_with_hashes(old: Vec<u64>, new: Vec<u64>, height: u16) -> (Renderer, RenderBuffer) {
        let mut r = Renderer::new();
        r.sync_output = true;
        r.old_hashes = old;
        r.new_hashes = new;
        let buf = RenderBuffer::new(1, height);
        (r, buf)
    }

    #[test]
    fn test_no_scroll() {
        // When every row matches at its own index, all entries are
        // identity mappings — the unique-match pass skips them, so
        // oldnum stays all-NO_MAP and no scroll work is scheduled.
        let (mut r, buf) = renderer_with_hashes(vec![1, 2, 3, 4, 5], vec![1, 2, 3, 4, 5], 5);
        r.update_hashmap(&buf, 5);
        assert_eq!(r.oldnum, vec![NO_MAP, NO_MAP, NO_MAP, NO_MAP, NO_MAP]);
    }

    #[test]
    fn test_scroll_up_one() {
        let (mut r, buf) =
            renderer_with_hashes(vec![10, 20, 30, 40, 50], vec![20, 30, 40, 50, 60], 5);
        r.update_hashmap(&buf, 5);
        assert_eq!(r.oldnum[0], 1);
        assert_eq!(r.oldnum[1], 2);
        assert_eq!(r.oldnum[2], 3);
        assert_eq!(r.oldnum[3], 4);
        assert_eq!(r.oldnum[4], NO_MAP);
    }

    #[test]
    fn test_all_different() {
        let (mut r, buf) = renderer_with_hashes(vec![1, 2, 3], vec![4, 5, 6], 3);
        r.update_hashmap(&buf, 3);
        assert_eq!(r.oldnum, vec![NO_MAP, NO_MAP, NO_MAP]);
    }

    #[test]
    fn test_small_hunk_long_shift_invalidated() {
        let mut old = vec![0u64; 20];
        let mut new = vec![0u64; 20];
        for (i, slot) in old.iter_mut().enumerate() {
            *slot = (i as u64 + 1) * 1000;
        }
        for (i, slot) in new.iter_mut().enumerate() {
            *slot = (i as u64 + 1) * 1000 + 7;
        }
        new[15] = old[5];
        new[16] = old[6];
        let (mut r, buf) = renderer_with_hashes(old, new, 20);
        r.update_hashmap(&buf, 20);
        assert_eq!(r.oldnum[15], NO_MAP);
        assert_eq!(r.oldnum[16], NO_MAP);
    }

    #[test]
    fn test_medium_hunk_oversized_shift_invalidated() {
        let mut old = vec![0u64; 30];
        let mut new = vec![0u64; 30];
        for (i, slot) in old.iter_mut().enumerate() {
            *slot = (i as u64 + 1) * 1000;
        }
        for (i, slot) in new.iter_mut().enumerate() {
            *slot = (i as u64 + 1) * 1000 + 7;
        }
        new[20..25].copy_from_slice(&old[10..15]);
        let (mut r, buf) = renderer_with_hashes(old, new, 30);
        r.update_hashmap(&buf, 30);
        for k in 0..5 {
            assert_eq!(
                r.oldnum[20 + k],
                NO_MAP,
                "row {} should be invalidated",
                20 + k
            );
        }
    }

    #[test]
    fn test_large_hunk_long_shift_kept() {
        let mut old = vec![0u64; 30];
        let mut new = vec![0u64; 30];
        for (i, slot) in old.iter_mut().enumerate() {
            *slot = (i as u64 + 1) * 1000;
        }
        for (i, slot) in new.iter_mut().enumerate() {
            *slot = (i as u64 + 1) * 1000 + 7;
        }
        new[12..24].copy_from_slice(&old[2..14]);
        let (mut r, buf) = renderer_with_hashes(old, new, 30);
        r.update_hashmap(&buf, 30);
        for k in 0..12 {
            assert_eq!(
                r.oldnum[12 + k],
                (2 + k) as i32,
                "row {} should be kept",
                12 + k
            );
        }
    }

    #[test]
    fn test_cost_effective_extends_through_mismatched_hash() {
        // Scroll-up by one: rows 1..5 of `old` reappear at rows 0..4
        // of `new`, but row 0 of `new` has a tiny edit so its hash
        // differs from old[1]. With cell-aware growth the hunk
        // should still extend to cover row 0 because the per-cell
        // cost of scrolling and patching one cell beats a full
        // redraw -- but only under synchronized output, which hides the
        // patch.
        let width: u16 = 20;
        let height: u16 = 5;
        let mut old_buf = RenderBuffer::new(width, height);
        let mut new_buf = RenderBuffer::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let ch = char::from_u32('a' as u32 + y as u32).unwrap();
                old_buf.set_cell((x, y), &Cell::narrow(ch.to_string()));
            }
        }
        for y in 0..height {
            for x in 0..width {
                let src = if y == 0 {
                    char::from_u32('b' as u32).unwrap()
                } else if y == height - 1 {
                    char::from_u32('z' as u32).unwrap()
                } else {
                    char::from_u32('a' as u32 + (y as u32 + 1)).unwrap()
                };
                new_buf.set_cell((x, y), &Cell::narrow(src.to_string()));
            }
        }
        new_buf.set_cell((0, 0), &Cell::narrow("Z"));

        let mut old_hashes = vec![0u64; height as usize];
        let mut new_hashes = vec![0u64; height as usize];
        for y in 0..height as usize {
            old_hashes[y] = simple_hash(old_buf.line(y as u16).unwrap());
            new_hashes[y] = simple_hash(new_buf.line(y as u16).unwrap());
        }

        let mut r = Renderer::new();
        r.cur_buf = Some(old_buf.clone());
        r.old_hashes = old_hashes.clone();
        r.new_hashes = new_hashes.clone();
        r.sync_output = true;
        r.update_hashmap(&new_buf, height as usize);
        assert_eq!(
            r.oldnum[0], 1,
            "cell-aware grow should have linked new[0] to old[1]"
        );

        // Without synchronized output the same frame must not link row 0:
        // the scroll would deliver the wrong cell there and the correction
        // repaint would be visible.
        let mut r = Renderer::new();
        r.cur_buf = Some(old_buf);
        r.old_hashes = old_hashes;
        r.new_hashes = new_hashes;
        r.sync_output = false;
        r.update_hashmap(&new_buf, height as usize);
        assert_eq!(
            r.oldnum[0], NO_MAP,
            "without sync output, growth must stop at the mismatched row"
        );
    }

    /// Build a pair of buffers where `new` is `old` shifted by one row,
    /// optionally with a one-cell edit on `edit_row` so its hash misses.
    fn shifted_pair(height: u16, edit_row: Option<u16>) -> (RenderBuffer, RenderBuffer) {
        let width: u16 = 20;
        let mut old_buf = RenderBuffer::new(width, height);
        let mut new_buf = RenderBuffer::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let a = char::from_u32('a' as u32 + y as u32).unwrap();
                old_buf.set_cell((x, y), &Cell::narrow(a.to_string()));
                let src = if y == height - 1 {
                    'z'
                } else {
                    char::from_u32('a' as u32 + y as u32 + 1).unwrap()
                };
                new_buf.set_cell((x, y), &Cell::narrow(src.to_string()));
            }
        }
        if let Some(r) = edit_row {
            new_buf.set_cell((0, r), &Cell::narrow("Z"));
        }
        (old_buf, new_buf)
    }

    fn run_hashmap(old_buf: RenderBuffer, new_buf: &RenderBuffer, sync: bool) -> Vec<i32> {
        let height = new_buf.height();
        let mut old_hashes = vec![0u64; height as usize];
        let mut new_hashes = vec![0u64; height as usize];
        for y in 0..height as usize {
            old_hashes[y] = simple_hash(old_buf.line(y as u16).unwrap());
            new_hashes[y] = simple_hash(new_buf.line(y as u16).unwrap());
        }
        let mut r = Renderer::new();
        r.cur_buf = Some(old_buf);
        r.old_hashes = old_hashes;
        r.new_hashes = new_hashes;
        r.sync_output = sync;
        r.update_hashmap(new_buf, height as usize);
        r.oldnum.clone()
    }

    #[test]
    fn hash_matched_rows_still_grow_without_sync_output() {
        // The guard must not disable growth. Rows 1..3 all hold the same
        // text, so their hash is not unique and the seeding pass skips
        // them -- only growth can map them. Seeds alone would leave a
        // 2-row hunk, which `invalidate_bad_hunks` then drops, so if
        // growth stops here no scroll survives at all.
        let width: u16 = 8;
        let rows = ["A", "B", "B", "B", "C", "D"];
        let mut old_buf = RenderBuffer::new(width, 6);
        let mut new_buf = RenderBuffer::new(width, 6);
        for y in 0..6u16 {
            for x in 0..width {
                old_buf.set_cell((x, y), &Cell::narrow(rows[y as usize]));
                // `new` is `old` shifted up one row.
                let src = if y == 5 { "E" } else { rows[y as usize + 1] };
                new_buf.set_cell((x, y), &Cell::narrow(src));
            }
        }
        let guarded = run_hashmap(old_buf, &new_buf, false);
        for y in 0..5 {
            assert_eq!(
                guarded[y],
                (y + 1) as i32,
                "row {y} is reachable only by growth and must still map \
                 unsynchronized: {guarded:?}"
            );
        }
    }

    #[test]
    fn forward_growth_is_guarded_too() {
        // Growth runs backward from the seed and forward from it. The
        // backward direction is covered by the cost-effective test; this
        // pins the forward one, where the mismatched row sits *below* the
        // seed.
        let (old_buf, new_buf) = shifted_pair(8, Some(5));
        let synced = run_hashmap(old_buf.clone(), &new_buf, true);
        assert_eq!(
            synced[5], 6,
            "synchronized, forward growth crosses the edited row: {synced:?}"
        );
        let guarded = run_hashmap(old_buf, &new_buf, false);
        assert_eq!(
            guarded[5], NO_MAP,
            "unsynchronized, forward growth must stop at it: {guarded:?}"
        );
    }

    fn simple_hash(line: &[Cell]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for c in line {
            c.content().hash(&mut h);
        }
        h.finish()
    }
}
