//! Scroll optimization: detection (hash-based unique line matching)
//! and application (DECSTBM-bracketed SU/SD/IL/DL with fallback).

use std::ops::{Deref, DerefMut};

pub(super) mod apply;
pub(super) mod detect;
mod emit;
mod plan;
mod verify;

pub(crate) use detect::HashEntry;

/// Per-frame map from new-buffer row to old-buffer row.
///
/// Index by new-buffer row; the stored value is the old-buffer row
/// that matches (or `-1` when no row matched). Wraps a flat
/// `Vec<i32>` so the storage is reused across frames; the
/// implementation transparently derefs to the underlying vector.
#[derive(Debug, Default)]
pub(crate) struct ScrollMap(Vec<i32>);

impl ScrollMap {
    pub(crate) const fn new() -> Self {
        Self(Vec::new())
    }
}

impl Deref for ScrollMap {
    type Target = Vec<i32>;
    fn deref(&self) -> &Vec<i32> {
        &self.0
    }
}

impl DerefMut for ScrollMap {
    fn deref_mut(&mut self) -> &mut Vec<i32> {
        &mut self.0
    }
}

impl From<Vec<i32>> for ScrollMap {
    fn from(v: Vec<i32>) -> Self {
        Self(v)
    }
}

impl PartialEq<Vec<i32>> for ScrollMap {
    fn eq(&self, other: &Vec<i32>) -> bool {
        self.0 == *other
    }
}
