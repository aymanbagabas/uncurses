use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
    sync::Arc,
};

use uncurses::{buffer::Surface, cell::Cell, color::Profile, layout::Position, screen::Screen};

use super::ImageRenderer;
use crate::CellImage;
use crate::protocols::kitty;
use crate::protocols::kitty_placeholders::{Resource, compression, image_id};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Upload {
    Pending,
    Complete,
}

#[derive(Debug, Default)]
pub(super) struct Resources {
    known: BTreeMap<u32, Arc<Resource>>,
    uploads: [BTreeMap<u32, Upload>; 2],
    committed: BTreeSet<u32>,
    fullscreen: Option<bool>,
    cleanup_failed: bool,
}

impl Resources {
    pub fn track(&mut self, resource: Arc<Resource>) {
        self.known.entry(resource.id).or_insert(resource);
    }

    fn visible<W: Write>(&self, screen: &Screen<W>, rows: u16) -> BTreeSet<u32> {
        let mut visible = BTreeSet::new();
        if self.known.is_empty() {
            return visible;
        }
        for y in 0..rows.min(screen.size().height) {
            for x in 0..screen.size().width {
                if let Some(id) = screen.cell((x, y).into()).and_then(image_id)
                    && self.known.contains_key(&id)
                {
                    visible.insert(id);
                }
            }
        }
        visible
    }

    fn prepare<W: Write>(
        &mut self,
        screen: &mut Screen<W>,
        visible: &BTreeSet<u32>,
        tmux: bool,
    ) -> io::Result<Vec<u32>> {
        if self.cleanup_failed {
            return Err(io::Error::other(
                "image cleanup is uncertain; finish pending output, restore terminal state, then invalidate before rendering",
            ));
        }
        if !visible.is_empty() && screen.color_profile() != Profile::TrueColor {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Kitty placeholders require true color",
            ));
        }
        let fullscreen = screen.fullscreen();
        if self.fullscreen != Some(fullscreen) {
            // Each alternate-screen visit starts with an empty graphics buffer.
            self.uploads[1].clear();
            self.fullscreen = Some(fullscreen);
        }
        let mode = usize::from(fullscreen);
        let mut prepared = Vec::new();
        for id in visible {
            if self.uploads[mode].get(id) != Some(&Upload::Complete) {
                write_commands(screen, &self.known[id].upload, tmux)?;
                self.uploads[mode].insert(*id, Upload::Pending);
                prepared.push(*id);
            }
        }
        Ok(prepared)
    }

    fn complete<W: Write>(
        &mut self,
        screen: &mut Screen<W>,
        visible: &BTreeSet<u32>,
        prepared: &[u32],
        tmux: bool,
    ) -> io::Result<()> {
        let mode = usize::from(screen.fullscreen());
        // A failed Screen frame can discard staged uploads. Confirm only after success.
        for id in prepared {
            self.uploads[mode].insert(*id, Upload::Complete);
        }
        let retired: Vec<_> = self.uploads[mode]
            .keys()
            .filter(|id| !visible.contains(id))
            .filter(|id| screen.fullscreen() || !self.committed.contains(id))
            .copied()
            .collect();
        for id in &retired {
            write_commands(screen, &self.known[id].delete, tmux)?;
        }
        if !retired.is_empty()
            && let Err(error) = screen.flush()
        {
            self.cleanup_failed = true;
            return Err(error);
        }
        for id in retired {
            self.uploads[mode].remove(&id);
        }
        self.known.retain(|id, resource| {
            Arc::strong_count(resource) > 1
                || visible.contains(id)
                || (self.uploads[0].contains_key(id) && !self.committed.contains(id))
                || self.uploads[1].contains_key(id)
        });
        Ok(())
    }

    fn compress<W: Write>(
        &self,
        screen: &mut Screen<W>,
        visible: &BTreeSet<u32>,
    ) -> Vec<(Position, Cell)> {
        if visible.is_empty() {
            return Vec::new();
        }
        let mut replacements = Vec::new();
        for y in 0..screen.size().height {
            for x in 1..screen.size().width {
                let position = Position::new(x, y);
                let current = screen.cell(position).unwrap();
                if image_id(current).is_some_and(|id| visible.contains(&id))
                    && let Some(cell) =
                        compression::compressed(screen.cell((x - 1, y).into()).unwrap(), current)
                {
                    replacements.push((position, current.clone(), cell));
                }
            }
        }
        replacements
            .into_iter()
            .map(|(position, original, compressed)| {
                screen.set_cell(position, &compressed);
                (position, original)
            })
            .collect()
    }
}

fn write_commands<W: Write>(screen: &mut Screen<W>, bytes: &[u8], tmux: bool) -> io::Result<()> {
    if tmux {
        screen.write_all(&kitty::wrap_tmux(bytes)?)
    } else {
        screen.write_all(bytes)
    }
}

impl ImageRenderer {
    /// Compose prepared placeholder cells and retain their image resource.
    ///
    /// Ordinary surface clipping applies. The screen must use true color.
    /// Finish the frame with [`render`](Self::render) to upload the pixels.
    pub fn draw_cells<W: Write>(
        &mut self,
        screen: &mut Screen<W>,
        image: &CellImage,
        position: impl Into<Position>,
    ) -> io::Result<()> {
        if screen.color_profile() != Profile::TrueColor {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Kitty placeholders require true color",
            ));
        }
        self.placeholders.track(image.resource.clone());
        image.cells().draw(screen, position.into());
        Ok(())
    }

    /// Render a frame with image-owned placeholder resources.
    ///
    /// Uploads precede cell output. Deletion follows successful removal of the
    /// final visible cells. Placeholder compression affects wire output while
    /// the desired screen keeps complete coordinates for ordinary composition.
    ///
    /// After an output error, finish pending output with [`Write::flush`],
    /// restore terminal state, and call [`invalidate`](Self::invalidate).
    pub fn render<W: Write>(&mut self, screen: &mut Screen<W>) -> io::Result<()> {
        let visible = self.placeholders.visible(screen, screen.size().height);
        let prepared = self
            .placeholders
            .prepare(screen, &visible, self.tmux_passthrough)?;
        let originals = self.placeholders.compress(screen, &visible);
        let result = screen.render();
        for (position, cell) in originals {
            screen.set_cell(position, &cell);
        }
        result?;
        self.placeholders
            .complete(screen, &visible, &prepared, self.tmux_passthrough)
    }

    /// Commit inline rows while preserving their placeholder resources.
    ///
    /// Counts and opaque placement boundaries follow [`Screen::commit_rows`].
    /// Committed image IDs stay in the terminal after their cached data drops.
    /// A final flush error can release rows; retry that output with
    /// [`Write::flush`] rather than committing those rows again.
    pub fn commit_rows<W: Write>(&mut self, screen: &mut Screen<W>, count: u16) -> io::Result<()> {
        let count = count.min(screen.size().height);
        if count == 0 || screen.fullscreen() {
            return Ok(());
        }
        let visible = self.placeholders.visible(screen, screen.size().height);
        let committed = self.placeholders.visible(screen, count);
        let prepared = self
            .placeholders
            .prepare(screen, &visible, self.tmux_passthrough)?;
        let height = screen.size().height;
        let result = screen.commit_rows(count);
        if screen.size().height < height {
            self.placeholders.committed.extend(committed);
        }
        result?;
        let retained = self.placeholders.visible(screen, screen.size().height);
        self.placeholders
            .complete(screen, &retained, &prepared, self.tmux_passthrough)
    }

    /// Restore image uploads and the cell frame after terminal recovery.
    ///
    /// Use this after a terminal reset, lost output, or a resize that discards
    /// graphics. Also use it after screen switches between image frames.
    /// Complete pending writes and restore the terminal parser and cursor
    /// before the next [`render`](Self::render).
    pub fn invalidate<W: Write>(&mut self, screen: &mut Screen<W>) {
        for uploads in &mut self.placeholders.uploads {
            uploads
                .values_mut()
                .for_each(|state| *state = Upload::Pending);
        }
        self.placeholders.cleanup_failed = false;
        screen.invalidate();
    }
}
