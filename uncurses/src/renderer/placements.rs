//! Placement damage and output state for the renderer's frame pipeline.

use std::sync::Arc;
use std::{
    collections::BTreeMap,
    io,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::layout::Rect;
use crate::screen::{
    PlacementContext, PlacementId, PlacementOptions, PlacementOutput, PlacementResource,
    ResourceKey,
};
use crate::{buffer::Surface, cell::Cell};

use super::rows::{FrameCheckpoint, invalid};
use super::{RenderBuffer, Renderer};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Record {
    pub id: PlacementId,
    pub reference: u64,
    pub area: Rect,
    pub options: Arc<PlacementOptions>,
    output_area: Rect,
    output: Arc<[Fragment]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Fragment {
    area: Rect,
    output: PlacementOutput,
}

static NEXT_REFERENCE: AtomicU64 = AtomicU64::new(1);

fn next_reference() -> io::Result<u64> {
    let mut id = NEXT_REFERENCE.load(Ordering::Relaxed);
    loop {
        let next = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("placement reference space exhausted"))?;
        match NEXT_REFERENCE.compare_exchange_weak(id, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(id),
            Err(current) => id = current,
        }
    }
}

#[derive(Default)]
pub(crate) struct State {
    desired: BTreeMap<PlacementId, Record>,
    order: Vec<PlacementId>,
    displayed: Vec<Record>,
    uncertain: Vec<Record>,
    retired: Vec<PlacementResource>,
    committed: Vec<ResourceKey>,
    epoch: u64,
    failure_epoch: Option<u64>,
    coverage: BTreeMap<u16, Vec<bool>>,
    pub(super) frame: Option<Box<Frame>>,
}

pub(super) struct Frame {
    pub active: Vec<Record>,
    pub erase: Vec<Record>,
    pub rows: BTreeMap<u16, Vec<Cell>>,
    pub coverage: BTreeMap<u16, Vec<bool>>,
    uploads: Vec<PlacementResource>,
    retired: Vec<PlacementResource>,
    checkpoint: FrameCheckpoint,
}

impl Frame {
    pub fn has_changes(&self) -> bool {
        !self.erase.is_empty() || !self.rows.is_empty()
    }
}

impl State {
    pub(super) fn validate_commit_rows(&self, front: &RenderBuffer, count: u16) -> io::Result<()> {
        for record in self.desired.values() {
            if record.area.y >= count
                || record.area.bottom() <= count
                || record.area.right() > front.width()
                || record.area.bottom() > front.height()
            {
                continue;
            }
            let mut owned = 0;
            for y in record.area.y..record.area.bottom() {
                for x in record.area.x..record.area.right() {
                    owned += usize::from(record.owns(front, x, y)?);
                }
            }
            if owned > 0
                && (!record.options.erase_on_overwrite
                    || owned == usize::from(record.area.width) * usize::from(record.area.height))
            {
                return Err(invalid("committed rows must not split an opaque placement"));
            }
        }
        Ok(())
    }

    pub(super) fn commit_rows(&mut self, front: &RenderBuffer, count: u16) {
        let committed = self.displayed.iter().filter(|record| {
            (record.area.y..record.area.bottom().min(count)).any(|y| {
                (record.area.x..record.area.right()).any(|x| {
                    record
                        .owns(front, x, y)
                        .expect("render validated placement references")
                })
            })
        });
        for resource in resources(committed) {
            let key = resource.key();
            if !self.committed.contains(&key) {
                self.committed.push(key);
            }
        }
        self.desired.retain(|_, record| {
            if record.area.bottom() <= count {
                return false;
            }
            record.area.y = record.area.y.saturating_sub(count);
            true
        });
        self.order.retain(|id| self.desired.contains_key(id));
        self.displayed = self
            .order
            .iter()
            .map(|id| self.desired[id].clone())
            .collect();
        self.coverage = std::mem::take(&mut self.coverage)
            .into_iter()
            .filter_map(|(y, row)| y.checked_sub(count).map(|y| (y, row)))
            .collect();
    }

    pub fn has_work(&self) -> bool {
        !self.desired.is_empty() || self.has_output()
    }

    pub fn has_pending_output(&self) -> bool {
        self.frame.is_some() || !self.retired.is_empty()
    }

    pub fn has_output(&self) -> bool {
        !self.displayed.is_empty() || !self.uncertain.is_empty() || !self.retired.is_empty()
    }

    fn is_empty(&self) -> bool {
        !self.has_work() && self.frame.is_none() && self.committed.is_empty()
    }

    fn prepare(
        &mut self,
        renderer: &mut Renderer,
        front: &mut RenderBuffer,
        epoch: u64,
        release: bool,
    ) -> io::Result<bool> {
        if self.failure_epoch == Some(epoch) {
            return Err(io::Error::other(
                "placement output is uncertain; restore the terminal parser and cursor, then invalidate before rendering",
            ));
        }
        let invalidated = self.epoch != epoch || renderer.force_clear;
        if !release
            && !invalidated
            && self.uncertain.is_empty()
            && self.order.len() == self.displayed.len()
            && self.order.iter().zip(&self.displayed).all(|(id, record)| {
                self.desired.get(id) == Some(record)
                    && record.area.right() <= front.width()
                    && record.area.bottom() <= front.height()
            })
            && !self.displayed.iter().any(|record| {
                (record.area.y..record.area.bottom()).any(|row| front.touched(row).is_some())
            })
        {
            return renderer.sync_front_text(front);
        }

        let width = front.width();
        let height = front.height();
        // Validate references before consuming touches or removing atomic placements.
        if !release {
            let references: BTreeMap<_, _> =
                self.desired.values().map(|r| (r.reference, r)).collect();
            for y in 0..height {
                if width > 0
                    && let Some(span) = front.touched(y)
                {
                    for x in span.first..=span.last {
                        if let Some(id) = front.cell((x, y).into()).unwrap().placement_ref()? {
                            let record = references
                                .get(&id)
                                .ok_or_else(|| invalid("unknown placement ID"))?;
                            if x < record.area.x
                                || x >= record.area.right()
                                || y < record.area.y
                                || y >= record.area.bottom()
                            {
                                return Err(invalid(
                                    "placement cell lies outside its registered rectangle",
                                ));
                            }
                        }
                    }
                }
            }
        }

        let mut active = Vec::new();
        if !release {
            for record in self
                .order
                .iter()
                .rev()
                .filter_map(|id| self.desired.get(id))
            {
                let fits = record.area.right() <= width && record.area.bottom() <= height;
                let mut count = 0usize;
                if fits {
                    for y in record.area.y..record.area.bottom() {
                        for x in record.area.x..record.area.right() {
                            if record.owns(front, x, y)? {
                                count += 1;
                            }
                        }
                    }
                }
                if !fits
                    || (record.options.erase_on_overwrite
                        && count
                            != usize::from(record.area.width) * usize::from(record.area.height))
                {
                    clear_cells(front, record.reference, record.area);
                } else if count > 0 {
                    active.push(record.clone());
                }
            }
            active.reverse();
            self.desired
                .retain(|id, _| active.iter().any(|record| record.id == *id));
            self.order.retain(|id| self.desired.contains_key(id));
        }

        let changed = renderer.sync_front(front);
        let mut owned = vec![false; height as usize];
        let mut repaint = vec![false; height as usize];
        for record in &active {
            owned[record.area.y as usize..record.area.bottom() as usize].fill(true);
        }
        let mut coverage = BTreeMap::new();
        for y in 0..height {
            if owned[y as usize] {
                let mask: Vec<_> = front
                    .line(y)
                    .unwrap()
                    .iter()
                    .map(Cell::is_placement)
                    .collect();
                if self.coverage.get(&y) != Some(&mask) || renderer.back_buf.touched(y).is_some() {
                    repaint[y as usize] = true;
                }
                coverage.insert(y, mask);
            }
        }
        let reordered = !active
            .iter()
            .map(|r| r.id)
            .eq(self.displayed.iter().map(|r| r.id));
        let previous = self.displayed.iter().chain(&self.uncertain);
        for record in previous.clone().chain(&active) {
            if invalidated
                || reordered
                || !self.uncertain.is_empty()
                || !active.contains(record)
                || !self.displayed.contains(record)
            {
                for y in record.area.y..record.area.bottom().min(height) {
                    repaint[y as usize] = true;
                }
            }
        }
        // Whole-row erasure also damages every placement crossing those rows.
        loop {
            let mut extended = false;
            for record in previous.clone().chain(&active) {
                let rows =
                    record.area.y.min(height) as usize..record.area.bottom().min(height) as usize;
                if repaint[rows.clone()].iter().any(|&r| r) {
                    for row in &mut repaint[rows] {
                        extended |= !*row;
                        *row = true;
                    }
                }
            }
            if !extended {
                break;
            }
        }

        let mut rows = BTreeMap::new();
        for (y, &repaint) in repaint.iter().enumerate() {
            if !repaint {
                continue;
            }
            let mut overlay = front.line(y as u16).unwrap().to_vec();
            let mut x = 0;
            while x < overlay.len() {
                let width = usize::from(overlay[x].width());
                if width == 0 {
                    overlay[x] = Cell::new(" ", 1, overlay[x].style());
                    x += 1;
                } else if width > overlay.len() - x {
                    let blank = Cell::new(" ", 1, overlay[x].style());
                    overlay[x..].fill(blank);
                    break;
                } else {
                    x += width;
                }
            }
            renderer.validate_row(renderer.back_buf.line(y as u16).unwrap())?;
            rows.insert(y as u16, overlay);
        }
        let damaged = |record: &Record| {
            invalidated
                || release
                || !self.uncertain.is_empty()
                || (record.area.y..record.area.bottom().min(height)).any(|y| repaint[y as usize])
                || record.area.y >= height
        };
        let erase = previous.filter(|r| damaged(r)).cloned().collect();
        for record in &mut active {
            if !rows.contains_key(&record.area.y) {
                continue;
            }
            record.output = if let Some(encoder) = &record.options.encoder {
                let regions = record.visible_regions(front)?;
                let previous = self.displayed.iter().find(|previous| {
                    previous.reference == record.reference
                        && previous.area == record.area
                        && previous.output_area == record.area
                        && previous.options == record.options
                        && previous
                            .output
                            .iter()
                            .map(|fragment| fragment.area)
                            .eq(regions.iter().copied())
                });
                if let Some(previous) = previous {
                    previous.output.clone()
                } else {
                    regions
                        .into_iter()
                        .map(|clip| {
                            Ok(Fragment {
                                area: clip,
                                output: encoder.prepare(PlacementContext {
                                    area: record.area,
                                    clip,
                                    instance: record.reference,
                                })?,
                            })
                        })
                        .collect::<io::Result<Vec<_>>>()?
                        .into()
                }
            } else {
                Arc::from([])
            };
            record.output_area = record.area;
        }
        let resources = resources(active.iter());
        let mut known = if invalidated || !self.uncertain.is_empty() {
            Vec::new()
        } else {
            resources_from_records(&self.displayed)
                .iter()
                .map(PlacementResource::key)
                .collect()
        };
        let committed = if renderer.fullscreen {
            &[][..]
        } else {
            self.committed.as_slice()
        };
        known.extend_from_slice(committed);
        let uploads = resources
            .iter()
            .filter(|r| !known.contains(&r.key()))
            .cloned()
            .collect();
        let mut retired = self.retired.clone();
        for resource in resources_from_records(&self.displayed)
            .into_iter()
            .chain(resources_from_records(&self.uncertain))
        {
            if !retired.contains(&resource) {
                retired.push(resource);
            }
        }
        retired.retain(|r| !resources.contains(r) && !committed.contains(&r.key()));
        let frame = Frame {
            active,
            erase,
            rows,
            coverage,
            uploads,
            retired,
            checkpoint: renderer.checkpoint_frame(),
        };
        renderer
            .protected_rows
            .resize(renderer.protected_rows.len().max(height as usize), false);
        for y in 0..height {
            renderer.protected_rows[y as usize] = owned[y as usize] || repaint[y as usize];
        }
        let changed = changed || frame.has_changes();
        self.frame = Some(Box::new(frame));
        Ok(changed)
    }
}

impl Record {
    fn owns(&self, front: &RenderBuffer, x: u16, y: u16) -> io::Result<bool> {
        let Some(cell) = front.cell((x, y).into()) else {
            return Ok(false);
        };
        Ok(cell.placement_ref()? == Some(self.reference))
    }

    fn visible_regions(&self, front: &RenderBuffer) -> io::Result<Vec<Rect>> {
        let mut regions: Vec<Rect> = Vec::new();
        let mut previous = BTreeMap::<(u16, u16), usize>::new();
        for y in 0..self.area.height {
            let mut current = BTreeMap::new();
            let mut x = 0;
            while x < self.area.width {
                if !self.owns(front, self.area.x + x, self.area.y + y)? {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < self.area.width && self.owns(front, self.area.x + x, self.area.y + y)? {
                    x += 1;
                }
                let key = (start, x - start);
                let index = if let Some(&index) = previous.get(&key) {
                    regions[index].height += 1;
                    index
                } else {
                    regions.push(Rect::new(start, y, x - start, 1));
                    regions.len() - 1
                };
                current.insert(key, index);
            }
            previous = current;
        }
        Ok(regions)
    }
}

fn resources<'a>(records: impl Iterator<Item = &'a Record>) -> Vec<PlacementResource> {
    let mut result = Vec::new();
    for record in records {
        let Some(resource) = &record.options.resource else {
            continue;
        };
        if !result.contains(resource) {
            result.push(resource.clone());
        }
    }
    result
}

fn resources_from_records(records: &[Record]) -> Vec<PlacementResource> {
    resources(records.iter())
}

fn clear_cells(front: &mut RenderBuffer, reference: u64, area: Rect) {
    for y in area.y..area.bottom().min(front.height()) {
        for x in area.x..area.right().min(front.width()) {
            if let Some(cell) = front.cell((x, y).into())
                && cell.placement_ref().ok().flatten() == Some(reference)
            {
                let blank = Cell::new(" ", 1, cell.style());
                front.set_cell((x, y), &blank);
            }
        }
    }
}

impl Renderer {
    #[inline]
    pub(crate) fn has_placement_work(&self) -> bool {
        self.placements
            .as_ref()
            .is_some_and(|state| state.has_work() || state.has_pending_output())
    }

    #[inline]
    pub(crate) fn has_placement_output(&self) -> bool {
        self.placements
            .as_ref()
            .is_some_and(|state| state.has_output())
    }

    #[inline]
    pub(crate) fn has_pending_placement_output(&self) -> bool {
        self.placements
            .as_ref()
            .is_some_and(|state| state.has_pending_output())
    }

    pub(super) fn discard_unused_placements(&mut self) {
        if self
            .placements
            .as_ref()
            .is_some_and(|state| state.is_empty())
        {
            self.placements = None;
            self.protected_rows.clear();
            self.invalid_rows.clear();
        }
    }

    pub(crate) fn set_placement(
        &mut self,
        front: &mut RenderBuffer,
        id: PlacementId,
        area: Rect,
        options: &PlacementOptions,
    ) -> io::Result<()> {
        if id == 0
            || area.width == 0
            || area.height == 0
            || u32::from(area.x) + u32::from(area.width) > u32::from(front.width())
            || u32::from(area.y) + u32::from(area.height) > u32::from(front.height())
        {
            return Err(invalid(
                "placement requires a nonzero ID and an in-bounds rectangle",
            ));
        }
        if options.encoder.is_some() && (!options.draw.is_empty() || !options.erase.is_empty()) {
            return Err(invalid("encoded placements require empty static payloads"));
        }
        let placements = self.placements.get_or_insert_with(Box::default);
        if let Some(old) = placements.desired.get(&id)
            && old.area != area
        {
            clear_cells(front, old.reference, old.area);
        }
        let reference = if let Some(old) = placements.desired.get_mut(&id) {
            old.area = area;
            if old.options.as_ref() != options {
                old.options = Arc::new(options.clone());
            }
            old.reference
        } else {
            let reference = next_reference()?;
            placements.desired.insert(
                id,
                Record {
                    id,
                    reference,
                    area,
                    options: Arc::new(options.clone()),
                    output_area: area,
                    output: Arc::from([]),
                },
            );
            reference
        };
        placements.order.retain(|previous| *previous != id);
        placements.order.push(id);
        let mut cell = Cell::placement(reference);
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                cell.copy_style_from(front.cell((x, y).into()).unwrap());
                front.set_cell((x, y), &cell);
            }
        }
        Ok(())
    }

    pub(crate) fn remove_placement(&mut self, front: &mut RenderBuffer, id: PlacementId) {
        let Some(placements) = self.placements.as_mut() else {
            return;
        };
        if let Some(old) = placements.desired.remove(&id) {
            placements.order.retain(|previous| *previous != id);
            clear_cells(front, old.reference, old.area);
        }
        self.discard_unused_placements();
    }

    pub(crate) fn sync_frame(
        &mut self,
        front: &mut RenderBuffer,
        epoch: u64,
        release: bool,
    ) -> io::Result<bool> {
        let Some(mut placements) = self.placements.take() else {
            return self.sync_front_text(front);
        };
        let result = if placements.has_work() {
            placements.prepare(self, front, epoch, release)
        } else {
            self.sync_front_text(front)
        };
        self.placements = Some(placements);
        if result.is_err() {
            front.touch_all();
        }
        result
    }

    pub(crate) fn complete_frame(&mut self, epoch: u64) {
        let Some(placements) = self.placements.as_mut() else {
            return;
        };
        if let Some(frame) = placements.frame.take() {
            if frame.coverage.is_empty() {
                self.protected_rows.clear();
                self.invalid_rows.clear();
            } else {
                self.protected_rows.resize(self.last_height as usize, false);
                for y in 0..self.last_height {
                    let owned = frame.coverage.contains_key(&y);
                    self.protected_rows[y as usize] = owned;
                    if owned {
                        self.invalid_rows[y as usize] = true;
                    }
                }
            }
            for record in &frame.active {
                if let Some(desired) = placements.desired.get_mut(&record.id) {
                    desired.output_area = record.output_area;
                    desired.output = record.output.clone();
                }
            }
            placements.displayed = frame.active;
            placements.retired = frame.retired;
            placements.coverage = frame.coverage;
            placements.uncertain.clear();
            placements.epoch = epoch;
            placements.failure_epoch = None;
        }
    }

    pub(crate) fn upload_resources(&self, out: &mut Vec<u8>) {
        if let Some(frame) = self
            .placements
            .as_ref()
            .and_then(|state| state.frame.as_ref())
        {
            for resource in &frame.uploads {
                out.extend_from_slice(resource.upload());
            }
        }
    }

    pub(crate) fn delete_resources(&self, out: &mut Vec<u8>) -> bool {
        let start = out.len();
        if let Some(placements) = &self.placements {
            for resource in &placements.retired {
                out.extend_from_slice(resource.delete());
            }
        }
        out.len() != start
    }

    pub(crate) fn complete_resource_cleanup(&mut self) {
        if let Some(placements) = self.placements.as_mut() {
            placements.retired.clear();
        }
        self.discard_unused_placements();
    }

    pub(crate) fn discard_frame(&mut self) {
        if let Some(frame) = self
            .placements
            .as_mut()
            .and_then(|state| state.frame.take())
        {
            self.restore_staged_frame(frame.checkpoint);
        }
    }

    pub(crate) fn output_failed(&mut self, epoch: u64) {
        let mut placements = self.placements.take();
        if let Some(state) = placements.as_mut()
            && let Some(frame) = state.frame.take()
        {
            self.protect_failed_frame(frame.checkpoint);
            for record in frame.active {
                if !state.uncertain.contains(&record) {
                    state.uncertain.push(record);
                }
            }
        }
        self.invalidate_cursor();
        self.pen_unknown = true;
        if let Some(state) = placements.as_mut()
            && state.has_work()
        {
            state.failure_epoch = Some(epoch);
        }
        self.placements = placements;
    }

    pub(super) fn erase_placements(
        &mut self,
        out: &mut Vec<u8>,
        size: crate::layout::Size,
    ) -> io::Result<()> {
        let frame = self
            .placements
            .as_mut()
            .and_then(|state| state.frame.take());
        let result = (|| {
            if let Some(frame) = &frame {
                for record in &frame.erase {
                    self.write_placement_record(out, size, record, true)?;
                }
            }
            Ok(())
        })();
        if let Some(placements) = self.placements.as_mut() {
            placements.frame = frame;
        }
        result
    }

    pub(super) fn write_placement_record(
        &mut self,
        out: &mut Vec<u8>,
        size: crate::layout::Size,
        record: &Record,
        erase: bool,
    ) -> io::Result<()> {
        if record.options.encoder.is_some() {
            for fragment in record.output.iter() {
                self.write_placement(
                    out,
                    size,
                    (
                        record.area.x + fragment.area.x,
                        record.area.y + fragment.area.y,
                    )
                        .into(),
                    if erase {
                        &fragment.output.erase
                    } else {
                        &fragment.output.draw
                    },
                )?;
            }
            Ok(())
        } else {
            self.write_placement(
                out,
                size,
                (record.area.x, record.area.y).into(),
                if erase {
                    &record.options.erase
                } else {
                    &record.options.draw
                },
            )
        }
    }
}
