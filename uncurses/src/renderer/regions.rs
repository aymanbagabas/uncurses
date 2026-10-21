//! Native placements, coverage, and prepared operations in the frame pipeline.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::Arc;

use crate::ansi;
use crate::buffer::{Bounded, SurfaceMut};
use crate::cell::Cell;
use crate::layout::{Position, Rect, Size};
use crate::renderer::{RenderBuffer, Renderer};
use crate::screen::{
    AnchoredRegion, PreparedRegion, Region, RegionContext, RegionOp, RegionResource,
};
use crate::text::WidthMode;

#[derive(Clone)]
pub(crate) struct Span {
    pub start: u16,
    pub end: u16,
    key: Arc<str>,
}

/// Sorted row intervals shared by composition, projection, and output safety.
#[derive(Clone, Default)]
pub(crate) struct Coverage {
    rows: Vec<Vec<Span>>,
    spans: usize,
}

impl Coverage {
    pub fn row(&self, y: u16) -> &[Span] {
        self.rows.get(usize::from(y)).map_or(&[], Vec::as_slice)
    }

    pub fn is_empty(&self) -> bool {
        self.spans == 0
    }

    pub fn clear(&mut self) {
        for row in &mut self.rows {
            row.clear();
        }
        self.spans = 0;
    }

    fn insert(&mut self, key: Arc<str>, rect: Rect) {
        self.rows
            .resize_with(self.rows.len().max(usize::from(rect.bottom())), Vec::new);
        for y in rect.top()..rect.bottom() {
            let row = &mut self.rows[usize::from(y)];
            let at = row.partition_point(|span| span.start < rect.left());
            row.insert(
                at,
                Span {
                    start: rect.left(),
                    end: rect.right(),
                    key: Arc::clone(&key),
                },
            );
            self.spans += 1;
        }
    }

    fn remove(&mut self, key: &str, rect: Rect) {
        for y in rect.top()..rect.bottom() {
            if let Some(row) = self.rows.get_mut(usize::from(y)) {
                let len = row.len();
                row.retain(|span| span.key.as_ref() != key);
                self.spans -= len - row.len();
            }
        }
    }

    fn first_intersection(&self, rect: Rect) -> Option<&Span> {
        if self.is_empty() || rect.is_empty() {
            return None;
        }
        let bottom = usize::from(rect.bottom()).min(self.rows.len());
        for y in usize::from(rect.top())..bottom {
            let row = &self.rows[y];
            let at = row.partition_point(|span| span.end <= rect.left());
            if let Some(span) = row.get(at)
                && span.start < rect.right()
            {
                return Some(span);
            }
        }
        None
    }

    pub fn intersects(&self, rect: Rect) -> bool {
        self.first_intersection(rect).is_some()
    }

    fn cut(&mut self, rect: Rect) -> bool {
        if self.is_empty() || rect.is_empty() {
            return false;
        }
        let mut changed = false;
        for y in rect.top()..rect.bottom() {
            let Some(row) = self.rows.get_mut(usize::from(y)) else {
                break;
            };
            let mut at = row.partition_point(|span| span.end <= rect.left());
            while at < row.len() && row[at].start < rect.right() {
                changed = true;
                let span = &mut row[at];
                if span.start < rect.left() {
                    if span.end > rect.right() {
                        let right = Span {
                            start: rect.right(),
                            end: span.end,
                            key: Arc::clone(&span.key),
                        };
                        span.end = rect.left();
                        row.insert(at + 1, right);
                        self.spans += 1;
                        break;
                    }
                    span.end = rect.left();
                    at += 1;
                } else if span.end > rect.right() {
                    span.start = rect.right();
                    break;
                } else {
                    row.remove(at);
                    self.spans -= 1;
                }
            }
        }
        changed
    }

    fn covers(&self, key: &str, rect: Rect) -> bool {
        (rect.top()..rect.bottom()).all(|y| {
            self.row(y).iter().any(|span| {
                span.key.as_ref() == key && span.start <= rect.left() && span.end >= rect.right()
            })
        })
    }

    fn same_key(&self, other: &Self, key: &str, rect: Rect, old_rect: Rect) -> bool {
        if self.is_empty() && other.is_empty() {
            return true;
        }
        (0..rect.height).all(|y| {
            self.row(rect.y + y)
                .iter()
                .filter(|span| span.key.as_ref() == key)
                .map(|span| (span.start - rect.x, span.end - rect.x))
                .eq(other
                    .row(old_rect.y + y)
                    .iter()
                    .filter(|span| span.key.as_ref() == key)
                    .map(|span| (span.start - old_rect.x, span.end - old_rect.x)))
        })
    }

    #[cfg(any(test, uncurses_bench))]
    pub fn from_rects(rects: impl Iterator<Item = Rect>) -> Self {
        let mut coverage = Self::default();
        for rect in rects.filter(|r| !r.is_empty()) {
            coverage.insert(Arc::from(""), rect);
        }
        coverage
    }
}

pub(crate) struct Entry {
    pub rect: Rect,
    content: Arc<dyn Region>,
}

pub(crate) struct Displayed {
    pub rect: Rect,
    content: Arc<dyn Region>,
    context: RegionContext,
    prepared: Arc<PreparedRegion>,
}

pub(crate) struct RegionFrame {
    next: BTreeMap<Arc<str>, Arc<Displayed>>,
    retire: Vec<Arc<Displayed>>,
    draw: Vec<Arc<Displayed>>,
    coverage: Coverage,
    retained: Coverage,
    overlays: Coverage,
    resources: Vec<Arc<RegionResource>>,
    pub(super) upload: Vec<Arc<RegionResource>>,
    release: Vec<Arc<RegionResource>>,
}

impl RegionFrame {
    pub fn has_output(&self) -> bool {
        !self.retire.is_empty()
            || !self.draw.is_empty()
            || !self.upload.is_empty()
            || !self.release.is_empty()
    }
}

#[derive(Default)]
pub(crate) struct Regions {
    pub entries: BTreeMap<Arc<str>, Entry>,
    pub displayed: BTreeMap<Arc<str>, Arc<Displayed>>,
    coverage: Coverage,
    overlaid: bool,
    displayed_overlays: Coverage,
    bindings: BTreeMap<Arc<str>, Arc<RegionResource>>,
    resources: Vec<Arc<RegionResource>>,
    dirty: bool,
    anchors: BTreeMap<Arc<str>, Anchor>,
}

struct Anchor {
    source: Arc<dyn AnchoredRegion>,
    position: Option<Position>,
    error: Option<(io::ErrorKind, String)>,
}

pub(crate) fn validate_coverage(region: &dyn Region, rect: Rect) -> io::Result<()> {
    if let Some(coverage) = region.coverage() {
        let bounds = Rect::new(0, 0, rect.width, rect.height);
        for (i, part) in coverage.iter().enumerate() {
            if part.is_empty()
                || part.intersection(bounds) != *part
                || coverage[..i]
                    .iter()
                    .any(|old| !part.intersection(*old).is_empty())
            {
                return Err(invalid(
                    "region coverage must contain disjoint rectangles within its bounds",
                ));
            }
        }
    }
    Ok(())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn touch(rect: Rect, base: &mut RenderBuffer) {
    let rect = rect.intersection(base.bounds());
    for y in rect.top()..rect.bottom() {
        // A covered glyph can extend past either rectangle edge.
        base.touch_full_line(y);
    }
}

impl Regions {
    #[inline]
    pub(crate) fn has_anchors(&self) -> bool {
        !self.anchors.is_empty()
    }

    pub fn has_coverage_at(&self, y: u16) -> bool {
        !self.coverage.row(y).is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.anchors.clear();
        self.bindings.clear();
        self.coverage.clear();
        self.overlaid = false;
        self.dirty = true;
    }

    fn remove(&mut self, key: &str, base: &mut RenderBuffer) {
        if let Some(entry) = self.entries.remove(key) {
            self.coverage.remove(key, entry.rect);
            touch(entry.rect, base);
            self.dirty = true;
        }
    }

    pub fn overwrite(&mut self, rect: Rect, base: &mut RenderBuffer) {
        let rect = rect.intersection(base.bounds());
        if !rect.is_empty() && self.coverage.cut(rect) {
            touch(rect, base);
            self.overlaid = true;
            self.dirty = true;
        }
    }

    fn replace(&mut self, rect: Rect, base: &mut RenderBuffer) {
        while let Some(key) = if self.overlaid {
            self.entries
                .iter()
                .find_map(|(key, entry)| (!entry.rect.intersection(rect).is_empty()).then_some(key))
        } else {
            self.coverage.first_intersection(rect).map(|span| &span.key)
        } {
            let key = Arc::clone(key);
            self.remove(&key, base);
            self.anchors.remove(&key);
        }
    }
}

impl Renderer {
    pub(crate) fn remove_anchor_declaration(&mut self, key: &str) {
        if let Some(regions) = &mut self.regions
            && regions.anchors.remove(key).is_some()
        {
            regions.dirty = true;
        }
    }

    pub(crate) fn set_anchored_region(
        &mut self,
        key: &str,
        source: Arc<dyn AnchoredRegion>,
        base: &mut RenderBuffer,
    ) {
        let regions = self.regions.get_or_insert_with(Default::default);
        regions.remove(key, base);
        regions.anchors.insert(
            Arc::from(key),
            Anchor {
                source,
                position: None,
                error: None,
            },
        );
        regions.dirty = true;
        self.refresh_anchored_regions(base);
    }

    pub(crate) fn refresh_anchored_regions(&mut self, base: &mut RenderBuffer) {
        let Some(regions) = &mut self.regions else {
            return;
        };
        if regions.anchors.is_empty() {
            return;
        }
        let keys: Vec<_> = regions.anchors.keys().cloned().collect();
        for key in keys {
            let Some(anchor) = regions.anchors.get_mut(&key) else {
                continue;
            };
            let resolved = anchor
                .source
                .resolve(crate::screen::RegionCells(base))
                .and_then(|placement| {
                    if let Some(placement) = &placement {
                        if placement.rect.is_empty()
                            || placement.rect.intersection(base.bounds()) != placement.rect
                        {
                            return Err(invalid("anchored region must fit inside the screen"));
                        }
                        validate_coverage(placement.content.as_ref(), placement.rect)?;
                    }
                    Ok(placement)
                });
            let placement = match resolved {
                Ok(placement) => {
                    anchor.error = None;
                    placement
                }
                Err(error) => {
                    anchor.error = Some((error.kind(), error.to_string()));
                    regions.dirty = true;
                    continue;
                }
            };
            let Some(placement) = placement else {
                anchor.position = None;
                regions.remove(&key, base);
                continue;
            };
            let moved = anchor.position != Some(placement.anchor);
            anchor.position = Some(placement.anchor);
            if !moved
                && regions.entries.get(&key).is_some_and(|old| {
                    old.rect == placement.rect && old.content.same(placement.content.as_ref())
                })
            {
                continue;
            }
            let mut holes = Coverage::default();
            if !moved && let Some(old) = regions.entries.get(&key) {
                holes.insert(Arc::clone(&key), old.rect);
                for y in old.rect.top()..old.rect.bottom() {
                    for span in regions
                        .coverage
                        .row(y)
                        .iter()
                        .filter(|span| span.key == key)
                    {
                        holes.cut(Rect::new(span.start, y, span.end - span.start, 1));
                    }
                }
            }
            regions.remove(&key, base);
            regions.replace(placement.rect, base);
            touch(placement.rect, base);
            regions.dirty = true;
            Self::insert_coverage(
                regions,
                Arc::clone(&key),
                placement.rect,
                placement.content.as_ref(),
            );
            regions.entries.insert(
                Arc::clone(&key),
                Entry {
                    rect: placement.rect,
                    content: placement.content,
                },
            );
            for (y, row) in holes.rows.iter().enumerate() {
                for span in row {
                    regions.overwrite(
                        Rect::new(span.start, y as u16, span.end - span.start, 1),
                        base,
                    );
                }
            }
        }
    }

    pub(crate) fn set_region_resource(&mut self, key: &str, resource: Arc<RegionResource>) {
        let regions = self.regions.get_or_insert_with(Default::default);
        if regions
            .bindings
            .get(key)
            .is_some_and(|old| Arc::ptr_eq(old, &resource))
        {
            return;
        }
        regions.bindings.insert(Arc::from(key), resource);
        regions.dirty = true;
    }

    pub(crate) fn remove_region_resource(&mut self, key: &str) {
        if let Some(regions) = &mut self.regions
            && regions.bindings.remove(key).is_some()
        {
            regions.dirty = true;
        }
    }

    pub(crate) fn set_region(
        &mut self,
        key: &str,
        rect: Rect,
        content: impl Region + 'static,
        base: &mut RenderBuffer,
    ) {
        let regions = self.regions.get_or_insert_with(Default::default);
        if regions
            .entries
            .get(key)
            .is_some_and(|entry| entry.rect == rect && entry.content.same(&content))
            && (!regions.overlaid || regions.coverage.covers(key, rect))
        {
            return;
        }
        let cached = regions
            .entries
            .get_key_value(key)
            .map(|(key, entry)| (key, &entry.content))
            .or_else(|| {
                regions
                    .displayed
                    .get_key_value(key)
                    .map(|(key, old)| (key, &old.content))
            });
        let key: Arc<str> = cached.map_or_else(|| Arc::from(key), |(key, _)| Arc::clone(key));
        let content: Arc<dyn Region> =
            if let Some((_, old)) = cached.filter(|(_, old)| old.same(&content)) {
                Arc::clone(old)
            } else {
                Arc::new(content)
            };
        regions.remove(&key, base);
        regions.replace(rect, base);
        touch(rect, base);
        regions.dirty = true;
        Self::insert_coverage(regions, Arc::clone(&key), rect, content.as_ref());
        regions.entries.insert(key, Entry { rect, content });
    }

    pub(crate) fn remove_region(&mut self, key: &str, base: &mut RenderBuffer) {
        if let Some(regions) = &mut self.regions {
            regions.remove(key, base);
        }
    }

    pub(crate) fn prepare_regions(&self, front: &RenderBuffer) -> io::Result<Option<RegionFrame>> {
        let Some(regions) = &self.regions else {
            return Ok(None);
        };
        if let Some((kind, message)) = regions
            .anchors
            .values()
            .find_map(|anchor| anchor.error.as_ref())
        {
            return Err(io::Error::new(*kind, message.clone()));
        }
        let size = Size::new(front.width(), front.height());
        let context = RegionContext {
            screen_size: size,
            color_profile: self.color_profile(),
        };
        let changed = regions.dirty || self.force_clear;
        if !changed {
            self.validate_region_text(front, &regions.coverage, &self.region_coverage)?;
            return Ok(None);
        }
        let bounds = Rect::new(0, 0, size.width, size.height);
        let mut frame = RegionFrame {
            next: BTreeMap::new(),
            retire: Vec::new(),
            draw: Vec::new(),
            coverage: regions.coverage.clone(),
            retained: Coverage::default(),
            overlays: Coverage::default(),
            resources: Vec::new(),
            upload: Vec::new(),
            release: Vec::new(),
        };
        if regions.overlaid {
            frame.coverage.clear();
            for (key, entry) in &regions.entries {
                frame.coverage.insert(Arc::clone(key), entry.rect);
            }
            frame.overlays = frame.coverage.clone();
            for (y, row) in regions.coverage.rows.iter().enumerate() {
                for span in row {
                    frame
                        .overlays
                        .cut(Rect::new(span.start, y as u16, span.end - span.start, 1));
                }
            }
        }
        for (key, entry) in &regions.entries {
            if entry.rect.intersection(bounds) != entry.rect {
                return Err(invalid(
                    "region lies outside the resized screen; move or remove it",
                ));
            }
            let old = regions.displayed.get(key).filter(|old| {
                old.context == context
                    && old.rect.width == entry.rect.width
                    && old.rect.height == entry.rect.height
                    && entry.content.same(old.content.as_ref())
            });
            let reusable = old.filter(|old| {
                frame
                    .overlays
                    .same_key(&regions.displayed_overlays, key, entry.rect, old.rect)
            });
            let displayed = if let Some(old) = reusable.filter(|old| old.rect == entry.rect) {
                Arc::clone(old)
            } else {
                let mut visible = (entry.rect.top()..entry.rect.bottom()).flat_map(|y| {
                    regions
                        .coverage
                        .row(y)
                        .iter()
                        .filter(move |span| span.key.as_ref() == key.as_ref())
                        .map(move |span| {
                            Rect::new(
                                span.start - entry.rect.x,
                                y - entry.rect.y,
                                span.end - span.start,
                                1,
                            )
                        })
                });
                let clipped = if reusable.is_some() {
                    None
                } else {
                    entry.content.prepare_visible(
                        Size::new(entry.rect.width, entry.rect.height),
                        context,
                        &mut visible,
                    )?
                };
                let prepared = if let Some(prepared) = clipped {
                    Self::validate_region_ops(&prepared, entry.rect)?;
                    Arc::new(prepared)
                } else if let Some(old) = old {
                    Arc::clone(&old.prepared)
                } else {
                    let prepared = entry
                        .content
                        .prepare(Size::new(entry.rect.width, entry.rect.height), context)?;
                    Self::validate_region_ops(&prepared, entry.rect)?;
                    Arc::new(prepared)
                };
                Arc::new(Displayed {
                    rect: entry.rect,
                    content: Arc::clone(&entry.content),
                    context,
                    prepared,
                })
            };
            if self.force_clear || old.is_none_or(|old| !Arc::ptr_eq(old, &displayed)) {
                frame.draw.push(Arc::clone(&displayed));
            } else {
                frame.retained.insert(Arc::clone(key), displayed.rect);
            }
            frame.next.insert(Arc::clone(key), displayed);
        }
        for (key, old) in &regions.displayed {
            if self.force_clear || frame.next.get(key).is_none_or(|new| !Arc::ptr_eq(old, new)) {
                Self::validate_region_cleanup(old, size)?;
                frame.retire.push(Arc::clone(old));
            }
        }
        let mut seen = BTreeSet::new();
        for resource in regions.bindings.values().chain(
            frame
                .next
                .values()
                .flat_map(|region| region.prepared.draw.iter().chain(&region.prepared.erase))
                .filter_map(|op| match op {
                    RegionOp::Resource(resource) => Some(resource),
                    _ => None,
                }),
        ) {
            collect_resource(resource, &mut seen, &mut frame.resources);
        }
        for resource in &frame.resources {
            if resource
                .minimum_color_profile
                .is_some_and(|minimum| context.color_profile < minimum)
            {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "terminal resource requires a higher color profile",
                ));
            }
            if self.force_clear
                || !regions
                    .resources
                    .iter()
                    .any(|old| Arc::ptr_eq(old, resource))
            {
                frame.upload.push(Arc::clone(resource));
            }
        }
        for resource in regions.resources.iter().rev() {
            if !frame.resources.iter().any(|new| Arc::ptr_eq(new, resource)) {
                frame.release.push(Arc::clone(resource));
            }
        }
        self.validate_region_text(front, &regions.coverage, &frame.coverage)?;
        Ok(Some(frame))
    }

    fn validate_region_ops(prepared: &PreparedRegion, rect: Rect) -> io::Result<()> {
        let bounds = Rect::new(0, 0, rect.width, rect.height);
        for op in prepared.draw.iter().chain(&prepared.erase) {
            let valid = match op {
                RegionOp::MoveTo(pos) | RegionOp::SetCursor(pos) => bounds.contains(*pos),
                RegionOp::Erase(rect) => !rect.is_empty() && rect.intersection(bounds) == *rect,
                RegionOp::SetStyle(_)
                | RegionOp::Write(_)
                | RegionOp::Resource(_)
                | RegionOp::Command(_) => true,
            };
            if !valid {
                return Err(invalid(
                    "prepared region operation lies outside its local bounds",
                ));
            }
        }
        Ok(())
    }

    fn validate_region_cleanup(region: &Displayed, size: Size) -> io::Result<()> {
        let bounds = Rect::new(0, 0, size.width, size.height);
        if region.rect.intersection(bounds) != region.rect
            && region.prepared.erase.iter().any(|op| {
                !matches!(
                    op,
                    RegionOp::Erase(_) | RegionOp::Command(_) | RegionOp::Resource(_)
                )
            })
        {
            return Err(invalid(
                "region cleanup after resize requires clipped erase operations",
            ));
        }
        Ok(())
    }

    /// Check effective rows without modifying the replay source or desired damage.
    fn validate_region_text(
        &self,
        front: &RenderBuffer,
        coverage: &Coverage,
        barriers: &Coverage,
    ) -> io::Result<()> {
        if barriers.is_empty() || self.width_mode != WidthMode::Wc {
            return Ok(());
        }
        for y in 0..front.height() {
            let spans = coverage.row(y);
            let Some(last) = barriers.row(y).last() else {
                continue;
            };
            // Placement and overlay changes already touch their affected rows.
            if !self.force_clear && front.touched(y).is_none() {
                continue;
            }
            let line = front.line(y).expect("desired row");
            let mut covered = 0;
            for (x, cell) in line.iter().enumerate().take(usize::from(last.end)) {
                if cell.is_continuation() {
                    continue;
                }
                while spans
                    .get(covered)
                    .is_some_and(|span| usize::from(span.end) <= x)
                {
                    covered += 1;
                }
                let hidden = spans.get(covered).is_some_and(|span| {
                    usize::from(span.start) < x + usize::from(cell.width().max(1))
                });
                if !hidden && self.width_is_uncertain(cell) {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "region intersects a text tail with uncertain width; use grapheme mode or explicit sized text",
                    ));
                }
            }
            if !self.force_clear
                && self
                    .cur_buf
                    .as_ref()
                    .and_then(|buf| buf.line(y))
                    .is_some_and(|line| {
                        self.uncertain_from(line)
                            .is_some_and(|x| x < usize::from(last.end))
                    })
            {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "region intersects a displayed text tail with uncertain width",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn sync_frame(&mut self, front: &mut RenderBuffer) -> bool {
        let Some(regions) = self.regions.take() else {
            return self.sync_front(front);
        };
        let changed = self.sync_front_covered(front, &regions.coverage);
        self.regions = Some(regions);
        changed
    }

    pub(crate) fn commit_regions(&mut self, frame: Option<RegionFrame>) {
        if let Some(frame) = frame {
            let regions = self.regions.as_mut().expect("prepared region frame");
            regions.displayed = frame.next;
            regions.displayed_overlays = frame.overlays;
            regions.resources = frame.resources;
            regions.dirty = false;
            if regions.entries.is_empty()
                && regions.displayed.is_empty()
                && regions.bindings.is_empty()
                && regions.resources.is_empty()
                && regions.anchors.is_empty()
            {
                self.regions = None;
            }
        }
    }

    fn insert_coverage(regions: &mut Regions, key: Arc<str>, rect: Rect, content: &dyn Region) {
        if let Some(parts) = content.coverage() {
            for part in parts {
                regions.coverage.insert(
                    Arc::clone(&key),
                    Rect::new(rect.x + part.x, rect.y + part.y, part.width, part.height),
                );
            }
            regions.overlaid = true;
        } else {
            regions.coverage.insert(key, rect);
        }
    }

    pub(crate) fn redraw_pending(&self) -> bool {
        self.force_clear
    }

    pub(crate) fn frame_pending(&self) -> bool {
        self.force_clear || self.regions.as_ref().is_some_and(|regions| regions.dirty)
    }

    pub(crate) fn region_intersects(&self, rect: Rect) -> bool {
        self.region_barriers
            .as_ref()
            .unwrap_or(&self.region_coverage)
            .intersects(rect)
    }

    #[cfg(test)]
    pub(crate) fn set_region_rects(&mut self, rects: impl Iterator<Item = Rect>) {
        self.region_coverage = Coverage::from_rects(rects);
    }

    pub(crate) fn has_displayed_regions(&self) -> bool {
        self.regions
            .as_ref()
            .is_some_and(|regions| !regions.displayed.is_empty() || !regions.resources.is_empty())
    }

    pub(crate) fn retire_regions(
        &mut self,
        out: &mut Vec<u8>,
        frame: &mut RegionFrame,
        size: Size,
    ) {
        for old in &frame.retire {
            self.emit_region_ops(out, &old.prepared.erase, old.rect, size);
            self.region_erased(old.rect);
        }
        if !frame.retire.is_empty() {
            self.reset_pen(out).unwrap();
        }
        for resource in &frame.release {
            out.extend_from_slice(&resource.release);
        }
        std::mem::swap(&mut self.region_coverage, &mut frame.coverage);
        self.region_barriers = Some(std::mem::take(&mut frame.retained));
    }

    pub(crate) fn draw_regions(
        &mut self,
        out: &mut Vec<u8>,
        frame: Option<&RegionFrame>,
        size: Size,
    ) {
        let Some(regions) = self.regions.take() else {
            return;
        };
        let draw = frame.map_or(&[][..], |frame| frame.draw.as_slice());
        for region in draw {
            self.emit_region_ops(out, &region.prepared.draw, region.rect, size);
            self.region_drawn(region.rect);
        }
        self.region_barriers = None;
        let overlays = frame.map_or(&regions.displayed_overlays, |frame| &frame.overlays);
        self.emit_region_overlays(out, overlays, Some(draw));
        self.regions = Some(regions);
        self.reset_pen(out).unwrap();
    }

    fn emit_region_overlays(
        &mut self,
        out: &mut Vec<u8>,
        overlays: &Coverage,
        redraw: Option<&[Arc<Displayed>]>,
    ) {
        if overlays.is_empty() {
            return;
        }
        let back = std::mem::replace(&mut self.back_buf, RenderBuffer::new(0, 0));
        let mut current = self.cur_buf.take();
        for y in 0..back.height() {
            let line = back.line(y).unwrap();
            for span in overlays.row(y) {
                let old = current.as_mut().and_then(|buf| buf.line_mut(y));
                let force = redraw.is_none_or(|draw| {
                    draw.iter()
                        .any(|region| region.rect.contains(Position::new(span.start, y)))
                });
                let previous = if force { None } else { old.as_deref() };
                let range = usize::from(span.start)..usize::from(span.end).min(line.len());
                let differs = |x: &usize| previous.and_then(|old| old.get(*x)) != Some(&line[*x]);
                let Some(first) = range.clone().find(differs) else {
                    continue;
                };
                let last = range.rev().find(differs).unwrap();
                let (first, last) =
                    back.shift_span(Position::new(first as u16, y), last as u16 + 1);
                self.move_to(out, &back, y, first).unwrap();
                if force {
                    // Native multi-cell glyphs can redirect a write to their
                    // right edge. Erase the target cells before replacing them.
                    self.reset_pen(out).unwrap();
                    ansi::screen::write_ech(out, last - first + 1).unwrap();
                }
                self.put_range(out, &back, previous, line, y, first as usize, last as usize)
                    .unwrap();
                if let Some(old) = old {
                    old[first as usize..=last as usize]
                        .clone_from_slice(&line[first as usize..=last as usize]);
                }
            }
        }
        self.cur_buf = current;
        self.back_buf = back;
    }

    fn emit_region_ops(&mut self, out: &mut Vec<u8>, ops: &[RegionOp], rect: Rect, size: Size) {
        if ops.is_empty() {
            return;
        }
        self.reset_pen(out).unwrap();
        // A clipped cleanup can start outside the new viewport. Erase supplies
        // its own clipped cursor positions; other recipes fit by preflight.
        if rect.intersection(Rect::new(0, 0, size.width, size.height)) == rect {
            self.move_to_between_frames(out, size, rect.y, rect.x)
                .unwrap();
        }
        let mut anchor = None;
        for op in ops {
            if matches!(op, RegionOp::MoveTo(_) | RegionOp::Erase(_)) {
                self.restore_region_anchor(out, &mut anchor);
            }
            match op {
                RegionOp::Resource(_) => {}
                RegionOp::Command(bytes) => out.extend_from_slice(bytes),
                RegionOp::MoveTo(pos) => self
                    .move_to_between_frames(out, size, rect.y + pos.y, rect.x + pos.x)
                    .unwrap(),
                RegionOp::SetStyle(style) => self
                    .update_pen(out, Some(&Cell::BLANK.style(style.clone())))
                    .unwrap(),
                RegionOp::Write(bytes) => {
                    if !bytes.is_empty() {
                        if !self.fullscreen && anchor.is_none() {
                            ansi::cursor::write_save_cursor(out).unwrap();
                            anchor = Some(self.cur.clone());
                        }
                        out.extend_from_slice(bytes);
                        self.invalidate_cursor();
                    }
                }
                RegionOp::Erase(local) => {
                    let erase = Rect::new(
                        rect.x + local.x,
                        rect.y + local.y,
                        local.width,
                        local.height,
                    )
                    .intersection(Rect::new(0, 0, size.width, size.height));
                    self.reset_pen(out).unwrap();
                    for y in erase.top()..erase.bottom() {
                        self.move_to_between_frames(out, size, y, erase.x).unwrap();
                        ansi::screen::write_ech(out, erase.width).unwrap();
                        self.set_cursor_position(Position::new(erase.x, y));
                    }
                }
                RegionOp::SetCursor(pos) => {
                    self.set_cursor_position(Position::new(rect.x + pos.x, rect.y + pos.y));
                    anchor = None;
                }
            }
        }
        self.restore_region_anchor(out, &mut anchor);
    }

    fn restore_region_anchor(
        &mut self,
        out: &mut Vec<u8>,
        anchor: &mut Option<super::state::Cursor>,
    ) {
        let Some(saved) = anchor.take() else {
            return;
        };
        // DECRC also restores the saved pen. Preserve any intervening style
        // operation, including links that terminals may not save with DECSC.
        let changed = (self.cur.style() != saved.style()).then(|| self.cur.style().clone());
        if changed.is_some() {
            self.reset_pen(out).unwrap();
        }
        ansi::cursor::write_restore_cursor(out).unwrap();
        self.cur = saved;
        if let Some(style) = changed {
            self.reset_pen(out).unwrap();
            self.update_pen(out, Some(&Cell::BLANK.style(style)))
                .unwrap();
        }
    }

    fn region_erased(&mut self, rect: Rect) {
        if let Some(current) = &mut self.cur_buf {
            current.clear_rect(rect);
        }
        for y in rect.top()..rect.bottom() {
            self.back_buf.touch_full_line(y);
        }
    }

    pub(crate) fn region_drawn(&mut self, rect: Rect) {
        if let Some(current) = &mut self.cur_buf {
            for y in rect.top()..rect.bottom() {
                if let (Some(line), Some(projected)) = (current.line_mut(y), self.back_buf.line(y))
                {
                    let span = usize::from(rect.left())..usize::from(rect.right());
                    line[span.clone()].clone_from_slice(&projected[span]);
                }
            }
        }
    }

    pub(crate) fn validate_region_replay(&self, size: Size) -> io::Result<()> {
        let bounds = Rect::new(0, 0, size.width, size.height);
        if self.regions.as_ref().is_some_and(|regions| {
            regions.resources.iter().any(|resource| {
                resource
                    .minimum_color_profile
                    .is_some_and(|minimum| self.color_profile() < minimum)
            })
        }) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "resource replay requires a higher color profile",
            ));
        }
        if self.regions.as_ref().is_some_and(|regions| {
            regions
                .displayed
                .values()
                .any(|region| region.rect.intersection(bounds) != region.rect)
        }) {
            return Err(invalid(
                "displayed region does not fit the resized screen; render a new frame before insertion",
            ));
        }
        Ok(())
    }

    pub(crate) fn replay_regions(&mut self, out: &mut Vec<u8>, size: Size) {
        self.back_buf.resize(size.width, size.height);
        // Normalize the one glyph per row that a smaller right edge can split.
        if size.width > 0 {
            for y in 0..size.height {
                let line = self.back_buf.line(y).unwrap();
                let mut x = usize::from(size.width - 1);
                while x > 0 && line[x].is_continuation() {
                    x -= 1;
                }
                if x + usize::from(line[x].width()) > usize::from(size.width) {
                    let blank = Cell::BLANK.style(line[x].style.clone());
                    for col in x as u16..size.width {
                        self.back_buf.set_cell((col, y), &blank);
                    }
                }
            }
        }
        self.request_clear();
        let regions = self.regions.take().expect("displayed region replay");
        self.render_back_with_resources(out, &regions.resources)
            .unwrap();
        for region in regions.displayed.values() {
            self.emit_region_ops(out, &region.prepared.draw, region.rect, size);
            self.region_drawn(region.rect);
        }
        self.emit_region_overlays(out, &regions.displayed_overlays, None);
        self.regions = Some(regions);
        self.reset_pen(out).unwrap();
    }

    pub(crate) fn release_regions(&mut self, out: &mut Vec<u8>, size: Size) -> io::Result<()> {
        let Some(regions) = &self.regions else {
            return Ok(());
        };
        for region in regions.displayed.values() {
            Self::validate_region_cleanup(region, size)?;
        }
        let regions = self.regions.take().unwrap();
        for region in regions.displayed.values() {
            self.emit_region_ops(out, &region.prepared.erase, region.rect, size);
            self.region_erased(region.rect);
        }
        for resource in regions.resources.iter().rev() {
            out.extend_from_slice(&resource.release);
        }
        self.regions = Some(regions);
        self.reset_pen(out).unwrap();
        Ok(())
    }

    pub(crate) fn commit_region_release(&mut self) {
        if let Some(regions) = &mut self.regions {
            regions.displayed.clear();
            regions.displayed_overlays.clear();
            regions.resources.clear();
            regions.dirty = true;
        }

        self.region_coverage.clear();
        self.request_clear();
    }
}

fn collect_resource(
    resource: &Arc<RegionResource>,
    seen: &mut BTreeSet<*const RegionResource>,
    ordered: &mut Vec<Arc<RegionResource>>,
) {
    if seen.insert(Arc::as_ptr(resource)) {
        for dependency in &resource.dependencies {
            collect_resource(dependency, seen, ordered);
        }
        ordered.push(Arc::clone(resource));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_cuts_preserve_other_cells_and_interval_counts() {
        for x in 0..12 {
            for width in 0..12 {
                let rect = Rect::new(2, 1, 6, 2);
                let cut = Rect::new(x, 2, width, 1);
                let mut coverage = Coverage::from_rects([rect].into_iter());
                coverage.cut(cut);
                for y in 0..4 {
                    for col in 0..24 {
                        let pos = Position::new(col, y);
                        assert_eq!(
                            coverage.intersects(Rect::new(col, y, 1, 1)),
                            rect.contains(pos) && !cut.contains(pos),
                            "cut={cut:?}, pos={pos:?}",
                        );
                    }
                }
                assert_eq!(coverage.spans, coverage.rows.iter().map(Vec::len).sum());
                assert!(!coverage.cut(cut));
                coverage.remove("", rect);
                assert!(coverage.is_empty());
            }
        }
    }
}
