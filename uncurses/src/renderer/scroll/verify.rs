//! Post-scroll bookkeeping: keep `old_hashes` aligned with the
//! now-shifted `cur_buf` so the next frame's scroll detection sees
//! the actual content rather than stale or zeroed entries.

use crate::renderer::Renderer;
use crate::renderer::frame::prepare::hash_line;

/// Recompute `old_hashes[start..end]` from cur_buf. Used after a
/// hardscroll to give the rows that the scroll byte blanked an
/// accurate hash (a fresh blank-line hash) instead of leaving them at
/// a shifted-in stale value or a placeholder zero.
///
/// The rows in `[start..end)` were just filled by `delete_lines` /
/// `insert_lines` with the same `blank` cell across the same width,
/// so every row in the range is byte-identical. Hash the first row
/// once and broadcast the value across the range. A `debug_assert!`
/// on the second row guards the invariant during test builds.
pub(super) fn recompute_blank_hashes(renderer: &mut Renderer, start: usize, end: usize) {
    let Some(cb) = renderer.cur_buf.as_ref() else {
        return;
    };
    let limit = end.min(renderer.old_hashes.len());
    if start >= limit {
        return;
    }
    let Some(first) = cb.line(start as u16) else {
        return;
    };
    let h = hash_line(first);
    debug_assert!(
        (start + 1..limit)
            .filter_map(|y| cb.line(y as u16))
            .all(|l| hash_line(l) == h),
        "recompute_blank_hashes: rows in [{start}..{limit}) expected to be identical blanks"
    );
    renderer.old_hashes[start..limit].fill(h);
}

/// Shift `old_hashes[top..bottom]` upward by `n` rows; zero the freed
/// entries at the bottom that now correspond to blanked lines.
pub(super) fn shift_old_hashes_up(old_hashes: &mut [u64], top: usize, bottom: usize, n: usize) {
    if n == 0 || bottom <= top || bottom > old_hashes.len() {
        return;
    }
    let region_len = bottom - top;
    let shift = n.min(region_len);
    old_hashes.copy_within(top + shift..bottom, top);
    old_hashes[bottom - shift..bottom].fill(0);
}

/// Shift `old_hashes[top..bottom]` downward by `n` rows; zero the freed
/// entries at the top that now correspond to blanked lines.
pub(super) fn shift_old_hashes_down(old_hashes: &mut [u64], top: usize, bottom: usize, n: usize) {
    if n == 0 || bottom <= top || bottom > old_hashes.len() {
        return;
    }
    let region_len = bottom - top;
    let shift = n.min(region_len);
    old_hashes.copy_within(top..bottom - shift, top + shift);
    old_hashes[top..top + shift].fill(0);
}
