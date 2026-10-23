//! Placement damage and output state for the renderer's frame pipeline.

use std::sync::Arc;
use std::{
    collections::BTreeMap,
    io,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::layout::Rect;
use crate::screen::{PlacementId, PlacementOptions};
use crate::{buffer::Surface, cell::Cell};

use super::rows::{FrameCheckpoint, invalid};
use super::{RenderBuffer, Renderer};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Record {
    pub id: PlacementId,
    pub reference: u64,
    pub area: Rect,
    pub options: Arc<PlacementOptions>,
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
    retired: Vec<Resource>,
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
    uploads: Vec<Resource>,
    retired: Vec<Resource>,
    checkpoint: FrameCheckpoint,
}

impl Frame {
    pub fn has_changes(&self) -> bool {
        !self.erase.is_empty() || !self.rows.is_empty()
    }
}

impl State {
    pub fn has_work(&self) -> bool {
        !self.desired.is_empty() || self.has_output()
    }

    pub fn has_output(&self) -> bool {
        !self.displayed.is_empty() || !self.uncertain.is_empty() || !self.retired.is_empty()
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
            && self
                .order
                .iter()
                .zip(&self.displayed)
                .all(|(id, record)| self.desired.get(id) == Some(record))
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
            for record in self.order.iter().filter_map(|id| self.desired.get(id)) {
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
                    clear_record(front, record);
                } else if count > 0 {
                    active.push(record.clone());
                }
            }
            self.desired
                .retain(|id, _| active.iter().any(|record| record.id == *id));
            self.order.retain(|id| self.desired.contains_key(id));
        }

        let changed = renderer.sync_front(front);
        let mut owned = vec![false; height as usize];
        let mut repaint = vec![false; height as usize];
        for record in active.iter().filter(|r| r.options.cells.is_none()) {
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
            .filter(|r| r.options.cells.is_none())
            .map(|r| r.id)
            .eq(self
                .displayed
                .iter()
                .filter(|r| r.options.cells.is_none())
                .map(|r| r.id));
        let previous = self
            .displayed
            .iter()
            .chain(&self.uncertain)
            .filter(|r| r.options.cells.is_none());
        for record in previous
            .clone()
            .chain(active.iter().filter(|r| r.options.cells.is_none()))
        {
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
            for record in previous
                .clone()
                .chain(active.iter().filter(|r| r.options.cells.is_none()))
            {
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
            for x in (0..overlay.len()).rev() {
                if let Some(cell) = super::placeholders::compressed(&overlay, x) {
                    overlay[x] = cell;
                }
            }
            let mut x = 0;
            while x < overlay.len() {
                let width = usize::from(overlay[x].width());
                if width == 0 {
                    overlay[x] = Cell::BLANK.style(overlay[x].style.clone());
                    x += 1;
                } else if width > overlay.len() - x {
                    let blank = Cell::BLANK.style(overlay[x].style.clone());
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
        let resources = resources(active.iter());
        let known = if invalidated || !self.uncertain.is_empty() {
            Vec::new()
        } else {
            resources_from_records(&self.displayed)
        };
        let uploads = resources
            .iter()
            .filter(|r| !known.contains(r))
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
        retired.retain(|r| !resources.contains(r));
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
        if let Some(cells) = &self.options.cells {
            Ok(cells.cell((x - self.area.x, y - self.area.y).into()) == Some(cell))
        } else {
            Ok(cell.placement_ref()? == Some(self.reference))
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Resource {
    upload: Arc<[u8]>,
    delete: Arc<[u8]>,
}

fn resources<'a>(records: impl Iterator<Item = &'a Record>) -> Vec<Resource> {
    let mut result = Vec::new();
    for record in records {
        if record.options.upload.is_empty() && record.options.delete.is_empty() {
            continue;
        }
        let resource = Resource {
            upload: record.options.upload.clone(),
            delete: record.options.delete.clone(),
        };
        if !result.contains(&resource) {
            result.push(resource);
        }
    }
    result
}

fn resources_from_records(records: &[Record]) -> Vec<Resource> {
    resources(records.iter())
}

fn clear_record(front: &mut RenderBuffer, record: &Record) {
    if let Some(cells) = &record.options.cells {
        for y in record.area.y..record.area.bottom().min(front.height()) {
            let mut x = record.area.x;
            while x < record.area.right().min(front.width()) {
                let source = cells
                    .cell((x - record.area.x, y - record.area.y).into())
                    .unwrap();
                let width = u16::from(source.width()).max(1).min(front.width() - x);
                if !source.is_continuation()
                    && (0..width).all(|offset| {
                        front.cell((x + offset, y).into())
                            == cells.cell((x + offset - record.area.x, y - record.area.y).into())
                    })
                {
                    for offset in 0..width {
                        front.set_cell((x + offset, y), &Cell::BLANK);
                    }
                }
                x += width;
            }
        }
    } else {
        clear_cells(front, record.reference, record.area);
    }
}

fn clear_cells(front: &mut RenderBuffer, reference: u64, area: Rect) {
    for y in area.y..area.bottom().min(front.height()) {
        for x in area.x..area.right().min(front.width()) {
            if let Some(cell) = front.cell((x, y).into())
                && cell.placement_ref().ok().flatten() == Some(reference)
            {
                let blank = Cell::BLANK.style(&cell.style);
                front.set_cell((x, y), &blank);
            }
        }
    }
}

impl Renderer {
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
        if let Some(cells) = &options.cells
            && (cells.width() != area.width
                || cells.height() != area.height
                || !options.draw.is_empty()
                || !options.erase.is_empty()
                || options.erase_on_overwrite)
        {
            return Err(invalid(
                "cell placements require matching dimensions and cell overwrite behavior",
            ));
        }
        if let Some(cells) = &options.cells {
            for y in 0..cells.height() {
                if cells.line(y).unwrap().iter().any(Cell::is_placement) {
                    return Err(invalid("cell placements require ordinary cells"));
                }
            }
        }
        let reference = if let Some(old) = self.placements.desired.get_mut(&id) {
            if old.area != area || old.options.cells.is_some() != options.cells.is_some() {
                clear_record(front, old);
                old.area = area;
            }
            if old.options.as_ref() != options {
                old.options = Arc::new(options.clone());
            }
            old.reference
        } else {
            let reference = next_reference()?;
            self.placements.desired.insert(
                id,
                Record {
                    id,
                    reference,
                    area,
                    options: Arc::new(options.clone()),
                },
            );
            reference
        };
        self.placements.order.retain(|previous| *previous != id);
        self.placements.order.push(id);
        if let Some(cells) = &options.cells {
            cells.draw(front, (area.x, area.y).into());
            return Ok(());
        }
        let mut cell = Cell::placement(reference);
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                cell.style = front.cell((x, y).into()).unwrap().style.clone();
                front.set_cell((x, y), &cell);
            }
        }
        Ok(())
    }

    pub(crate) fn remove_placement(&mut self, front: &mut RenderBuffer, id: PlacementId) {
        if let Some(old) = self.placements.desired.remove(&id) {
            self.placements.order.retain(|previous| *previous != id);
            clear_record(front, &old);
        }
    }

    pub(crate) fn sync_frame(
        &mut self,
        front: &mut RenderBuffer,
        epoch: u64,
        release: bool,
    ) -> io::Result<bool> {
        if !self.placements.has_work() {
            return self.sync_front_text(front);
        }
        let mut placements = std::mem::take(&mut self.placements);
        let result = placements.prepare(self, front, epoch, release);
        self.placements = placements;
        if result.is_err() {
            front.touch_all();
        }
        result
    }

    pub(crate) fn complete_frame(&mut self, epoch: u64) {
        if let Some(frame) = self.placements.frame.take() {
            self.protected_rows.resize(self.last_height as usize, false);
            for y in 0..self.last_height {
                let owned = frame.coverage.contains_key(&y);
                self.protected_rows[y as usize] = owned;
                if owned {
                    self.invalid_rows[y as usize] = true;
                }
            }
            self.placements.displayed = frame.active;
            self.placements.retired = frame.retired;
            self.placements.coverage = frame.coverage;
            self.placements.uncertain.clear();
            self.placements.epoch = epoch;
            self.placements.failure_epoch = None;
        }
    }

    pub(crate) fn upload_resources(&self, out: &mut Vec<u8>) {
        if let Some(frame) = &self.placements.frame {
            for resource in &frame.uploads {
                out.extend_from_slice(&resource.upload);
            }
        }
    }

    pub(crate) fn delete_resources(&self, out: &mut Vec<u8>) -> bool {
        let start = out.len();
        for resource in &self.placements.retired {
            out.extend_from_slice(&resource.delete);
        }
        out.len() != start
    }

    pub(crate) fn complete_resource_cleanup(&mut self) {
        self.placements.retired.clear();
    }

    pub(crate) fn discard_frame(&mut self) {
        if let Some(frame) = self.placements.frame.take() {
            self.restore_staged_frame(frame.checkpoint);
        }
    }

    pub(crate) fn output_failed(&mut self, epoch: u64) {
        if let Some(frame) = self.placements.frame.take() {
            self.protect_failed_frame(frame.checkpoint);
            for record in frame.active {
                if !self.placements.uncertain.contains(&record) {
                    self.placements.uncertain.push(record);
                }
            }
        }
        self.invalidate_cursor();
        self.pen_unknown = true;
        if self.placements.has_work() {
            self.placements.failure_epoch = Some(epoch);
        }
    }

    pub(super) fn erase_placements(
        &mut self,
        out: &mut Vec<u8>,
        size: crate::layout::Size,
    ) -> io::Result<()> {
        let frame = self.placements.frame.take();
        let result = (|| {
            if let Some(frame) = &frame {
                for record in &frame.erase {
                    self.write_placement(
                        out,
                        size,
                        (record.area.x, record.area.y).into(),
                        &record.options.erase,
                    )?;
                }
            }
            Ok(())
        })();
        self.placements.frame = frame;
        result
    }
}
