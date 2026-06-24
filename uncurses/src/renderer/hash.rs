//! Content hashing shared by the frame and scroll stages.

use std::hash::{Hash, Hasher};

use rustc_hash::FxHasher;

use crate::cell::Cell;

/// Hash a line's content (ignoring style) for scroll detection.
///
/// Uses a non-cryptographic hasher because the only consequence of an
/// unlucky collision is a missed scroll opportunity — the affected
/// rows fall through to direct redraw, never visual corruption.
pub(crate) fn hash_line(line: &[Cell]) -> u64 {
    let mut hasher = FxHasher::default();
    for cell in line {
        cell.content().hash(&mut hasher);
    }
    hasher.finish()
}
