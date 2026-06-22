//! Configurable horizontal tab stops for the renderer.
//!
//! Each column has an on/off bit indicating whether a tab character
//! lands the cursor exactly there. Default initialization sets a stop
//! every [`DEFAULT_TAB_INTERVAL`] columns starting at column 0. Apps
//! that issue `HTS` (set tab here) or `TBC` (clear tab) sequences can
//! reconfigure the stops via [`TabStops::set`] / [`TabStops::reset`].
//!
//! ## Clamped vs. unclamped next stops
//!
//! [`TabStops::next`] is for bounded surface work and clamps the "no next
//! stop" case to the right edge. [`TabStops::next_stop`] models an
//! actual terminal tab: if the next interval stop lies past the surface
//! edge, that past-edge column is returned. Forward cursor planning uses
//! the unclamped value so it never emits a tab that would overshoot the
//! requested target.

/// Default interval between tab stops — 8 columns matches `tput tabs`
/// and the assumption of `expand`/`unexpand`-style tools.
pub const DEFAULT_TAB_INTERVAL: u16 = 8;

/// Word size of the packed bitset backing `stops`.
const WORD_BITS: usize = u64::BITS as usize;

/// Horizontal tab stops for a single renderer line width.
///
/// The table stores explicit stops plus O(1) neighbor lookup caches.
/// Mutating the stop set rebuilds those caches immediately so cursor
/// planning can walk stops without scanning the row.
#[derive(Debug, Clone)]
pub struct TabStops {
    /// Packed bitset, one bit per column. The bit for column `x` is
    /// `(stops[x / 64] >> (x % 64)) & 1`. Cold path — the per-move
    /// planner reads `next_stop` / `prev_stop` directly. Used by
    /// mutators and by `is_stop`.
    stops: Vec<u64>,
    /// Number of columns covered by `stops`.
    width: u16,
    /// Precomputed result of [`TabStops::next`] for every column.
    /// Rebuilt whenever `stops` changes so the cursor planner's
    /// per-iteration lookups become an O(1) array index instead of
    /// an O(width) linear scan.
    next_stop: Vec<u16>,
    /// Precomputed unclamped next tab stop for every column — the true
    /// stop a terminal's tab advance lands on, which may lie past the
    /// right edge (unlike `next_stop`, which clamps to `width - 1`).
    /// Drives [`TabStops::next_stop`]; see ncurses' `NEXTTAB`.
    next_unclamped: Vec<u16>,
    /// Precomputed result of [`TabStops::prev`] for every column.
    prev_stop: Vec<u16>,
    interval: u16,
}

#[allow(dead_code)] // Mutators reserved for future HTS / TBC handling.
impl TabStops {
    /// Build a tab-stop table.
    ///
    /// # Parameters
    ///
    /// - `width`: number of columns covered by the table.
    /// - `interval`: default spacing between tab stops. Values below
    ///   `1` are treated as `1`.
    ///
    /// # Returns
    ///
    /// A table with default stops at columns that are multiples of
    /// `interval`, starting at column `0`.
    pub fn new(width: u16, interval: u16) -> Self {
        let interval = interval.max(1);
        let words = width.div_ceil(WORD_BITS as u16) as usize;
        let mut s = Self {
            stops: vec![0u64; words],
            width,
            next_stop: Vec::new(),
            next_unclamped: Vec::new(),
            prev_stop: Vec::new(),
            interval,
        };
        s.init_default(0, width);
        s.rebuild_neighbor_tables();
        s
    }

    /// Build a table with the default [`DEFAULT_TAB_INTERVAL`].
    ///
    /// # Parameters
    ///
    /// - `width`: number of columns covered by the table.
    pub fn default_for(width: u16) -> Self {
        Self::new(width, DEFAULT_TAB_INTERVAL)
    }

    /// Resize the table to a new width.
    ///
    /// Keeps existing stops in the overlapping region. When growing,
    /// initializes default stops in the newly added region. When
    /// shrinking, clears stale bits beyond the new width before
    /// rebuilding neighbor caches.
    pub fn resize(&mut self, width: u16) {
        let old = self.width;
        if width == old {
            return;
        }
        let words = width.div_ceil(WORD_BITS as u16) as usize;
        self.stops.resize(words, 0);
        // When shrinking, mask off bits past the new width in the
        // last word so future scans don't see stale stops.
        if width < old {
            let last_bit = width as usize;
            if last_bit < self.stops.len() * WORD_BITS {
                let word_idx = last_bit / WORD_BITS;
                let bit_idx = last_bit % WORD_BITS;
                if let Some(w) = self.stops.get_mut(word_idx) {
                    let mask = (1u64 << bit_idx).wrapping_sub(1);
                    *w &= mask;
                }
                for w in &mut self.stops[word_idx + 1..] {
                    *w = 0;
                }
            }
        }
        self.width = width;
        if width > old {
            self.init_default(old, width);
        }
        self.rebuild_neighbor_tables();
    }

    /// Return the current width in columns.
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Return whether `x` is a tab stop.
    ///
    /// Out-of-range positions return `false`. `x` is a zero-based column
    /// index.
    pub fn is_stop(&self, x: u16) -> bool {
        if x >= self.width {
            return false;
        }
        let idx = (x as usize) / WORD_BITS;
        let bit = (x as usize) % WORD_BITS;
        (self.stops[idx] >> bit) & 1 != 0
    }

    /// Return the next in-bounds tab stop strictly after `x`.
    ///
    /// Returns the right edge (`width - 1`) when no further stop exists.
    /// This is the clamped helper; cursor planning for literal tabs uses
    /// [`TabStops::next_stop`] instead.
    pub fn next(&self, x: u16) -> u16 {
        let w = self.width();
        if w == 0 {
            return x;
        }
        // Cached lookup. For out-of-range `x` (callers pass arbitrary
        // columns), saturate to the last entry which already encodes
        // "no further stop → right edge".
        let idx = (x as usize).min(self.next_stop.len() - 1);
        self.next_stop[idx]
    }

    /// The true next tab stop strictly after `x`, *unclamped*: it may lie
    /// past the right edge, exactly as a terminal's tab advance would land
    /// past the last in-bounds stop (a tab from the last interior stop
    /// goes to the next interval stop, not the screen edge). Mirrors
    /// ncurses' `NEXTTAB`. The cursor planner uses this so it never emits a
    /// tab unless the tab genuinely lands at or before the target column.
    pub fn next_stop(&self, x: u16) -> u16 {
        let interval = self.interval;
        if x >= self.width {
            return (x / interval + 1) * interval;
        }
        self.next_unclamped[x as usize]
    }

    /// Previous tab stop strictly before `x`. Returns 0 when no
    /// earlier stop exists or when there are no stops at all. `x` is
    /// 0-based; values past the configured width are clamped first so
    /// callers can pass any column without risking an out-of-bounds
    /// index.
    pub fn prev(&self, x: u16) -> u16 {
        if x == 0 {
            return 0;
        }
        let len = self.prev_stop.len();
        if len == 0 {
            return 0;
        }
        let idx = (x as usize).min(len - 1);
        self.prev_stop[idx]
    }

    /// Add a tab stop at `x`.
    ///
    /// Out-of-range positions are ignored. Rebuilds neighbor caches.
    pub fn set(&mut self, x: u16) {
        if x >= self.width {
            return;
        }
        let idx = (x as usize) / WORD_BITS;
        let bit = (x as usize) % WORD_BITS;
        self.stops[idx] |= 1u64 << bit;
        self.rebuild_neighbor_tables();
    }

    /// Remove the tab stop at `x`.
    ///
    /// Out-of-range positions are ignored. Rebuilds neighbor caches.
    pub fn reset(&mut self, x: u16) {
        if x >= self.width {
            return;
        }
        let idx = (x as usize) / WORD_BITS;
        let bit = (x as usize) % WORD_BITS;
        self.stops[idx] &= !(1u64 << bit);
        self.rebuild_neighbor_tables();
    }

    /// Remove all tab stops and rebuild neighbor caches.
    pub fn clear(&mut self) {
        for w in &mut self.stops {
            *w = 0;
        }
        self.rebuild_neighbor_tables();
    }

    fn init_default(&mut self, from: u16, to: u16) {
        let interval = self.interval as usize;
        let mut x = from as usize;
        while x < to as usize {
            if x.is_multiple_of(interval) {
                let idx = x / WORD_BITS;
                let bit = x % WORD_BITS;
                self.stops[idx] |= 1u64 << bit;
            }
            x += 1;
        }
    }

    /// Recompute `next_stop` / `prev_stop` from `stops`. O(width)
    /// once per mutation; the planner's per-iteration `next`/`prev`
    /// calls then run in O(1).
    fn rebuild_neighbor_tables(&mut self) {
        let w = self.width as usize;
        self.next_stop.resize(w, 0);
        self.next_unclamped.resize(w, 0);
        self.prev_stop.resize(w, 0);
        if w == 0 {
            return;
        }
        let last = (w - 1) as u16;
        let interval = self.interval;

        // Forward sweep: scan right-to-left so each entry sees the
        // nearest stop strictly to its right. `next_stop` clamps the
        // "no further stop" case to the right edge; `next_unclamped`
        // instead reports the true interval stop past the edge.
        let mut nearest: u16 = last;
        let mut nearest_real: Option<u16> = None;
        for x in (0..w).rev() {
            self.next_stop[x] = nearest;
            self.next_unclamped[x] = match nearest_real {
                Some(s) => s,
                None => (x as u16 / interval + 1) * interval,
            };
            if self.bit(x) {
                nearest = x as u16;
                nearest_real = Some(x as u16);
            }
        }

        // Backward sweep: scan left-to-right so each entry sees the
        // nearest stop strictly to its left.
        let mut nearest: u16 = 0;
        for x in 0..w {
            self.prev_stop[x] = nearest;
            if self.bit(x) {
                nearest = x as u16;
            }
        }
    }

    #[inline]
    fn bit(&self, x: usize) -> bool {
        let idx = x / WORD_BITS;
        let bit = x % WORD_BITS;
        (self.stops[idx] >> bit) & 1 != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_stops_every_eight() {
        let ts = TabStops::default_for(32);
        for c in 0..32 {
            assert_eq!(ts.is_stop(c), c.is_multiple_of(8), "col {c}");
        }
    }

    #[test]
    fn next_returns_following_stop() {
        let ts = TabStops::default_for(32);
        assert_eq!(ts.next(0), 8);
        assert_eq!(ts.next(7), 8);
        assert_eq!(ts.next(8), 16);
        assert_eq!(ts.next(30), 31);
    }

    #[test]
    fn next_stop_is_unclamped_past_the_edge() {
        // Width 46: stops at 0,8,..,40. `next` clamps "no further stop" to
        // the edge (45), but `next_stop` reports the true interval stop
        // past the edge (48) — what a real terminal's tab would reach.
        let ts = TabStops::default_for(46);
        assert_eq!(ts.next(24), 32);
        assert_eq!(ts.next_stop(24), 32); // real in-range stop: same
        assert_eq!(ts.next(40), 45); // clamped to the right edge
        assert_eq!(ts.next_stop(40), 48); // true next stop, unclamped
        assert_eq!(ts.next_stop(41), 48);
        assert_eq!(ts.next_stop(45), 48);
        // Out-of-range still follows the interval.
        assert_eq!(ts.next_stop(46), 48);
        assert_eq!(ts.next_stop(48), 56);
    }

    #[test]
    fn prev_returns_preceding_stop() {
        let ts = TabStops::default_for(32);
        assert_eq!(ts.prev(0), 0);
        assert_eq!(ts.prev(1), 0);
        assert_eq!(ts.prev(8), 0);
        assert_eq!(ts.prev(9), 8);
        assert_eq!(ts.prev(17), 16);
    }

    #[test]
    fn set_and_clear_individual_stops() {
        let mut ts = TabStops::default_for(16);
        ts.reset(8);
        assert!(!ts.is_stop(8));
        assert_eq!(ts.next(0), 15); // 8 cleared, next is right edge
        ts.set(5);
        assert!(ts.is_stop(5));
        assert_eq!(ts.next(0), 5);
        ts.clear();
        for c in 0..16 {
            assert!(!ts.is_stop(c));
        }
    }

    #[test]
    fn resize_preserves_existing_stops() {
        let mut ts = TabStops::default_for(16);
        ts.set(5);
        ts.resize(32);
        assert!(ts.is_stop(5));
        assert!(ts.is_stop(24));
    }

    #[test]
    fn prev_does_not_panic_on_zero_width() {
        let ts = TabStops::new(0, DEFAULT_TAB_INTERVAL);
        assert_eq!(ts.prev(0), 0);
        assert_eq!(ts.prev(5), 0);
        assert_eq!(ts.prev(u16::MAX), 0);
    }

    #[test]
    fn prev_clamps_x_above_width() {
        let ts = TabStops::default_for(16);
        // 8 is a stop, 16 is past width; clamping should land on the
        // last in-range stop without panicking.
        assert_eq!(ts.prev(u16::MAX), 8);
        assert_eq!(ts.prev(16), 8);
    }

    // --- additional tab-stop coverage ---
    //
    // Covers:
    // * Custom-interval `TabStops::new(_, n)` constructor: stops are
    //   placed every `n` columns; see
    //   `tabstops_custom_interval_is_stop`.
    // * Default basics and Set/Reset: see `default_stops_every_eight`
    //   and `set_and_clear_individual_stops`.
    // * Navigation (`next` / `prev`): see `next_returns_following_stop`
    //   and `prev_returns_preceding_stop`.
    // * Full clear: see `tabstops_clear_unsets_all_default_stops`.
    // * Resize (grow / shrink / custom-interval grow): see
    //   `resize_preserves_existing_stops` plus the additional
    //   `tabstops_resize_*` cases below for the edge cases.

    #[test]
    fn tabstops_custom_interval_is_stop() {
        let mut ts = TabStops::new(16, 4);
        for &(col, want) in &[
            (0u16, true),
            (3, false),
            (4, true),
            (7, false),
            (8, true),
            (12, true),
            (15, false),
        ] {
            assert_eq!(ts.is_stop(col), want, "col {col}");
        }

        let custom = 5;
        ts.set(custom);
        assert!(ts.is_stop(custom));

        let regular = 4;
        ts.reset(regular);
        assert!(!ts.is_stop(regular));
    }

    #[test]
    fn tabstops_default_interval_is_eight() {
        assert_eq!(DEFAULT_TAB_INTERVAL, 8);
        let ts = TabStops::default_for(24);
        for &(col, want) in &[
            (0u16, true),
            (7, false),
            (8, true),
            (15, false),
            (16, true),
            (23, false),
        ] {
            assert_eq!(ts.is_stop(col), want, "col {col}");
        }
    }

    #[test]
    fn tabstops_clear_unsets_all_default_stops() {
        let mut ts = TabStops::default_for(24);
        assert!(ts.is_stop(0) && ts.is_stop(8) && ts.is_stop(16));
        ts.clear();
        for c in 0..24 {
            assert!(!ts.is_stop(c), "col {c} still set after clear");
        }
    }

    #[test]
    fn tabstops_resize_grow_initializes_new_default_stops() {
        let mut ts = TabStops::default_for(16);
        ts.resize(24);
        assert_eq!(ts.width(), 24);
        for &(col, want) in &[(0u16, true), (8, true), (16, true), (23, false)] {
            assert_eq!(ts.is_stop(col), want, "col {col}");
        }
    }

    #[test]
    fn tabstops_resize_same_size_is_noop() {
        let mut ts = TabStops::default_for(16);
        ts.resize(16);
        assert_eq!(ts.width(), 16);
        assert!(ts.is_stop(0));
        assert!(ts.is_stop(8));
        assert!(!ts.is_stop(15));
    }

    #[test]
    fn tabstops_resize_with_custom_interval_keeps_interval() {
        let mut ts = TabStops::new(8, 4);
        ts.resize(16);
        assert_eq!(ts.width(), 16);
        for &(col, want) in &[(0u16, true), (4, true), (8, true), (12, true), (15, false)] {
            assert_eq!(ts.is_stop(col), want, "col {col}");
        }
    }

    #[test]
    fn tabstops_resize_to_zero_drops_all_stops() {
        let mut ts = TabStops::default_for(8);
        ts.resize(0);
        assert_eq!(ts.width(), 0);
        assert!(!ts.is_stop(0));
    }

    #[test]
    fn tabstops_resize_to_very_large_keeps_default_interval() {
        let mut ts = TabStops::default_for(8);
        ts.resize(1000);
        assert!(ts.is_stop(992));
        assert!(!ts.is_stop(999));
    }

    #[test]
    fn tabstops_multiple_resizes_keep_first_stop_set() {
        let mut ts = TabStops::default_for(8);
        for size in [16u16, 8, 24, 4] {
            ts.resize(size);
            assert_eq!(ts.width(), size);
            assert!(ts.is_stop(0), "first stop missing after resize to {size}");
        }
    }
}
