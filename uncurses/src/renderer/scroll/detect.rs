//! Hash-based scroll detection.
//!
//! Detects scrolled content by matching line content hashes between
//! old and new buffers. Finds unique 1:1 matches and grows them into
//! contiguous hunks. When the renderer has a previous-frame buffer,
//! growth past hash mismatches is allowed whenever the per-cell cost
//! of scrolling-then-patching beats redrawing in place.

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
                if new_idx < height {
                    self.oldnum[new_idx] = entry.old_idx;
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
                let ok = hash_match
                    || self.cost_effective(new_buf, target as usize, j as usize, shift < 0);
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
                let ok = hash_match
                    || self.cost_effective(new_buf, target as usize, j as usize, shift > 0);
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

    fn renderer_with_hashes(old: Vec<u64>, new: Vec<u64>, height: u16) -> (Renderer, RenderBuffer) {
        let mut r = Renderer::new();
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
        // redraw.
        let width: u16 = 20;
        let height: u16 = 5;
        let mut old_buf = RenderBuffer::new(width, height);
        let mut new_buf = RenderBuffer::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let ch = char::from_u32('a' as u32 + y as u32).unwrap();
                old_buf.set_cell((x, y), &Cell::new(ch.to_string(), 1));
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
                new_buf.set_cell((x, y), &Cell::new(src.to_string(), 1));
            }
        }
        new_buf.set_cell((0, 0), &Cell::new("Z", 1));

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
        r.update_hashmap(&new_buf, height as usize);
        assert_eq!(
            r.oldnum[0], 1,
            "cell-aware grow should have linked new[0] to old[1]"
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
