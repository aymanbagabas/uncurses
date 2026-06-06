//! External paint regions on a [`Screen`].
//!
//! A region is a rectangle of cells whose visible content is owned
//! by an external paint layer (sixel, kitty graphics, a future
//! protocol, …) plus the byte sequence that re-asserts that paint
//! each frame. The screen treats the cells inside a region as
//! [`Cell::skip`] placeholders: the cell-diff renderer emits them
//! as blank spaces and refuses cell-shifting optimizations on
//! their rows, so the painted area stays anchored to the columns
//! the caller chose.
//!
//! The screen calls into this module from
//! [`Screen::set_region`](super::Screen::set_region),
//! [`Screen::clear_region`](super::Screen::clear_region) and
//! [`Screen::clear_regions`](super::Screen::clear_regions). After
//! the cell diff, [`Screen::render`](super::Screen::render)
//! iterates the registered regions in registration order: it
//! queues a cursor move to the region's origin, then writes the
//! payload bytes raw. Callers are responsible for the cursor
//! state inside the payload — the typical pattern is to wrap with
//! `\x1b7…\x1b8` (DECSC/DECRC) so the cursor returns to the
//! anchor and stays in sync with the renderer's tracked cursor.
//!
//! ## Overlap and z-order
//!
//! Regions may overlap freely. When two regions cover the same
//! cell, the one registered later "wins" visually (its payload is
//! emitted last, so its bytes paint on top). Releasing one region
//! only blanks cells that aren't covered by another live region,
//! so a moving region doesn't poke holes in an underlying
//! stationary one.
//!
//! ## Identity
//!
//! Region ids are caller-allocated [`RegionId`] values. Two
//! distinct paint instances of the same image at different
//! positions need two distinct ids — using one id for both would
//! make the second `set_region` release the first's footprint.

use std::sync::Arc;

use crate::Rect;

/// Caller-allocated identifier for an external paint region.
///
/// Equal ids denote the same region: a second
/// [`Screen::set_region`](super::Screen::set_region) with an
/// existing id replaces that region's area and payload (releasing
/// any cells the previous footprint owned that aren't covered by
/// the new one). Distinct paint instances need distinct ids — even
/// when they paint the same pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RegionId(pub u64);

impl RegionId {
    /// Inner integer value.
    pub fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for RegionId {
    fn from(v: u64) -> Self {
        RegionId(v)
    }
}

/// One registered region: a rectangle of cells plus the byte
/// sequence emitted each frame to (re)assert the external paint.
#[derive(Debug, Clone)]
pub(crate) struct Region {
    pub area: Rect,
    pub payload: Arc<[u8]>,
}

/// Per-screen registry of external paint regions.
///
/// Entries are stored in registration order. Updating an existing
/// id keeps its slot — registration order is established by the
/// first `set_region` for a given id. Iteration in registration
/// order drives deterministic z-ordering at emission time.
#[derive(Debug, Default)]
pub(crate) struct Regions {
    entries: Vec<(RegionId, Region)>,
}

impl Regions {
    /// Lookup entry index by id.
    fn position(&self, id: RegionId) -> Option<usize> {
        self.entries.iter().position(|(i, _)| *i == id)
    }

    /// Iterate in registration order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (RegionId, &Region)> {
        self.entries.iter().map(|(id, r)| (*id, r))
    }

    /// True when no regions are registered.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Insert or replace the entry for `id`. Returns the previous
    /// entry, if any.
    pub(crate) fn insert(&mut self, id: RegionId, region: Region) -> Option<Region> {
        if let Some(pos) = self.position(id) {
            Some(std::mem::replace(&mut self.entries[pos].1, region))
        } else {
            self.entries.push((id, region));
            None
        }
    }

    /// Remove the entry for `id`. Returns it, if present.
    pub(crate) fn remove(&mut self, id: RegionId) -> Option<Region> {
        let pos = self.position(id)?;
        Some(self.entries.remove(pos).1)
    }

    /// Drain every entry into a vector. Used by
    /// [`Screen::clear_regions`](super::Screen::clear_regions) so
    /// the caller can release placeholder cells after the registry
    /// has been emptied (avoiding self-shadowing in the coverage
    /// check).
    pub(crate) fn drain(&mut self) -> Vec<(RegionId, Region)> {
        std::mem::take(&mut self.entries)
    }

    /// True when any region other than `exclude` covers `(x, y)`.
    pub(crate) fn any_other_covers(&self, exclude: RegionId, x: u16, y: u16) -> bool {
        self.entries
            .iter()
            .any(|(id, r)| *id != exclude && r.area.contains((x, y)))
    }

    /// True when any region covers `(x, y)`.
    pub(crate) fn any_covers(&self, x: u16, y: u16) -> bool {
        self.entries.iter().any(|(_, r)| r.area.contains((x, y)))
    }
}
