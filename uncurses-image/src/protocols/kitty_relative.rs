//! Relative Kitty image groups and ordinary-cell anchors.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use uncurses::buffer::{Bounded, Surface};
use uncurses::layout::{Position, Rect, Size};
use uncurses::screen::{
    AnchoredRegion, PreparedRegion, Region, RegionCells, RegionContext, RegionOp, RegionPlacement,
    RegionResource, Screen,
};

use super::kitty;
use crate::geometry::coalesce;
use crate::{Image, ImageOptions, Placeholders, invalid};

/// A group of Kitty images with relative cell positions.
///
/// Layers paint in declaration order. Each layer can refer to an earlier or
/// later layer by key. Missing parents and cycles return errors before output.
/// Positions are resolved as a group, so nested layouts do not depend on the
/// terminal's maximum native reference depth.
///
/// Use [`draw`](Self::draw) with a regular native anchor, or
/// [`attach`](Self::attach) to follow ordinary Unicode placeholder cells.
/// The application must confirm Kitty relative-placement support.
#[derive(Clone)]
pub struct ImageGroup {
    layers: Arc<Vec<Layer>>,
    anchor_image: u32,
    anchor_resource: Arc<RegionResource>,
}

#[derive(Clone)]
struct Layer {
    key: String,
    parent: Option<String>,
    offset: (i32, i32),
    size: Size,
    image: Image,
    options: ImageOptions,
}

impl ImageGroup {
    /// Create an empty group and an invisible regular anchor resource.
    pub fn new() -> io::Result<Self> {
        let id = kitty::next_id()?;
        Ok(Self {
            layers: Arc::default(),
            anchor_image: id,
            anchor_resource: Arc::new(RegionResource {
                create: kitty::command(
                    &format!("a=T,f=32,s=1,v=1,i={id},U=1,p=1,c=1,r=1,q=2"),
                    b"AAAAAA==",
                )?,
                release: kitty::command(&format!("a=d,d=I,i={id},q=2"), &[])?,
                ..Default::default()
            }),
        })
    }

    /// Add or replace a layer. Replacement moves it to the top.
    ///
    /// `parent: None` measures the offset from the group anchor. A parent key
    /// measures it from that layer's origin. Offsets can be negative.
    /// Geometry and pixels use the same options as ordinary image placements.
    #[allow(clippy::too_many_arguments)]
    pub fn set(
        &mut self,
        key: &str,
        parent: Option<&str>,
        offset: (i32, i32),
        size: Size,
        image: &Image,
        options: &ImageOptions,
    ) -> io::Result<()> {
        crate::geometry::resolve(image.dimensions(), size, (1, 1), options)?;
        let layers = Arc::make_mut(&mut self.layers);
        layers.retain(|layer| layer.key != key);
        layers.push(Layer {
            key: key.into(),
            parent: parent.map(String::from),
            offset,
            size,
            image: image.clone(),
            options: options.clone(),
        });
        Ok(())
    }

    /// Remove a layer and its descendants.
    pub fn remove(&mut self, key: &str) {
        let layers = Arc::make_mut(&mut self.layers);
        let mut removed = vec![key.to_owned()];
        let mut i = 0;
        while i < removed.len() {
            for layer in layers.iter() {
                if layer.parent.as_ref() == Some(&removed[i]) && !removed.contains(&layer.key) {
                    removed.push(layer.key.clone());
                }
            }
            i += 1;
        }
        layers.retain(|layer| !removed.contains(&layer.key));
    }

    /// Stage the group at a regular native anchor.
    ///
    /// Moving the group restores the cells beneath its old footprint.
    /// Images clip to the screen and later text clips individual image cells.
    pub fn draw<W: Write>(
        &self,
        screen: &mut Screen<W>,
        key: &str,
        anchor: Position,
    ) -> io::Result<()> {
        let offsets = self.offsets()?;
        if !screen.bounds().contains(anchor) {
            return Err(invalid("image group anchor must fit inside the screen"));
        }
        let resolved = self.layout(screen.bounds(), anchor, &[], None, &offsets);
        if let Some(resolved) = resolved {
            let rect = resolved.rect;
            screen.set_region(key, rect, resolved)?;
        } else {
            screen.remove_region(key);
        }
        Ok(())
    }

    /// Follow all ordinary cells that refer to a virtual placement.
    ///
    /// The anchor uses the minimum column and minimum row among those cells.
    /// Text edits, row operations, and clipped copies move it automatically.
    /// When all anchor cells disappear, the screen removes the native group.
    /// The declaration remains available if those cells return.
    ///
    /// This method snapshots the group. Attach it again after changing layers.
    /// A full screen clear/fill removes the declaration. Reattach each frame
    /// when the application rebuilds complete frames.
    pub fn attach<W: Write>(
        &self,
        screen: &mut Screen<W>,
        key: &str,
        parent: &Placeholders,
    ) -> io::Result<()> {
        let offsets = self.offsets()?;
        screen.set_anchored_region(
            key,
            Attachment {
                group: self.clone(),
                parent: parent.clone(),
                offsets,
                cached: Mutex::new(None),
            },
        );
        Ok(())
    }

    fn offsets(&self) -> io::Result<Vec<(i64, i64)>> {
        let mut resolved = vec![None; self.layers.len()];
        for start in 0..self.layers.len() {
            let mut chain = Vec::new();
            let mut at = Some(start);
            while let Some(index) = at {
                if resolved[index].is_some() {
                    break;
                }
                if chain.contains(&index) {
                    return Err(invalid("image group contains a parent cycle"));
                }
                chain.push(index);
                at = self.layers[index]
                    .parent
                    .as_ref()
                    .map(|key| {
                        self.layers
                            .iter()
                            .position(|layer| &layer.key == key)
                            .ok_or_else(|| invalid("image group parent does not exist"))
                    })
                    .transpose()?;
            }
            let mut offset: (i64, i64) = at.and_then(|index| resolved[index]).unwrap_or((0, 0));
            for index in chain.into_iter().rev() {
                offset = (
                    offset
                        .0
                        .checked_add(i64::from(self.layers[index].offset.0))
                        .ok_or_else(|| invalid("relative horizontal offset overflow"))?,
                    offset
                        .1
                        .checked_add(i64::from(self.layers[index].offset.1))
                        .ok_or_else(|| invalid("relative vertical offset overflow"))?,
                );
                resolved[index] = Some(offset);
            }
        }
        Ok(resolved.into_iter().map(Option::unwrap).collect())
    }

    fn layout(
        &self,
        bounds: Rect,
        anchor: Position,
        preserved: &[Position],
        parent: Option<Placeholders>,
        offsets: &[(i64, i64)],
    ) -> Option<Resolved> {
        let positions: Vec<_> = offsets
            .iter()
            .map(|&(x, y)| (x + i64::from(anchor.x), y + i64::from(anchor.y)))
            .collect();
        let mut rects = Vec::new();
        for (layer, &(x, y)) in self.layers.iter().zip(&positions) {
            let left = x.max(i64::from(bounds.x));
            let top = y.max(i64::from(bounds.y));
            let right = (x + i64::from(layer.size.width)).min(i64::from(bounds.right()));
            let bottom = (y + i64::from(layer.size.height)).min(i64::from(bounds.bottom()));
            rects.push((left < right && top < bottom).then(|| {
                Rect::new(
                    left as u16,
                    top as u16,
                    (right - left) as u16,
                    (bottom - top) as u16,
                )
            }));
        }
        let mut rect = Rect::new(anchor.x, anchor.y, 1, 1);
        if rects.iter().all(Option::is_none) {
            return None;
        }
        for part in rects.iter().flatten() {
            let right = rect.right().max(part.right());
            let bottom = rect.bottom().max(part.bottom());
            rect.x = rect.x.min(part.x);
            rect.y = rect.y.min(part.y);
            rect.width = right - rect.x;
            rect.height = bottom - rect.y;
        }
        let mut owners = vec![None; usize::from(rect.width) * usize::from(rect.height)];
        for (index, part) in rects.iter().enumerate() {
            if let Some(part) = part {
                for y in part.top()..part.bottom() {
                    for x in part.left()..part.right() {
                        owners[usize::from(y - rect.y) * usize::from(rect.width)
                            + usize::from(x - rect.x)] = Some(index);
                    }
                }
            }
        }
        let preserved: Vec<_> = preserved
            .iter()
            .copied()
            .filter(|p| rect.contains(*p))
            .collect();
        let coverage = spans(rect, |x, y| {
            owners[usize::from(y) * usize::from(rect.width) + usize::from(x)].is_some()
                && !preserved.contains(&Position::new(rect.x + x, rect.y + y))
        });
        Some(Resolved {
            group: self.clone(),
            rect,
            anchor,
            parent,
            positions,
            preserved,
            owners,
            coverage,
        })
    }
}

struct Attachment {
    group: ImageGroup,
    parent: Placeholders,
    offsets: Vec<(i64, i64)>,
    cached: Mutex<Option<CachedLayout>>,
}

struct CachedLayout {
    bounds: Rect,
    anchors: Vec<Position>,
    content: Option<Arc<Resolved>>,
}

impl AnchoredRegion for Attachment {
    fn resolve(&self, cells: RegionCells<'_>) -> io::Result<Option<RegionPlacement>> {
        let mut cached = self
            .cached
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        let reusable = cached.as_ref().filter(|old| old.bounds == cells.bounds());
        let rows: Vec<_> = if reusable.is_some() {
            cells.changed_rows().collect()
        } else {
            (cells.bounds().top()..cells.bounds().bottom()).collect()
        };
        let mut positions = reusable.map_or_else(Vec::new, |old| {
            old.anchors
                .iter()
                .copied()
                .filter(|pos| rows.binary_search(&pos.y).is_err())
                .collect()
        });
        for y in rows {
            for x in cells.bounds().left()..cells.bounds().right() {
                let pos = Position::new(x, y);
                if cells
                    .cell(pos)
                    .is_some_and(|cell| self.parent.matches(cell))
                {
                    positions.push(pos);
                }
            }
        }
        positions.sort_by_key(|pos| (pos.y, pos.x));
        if positions.is_empty() {
            *cached = Some(CachedLayout {
                bounds: cells.bounds(),
                anchors: positions,
                content: None,
            });
            return Ok(None);
        }
        let anchor = Position::new(
            positions.iter().map(|p| p.x).min().unwrap(),
            positions.iter().map(|p| p.y).min().unwrap(),
        );
        if let Some(old) = &*cached
            && old.bounds == cells.bounds()
            && old.anchors == positions
        {
            return Ok(old.content.as_ref().map(|content| RegionPlacement {
                anchor,
                rect: content.rect,
                content: content.clone(),
            }));
        }
        let Some(content) = self.group.layout(
            cells.bounds(),
            anchor,
            &positions,
            Some(self.parent.clone()),
            &self.offsets,
        ) else {
            *cached = Some(CachedLayout {
                bounds: cells.bounds(),
                anchors: positions,
                content: None,
            });
            return Ok(None);
        };
        let content = Arc::new(content);
        *cached = Some(CachedLayout {
            bounds: cells.bounds(),
            anchors: positions,
            content: Some(content.clone()),
        });
        Ok(Some(RegionPlacement {
            anchor,
            rect: content.rect,
            content,
        }))
    }
}

struct Resolved {
    group: ImageGroup,
    rect: Rect,
    anchor: Position,
    parent: Option<Placeholders>,
    positions: Vec<(i64, i64)>,
    preserved: Vec<Position>,
    owners: Vec<Option<usize>>,
    coverage: Vec<Rect>,
}

impl Region for Resolved {
    fn same(&self, other: &dyn Region) -> bool {
        other.downcast_ref::<Self>().is_some_and(|other| {
            Arc::ptr_eq(&self.group.layers, &other.group.layers)
                && self.anchor == other.anchor
                && self.rect == other.rect
                && self.preserved == other.preserved
                && self.parent.as_ref().map(Placeholders::image_id)
                    == other.parent.as_ref().map(Placeholders::image_id)
        })
    }

    fn coverage(&self) -> Option<&[Rect]> {
        Some(&self.coverage)
    }

    fn prepare(&self, size: Size, context: RegionContext) -> io::Result<PreparedRegion> {
        self.prepare_visible(size, context, &mut self.coverage.clone().into_iter())
            .map(Option::unwrap)
    }

    fn prepare_visible(
        &self,
        _: Size,
        _: RegionContext,
        visible: &mut dyn Iterator<Item = Rect>,
    ) -> io::Result<Option<PreparedRegion>> {
        let mut mask = vec![false; self.owners.len()];
        for part in visible {
            for y in part.top()..part.bottom() {
                for x in part.left()..part.right() {
                    mask[usize::from(y) * usize::from(self.rect.width) + usize::from(x)] = true;
                }
            }
        }
        for pos in &self.preserved {
            mask[usize::from(pos.y - self.rect.y) * usize::from(self.rect.width)
                + usize::from(pos.x - self.rect.x)] = true;
        }
        let mut prepared = PreparedRegion::default();
        let (parent_id, parent_placement) = if let Some(parent) = &self.parent {
            prepared
                .draw
                .push(RegionOp::Resource(parent.resource().clone()));
            (parent.image_id(), parent.placement_id())
        } else {
            let id = self.group.anchor_image;
            let placement = kitty::next_id()?;
            prepared.draw.extend([
                RegionOp::Resource(self.group.anchor_resource.clone()),
                RegionOp::MoveTo(Position::new(
                    self.anchor.x - self.rect.x,
                    self.anchor.y - self.rect.y,
                )),
                RegionOp::Command(kitty::command(
                    &format!("a=p,i={id},p={placement},c=1,r=1,C=1,q=2"),
                    &[],
                )?),
            ]);
            prepared.erase.push(RegionOp::Command(kitty::command(
                &format!("a=d,d=i,i={id},p={placement},q=2"),
                &[],
            )?));
            (id, placement)
        };
        for (index, layer) in self.group.layers.iter().enumerate() {
            let parts = spans(self.rect, |x, y| {
                let at = usize::from(y) * usize::from(self.rect.width) + usize::from(x);
                mask[at] && self.owners[at] == Some(index)
            });
            if parts.is_empty() {
                continue;
            }
            let (id, resource) = kitty::pinned_resource(&layer.image, layer.size, &layer.options)?;
            prepared.draw.push(RegionOp::Resource(resource));
            let (cw, ch) = layer.options.cell_pixels;
            for part in parts {
                let placement = kitty::next_id()?;
                let x = i64::from(self.rect.x) + i64::from(part.x);
                let y = i64::from(self.rect.y) + i64::from(part.y);
                let (source_x, source_y) = self.positions[index];
                let command = format!(
                    "a=p,i={id},p={placement},P={parent_id},Q={parent_placement},H={},V={},x={},y={},w={},h={},c={},r={},q=2",
                    x - i64::from(self.anchor.x),
                    y - i64::from(self.anchor.y),
                    (x - source_x) * i64::from(cw),
                    (y - source_y) * i64::from(ch),
                    u32::from(part.width) * u32::from(cw),
                    u32::from(part.height) * u32::from(ch),
                    part.width,
                    part.height,
                );
                prepared
                    .draw
                    .push(RegionOp::Command(kitty::command(&command, &[])?));
                prepared.erase.push(RegionOp::Command(kitty::command(
                    &format!("a=d,d=i,i={id},p={placement},q=2"),
                    &[],
                )?));
            }
        }
        prepared.erase.reverse();
        prepared.erase.push(RegionOp::Erase(Rect::new(
            0,
            0,
            self.rect.width,
            self.rect.height,
        )));
        Ok(Some(prepared))
    }
}

fn spans(rect: Rect, mut visible: impl FnMut(u16, u16) -> bool) -> Vec<Rect> {
    let mut rows = Vec::new();
    for y in 0..rect.height {
        let mut x = 0;
        while x < rect.width {
            if !visible(x, y) {
                x += 1;
                continue;
            }
            let first = x;
            x += 1;
            while x < rect.width && visible(x, y) {
                x += 1;
            }
            rows.push(Rect::new(first, y, x - first, 1));
        }
    }
    coalesce(rows.into_iter())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uncurses::buffer::SurfaceMut;
    use uncurses::cell::Cell;
    use uncurses::style::Style;
    use uncurses::text::TextSurface;

    fn image() -> Image {
        Image::from_rgba(1, 1, vec![255, 0, 0, 255]).unwrap()
    }

    #[test]
    fn parent_chains_cycles_missing_parents_and_removal() {
        let image = image();
        let options = ImageOptions::default();
        let mut group = ImageGroup::new().unwrap();
        for i in 0..9 {
            group
                .set(
                    &i.to_string(),
                    (i > 0).then(|| (i - 1).to_string()).as_deref(),
                    (1, -1),
                    Size::new(2, 2),
                    &image,
                    &options,
                )
                .unwrap();
        }
        assert_eq!(group.offsets().unwrap()[8], (9, -9));
        group
            .set("0", Some("8"), (0, 0), Size::new(2, 2), &image, &options)
            .unwrap();
        let mut screen = Screen::new(Vec::new(), (20, 20));
        assert!(group.draw(&mut screen, "group", Position::ORIGIN).is_err());
        assert!(screen.writer().is_empty());
        group
            .set(
                "0",
                Some("missing"),
                (0, 0),
                Size::new(2, 2),
                &image,
                &options,
            )
            .unwrap();
        assert!(group.draw(&mut screen, "group", Position::ORIGIN).is_err());
        group.remove("0");
        assert!(group.layers.is_empty());
    }

    #[test]
    fn anchors_follow_cells_restore_text_and_return_after_erasure() {
        let image = image();
        let options = ImageOptions::default();
        let parent = image.kitty_placeholders(Size::new(1, 1), &options).unwrap();
        let mut group = ImageGroup::new().unwrap();
        group
            .set("image", None, (0, 0), Size::new(4, 2), &image, &options)
            .unwrap();
        let mut screen = Screen::new(Vec::new(), (20, 8));
        screen.set_fullscreen(true);
        parent.draw(&mut screen, "anchor", (2, 2).into()).unwrap();
        group.attach(&mut screen, "child", &parent).unwrap();
        screen.render().unwrap();
        screen.writer_mut().clear();
        screen.set_str((3, 3), "X", Style::default());
        screen.render().unwrap();
        assert!(screen.writer().contains(&b'X'));
        screen.writer_mut().clear();
        let style = screen.cell((2, 2).into()).unwrap().style.clone().bold();
        screen.cell_mut((2, 2)).unwrap().style = style;
        screen.render().unwrap();
        assert!(screen.writer().windows(3).all(|bytes| bytes != b"a=t"));
        screen.writer_mut().clear();
        screen.delete_lines(0, 1, 8, &Cell::BLANK);
        screen.render().unwrap();
        assert!(screen.writer().windows(3).any(|bytes| bytes == b",P="));
        screen.writer_mut().clear();
        screen.clear_rect(Rect::new(2, 1, 1, 1));
        screen.render().unwrap();
        assert!(screen.writer().windows(7).any(|bytes| bytes == b"a=d,d=I"));
        screen.writer_mut().clear();
        screen.set_str((0, 7), "unrelated", Style::default());
        screen.render().unwrap();
        assert!(!screen.writer().windows(3).any(|bytes| bytes == b"a=p"));
        screen.writer_mut().clear();
        parent.draw(&mut screen, "anchor", (17, 7).into()).unwrap();
        screen.render().unwrap();
        assert!(screen.writer().windows(3).any(|bytes| bytes == b"a=p"));
        screen.writer_mut().clear();
        screen.remove_region("child");
        screen.render().unwrap();
        assert!(!screen.writer().windows(3).any(|bytes| bytes == b"a=p"));
        screen.writer_mut().clear();
        parent.draw(&mut screen, "anchor", (1, 1).into()).unwrap();
        screen.render().unwrap();
        assert!(!screen.writer().windows(3).any(|bytes| bytes == b"a=p"));
    }

    #[test]
    fn equal_attached_rebuilds_emit_no_bytes() {
        let image = image();
        let options = ImageOptions::default();
        let parent = image.kitty_placeholders(Size::new(1, 1), &options).unwrap();
        let mut group = ImageGroup::new().unwrap();
        group
            .set("image", None, (0, 0), Size::new(4, 2), &image, &options)
            .unwrap();
        let mut screen = Screen::new(Vec::new(), (20, 8));
        screen.set_fullscreen(true);
        for frame in 0..3 {
            screen.clear();
            parent.draw(&mut screen, "anchor", (2, 2).into()).unwrap();
            group.attach(&mut screen, "group", &parent).unwrap();
            screen.set_str((3, 3), "X", Style::default());
            screen.render().unwrap();
            if frame != 0 {
                assert!(screen.writer().is_empty());
            }
            screen.writer_mut().clear();
        }
    }

    #[test]
    fn native_groups_clip_negative_offsets_and_preserve_gaps() {
        let image = image();
        let mut group = ImageGroup::new().unwrap();
        group
            .set(
                "left",
                None,
                (-2, -1),
                Size::new(4, 3),
                &image,
                &ImageOptions::default(),
            )
            .unwrap();
        group
            .set(
                "right",
                None,
                (5, 0),
                Size::new(2, 2),
                &image,
                &ImageOptions::default(),
            )
            .unwrap();
        let resolved = group
            .layout(
                Rect::new(0, 0, 8, 4),
                Position::ORIGIN,
                &[],
                None,
                &group.offsets().unwrap(),
            )
            .unwrap();
        assert_eq!(resolved.rect, Rect::new(0, 0, 7, 2));
        assert_eq!(
            resolved.coverage,
            [Rect::new(0, 0, 2, 2), Rect::new(5, 0, 2, 2)]
        );
        let prepared = resolved
            .prepare(
                Size::new(7, 2),
                RegionContext {
                    screen_size: Size::new(8, 4),
                    color_profile: uncurses::color::Profile::TrueColor,
                },
            )
            .unwrap();
        assert!(prepared.draw.iter().any(|op| matches!(op, RegionOp::Command(bytes) if bytes.windows(10).any(|part| part == b"x=16,y=16,"))));
    }
}
