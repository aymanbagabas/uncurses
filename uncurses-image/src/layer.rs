use std::io::{self, Write};

use rustc_hash::FxHashMap;

use uncurses::Rect;
use uncurses::cell::Cell;
use uncurses::screen::{Capabilities, Screen};

use crate::image_src::Image;
use crate::placement::{Erase, ImageId, Placement};
#[cfg(feature = "sixel")]
use crate::protocol::sixel::Sixel;
use crate::protocol::{
    Backend, ImageProtocol, PaintCtx, halfblocks::HalfBlocks, iterm2::Iterm2, kitty::Kitty,
};
use crate::resize::Resize;

/// Image rendering addon for a [`uncurses::screen::Screen`].
///
/// The layer is an addon, not a wrapper: each method takes a
/// `&mut Screen<W>` so the host application keeps full control of
/// the screen's lifecycle. Capabilities are passed in per call to
/// [`Self::reserve`] / [`Self::paint`] / [`Self::render`] so the
/// host can mutate the snapshot between frames (e.g. after a late
/// probe reply) and the next render picks up the new state.
///
/// # Per-frame flow
///
/// ```text
/// layer.reserve(&caps, &mut screen);  // stamp blanks/halfblocks into the surface
/// screen.render()?;                   // renderer flushes text bytes
/// layer.paint(&caps, &mut screen)?;   // raster protocols emit raw bytes
/// screen.writer_mut().flush()?;       // (caller's responsibility)
/// ```
///
/// [`Self::render`] is a convenience that performs all three.
pub struct ImageLayer {
    protocol_choice: ImageProtocol,
    /// Resolved protocol that was used during the last paint.
    /// Tracking lets us detect a mid-session protocol switch (e.g.
    /// half-blocks → Kitty after a late probe reply) and force a
    /// full redraw of every placement on the next paint.
    last_resolved: Option<ImageProtocol>,
    images: FxHashMap<ImageId, Image>,
    placements: FxHashMap<ImageId, Placement>,
    pending_erasures: Vec<Erase>,
    next_id: u64,
    halfblocks: HalfBlocks,
    kitty: Kitty,
    iterm2: Iterm2,
    #[cfg(feature = "sixel")]
    sixel: Sixel,
}

impl Default for ImageLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageLayer {
    /// Create a new layer with [`ImageProtocol::Auto`] resolution.
    pub fn new() -> Self {
        Self {
            protocol_choice: ImageProtocol::Auto,
            last_resolved: None,
            images: FxHashMap::default(),
            placements: FxHashMap::default(),
            pending_erasures: Vec::new(),
            next_id: 0,
            halfblocks: HalfBlocks,
            kitty: Kitty::new(),
            iterm2: Iterm2::default(),
            #[cfg(feature = "sixel")]
            sixel: Sixel::default(),
        }
    }

    /// Force a specific [`ImageProtocol`]. Useful for tests and for
    /// hosts that want to override the auto-resolution heuristics.
    #[must_use]
    pub fn with_protocol(mut self, protocol: ImageProtocol) -> Self {
        self.protocol_choice = protocol;
        self
    }

    /// The protocol that the next [`Self::paint`] will use, given
    /// `caps`. For [`ImageProtocol::Auto`] this is recomputed on
    /// each call.
    pub fn protocol(&self, caps: &Capabilities) -> ImageProtocol {
        self.protocol_choice.resolve(caps)
    }

    /// Register an image with the layer. The returned [`ImageId`] is
    /// stable until [`Self::remove`] is called. The image bytes are
    /// retained inside the layer so subsequent paints don't need to
    /// re-decode.
    pub fn add(&mut self, image: Image) -> ImageId {
        let id = ImageId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        self.images.insert(id, image);
        id
    }

    /// Remove an image. If the image is currently placed, its
    /// placement is also removed and queued for erasure on the next
    /// paint.
    pub fn remove(&mut self, id: ImageId) {
        if self.images.remove(&id).is_some() {
            self.halfblocks.on_image_removed(id);
            self.kitty.on_image_removed(id);
            self.iterm2.on_image_removed(id);
            #[cfg(feature = "sixel")]
            self.sixel.on_image_removed(id);
            if let Some(p) = self.placements.remove(&id) {
                self.pending_erasures.push(Erase {
                    area: p.area,
                    backend_handle: None,
                });
            }
        }
    }

    /// Place (or move) an image at `area` with the given resize
    /// strategy. If the placement already exists at the same area
    /// with the same resize, this is a no-op except that the
    /// underlying image's content hash is checked on the next
    /// paint — content changes still trigger a repaint.
    pub fn place(&mut self, id: ImageId, area: Rect, resize: Resize) {
        if !self.images.contains_key(&id) {
            return;
        }
        match self.placements.get_mut(&id) {
            Some(existing) if existing.area == area && existing.resize == resize => {
                // Idempotent: hash check at paint time still catches
                // content-only changes.
            }
            Some(existing) => {
                if existing.area != area {
                    self.pending_erasures.push(Erase {
                        area: existing.area,
                        backend_handle: None,
                    });
                }
                existing.area = area;
                existing.resize = resize;
                existing.last_painted_hash = None;
                existing.last_cell_px = None;
            }
            None => {
                self.placements.insert(id, Placement::new(area, resize));
            }
        }
    }

    /// Remove a placement. The image itself stays registered so a
    /// later [`Self::place`] is cheap.
    pub fn unplace(&mut self, id: ImageId) {
        if let Some(p) = self.placements.remove(&id) {
            self.pending_erasures.push(Erase {
                area: p.area,
                backend_handle: None,
            });
        }
    }

    /// Remove every placement (but keep all registered images).
    pub fn clear_placements(&mut self) {
        for (_, p) in self.placements.drain() {
            self.pending_erasures.push(Erase {
                area: p.area,
                backend_handle: None,
            });
        }
    }

    /// Mark every placement dirty so the next paint re-encodes them
    /// from scratch. Useful after a `Screen::invalidate` so terminal
    /// state and layer state stay in sync.
    pub fn invalidate(&mut self) {
        for p in self.placements.values_mut() {
            p.last_painted_hash = None;
            p.last_cell_px = None;
        }
    }

    /// Stamp cells for every active placement (and emit any pre-frame
    /// payload for protocols that need it) into `screen`. Must be
    /// called before [`Screen::render`].
    pub fn reserve<W: Write>(
        &mut self,
        caps: &Capabilities,
        screen: &mut Screen<W>,
    ) -> io::Result<()> {
        let resolved = self.protocol(caps);

        // Detect a mid-session protocol switch and force a full
        // re-encode of every placement.
        if self.last_resolved != Some(resolved) {
            for p in self.placements.values_mut() {
                p.last_painted_hash = None;
                p.last_cell_px = None;
            }
        }

        // Per-backend `erase` handling (in `paint`) still receives
        // these erasure regions for protocol-side cleanup
        // (e.g. Kitty image delete). The cells themselves are owned
        // by the host: the host's next frame is expected to fill the
        // cells outside any current placement with whatever it wants
        // there. Blanking them here would clobber that content (e.g.
        // a backdrop the host drew immediately before render) without
        // actually clearing terminal-side raster pixels — the
        // renderer's diff only emits text where the front buffer
        // changed, and `reserve` on the previous frame had already
        // blanked these cells, so a blank→blank diff produces no
        // output and any old raster pixels persist anyway.

        for (id, placement) in self.placements.iter() {
            let Some(image) = self.images.get(id) else {
                continue;
            };
            let ctx = PaintCtx {
                id: *id,
                image,
                placement,
                caps,
            };
            match resolved {
                ImageProtocol::HalfBlocks => self.halfblocks.reserve(&ctx, screen)?,
                ImageProtocol::Kitty => self.kitty.reserve(&ctx, screen)?,
                ImageProtocol::Iterm2 => self.iterm2.reserve(&ctx, screen)?,
                #[cfg(feature = "sixel")]
                ImageProtocol::Sixel => self.sixel.reserve(&ctx, screen)?,
                _ => fill_blanks(screen, placement.area),
            }
        }
        Ok(())
    }

    /// Emit any post-frame payload (sixel, iTerm2 inline) and any
    /// pending terminal-side cleanup. Must be called after
    /// [`Screen::render`].
    pub fn paint<W: Write>(
        &mut self,
        caps: &Capabilities,
        screen: &mut Screen<W>,
    ) -> io::Result<()> {
        let resolved = self.protocol(caps);
        let cell_px = caps.cell_pixel_size;

        // Backend-side cleanup for any pending erasures.
        for erase in self.pending_erasures.drain(..) {
            match resolved {
                ImageProtocol::HalfBlocks => self.halfblocks.erase(&erase, screen)?,
                ImageProtocol::Kitty => self.kitty.erase(&erase, screen)?,
                ImageProtocol::Iterm2 => self.iterm2.erase(&erase, screen)?,
                #[cfg(feature = "sixel")]
                ImageProtocol::Sixel => self.sixel.erase(&erase, screen)?,
                _ => {}
            }
        }

        // Per-placement paint (post-frame). For Kitty / half-blocks
        // this is a no-op — those protocols emit everything during
        // `reserve`. Future raster backends fill this in.
        for (id, placement) in self.placements.iter_mut() {
            let Some(image) = self.images.get(id) else {
                continue;
            };
            if !placement.needs_repaint(image.content_hash(), cell_px) {
                continue;
            }
            let ctx = PaintCtx {
                id: *id,
                image,
                placement,
                caps,
            };
            match resolved {
                ImageProtocol::HalfBlocks => self.halfblocks.paint(&ctx, screen)?,
                ImageProtocol::Kitty => self.kitty.paint(&ctx, screen)?,
                ImageProtocol::Iterm2 => self.iterm2.paint(&ctx, screen)?,
                #[cfg(feature = "sixel")]
                ImageProtocol::Sixel => self.sixel.paint(&ctx, screen)?,
                _ => {}
            }
            placement.mark_painted(image.content_hash(), cell_px);
        }

        // Per-frame finalize (e.g. flush queued Kitty deletes).
        match resolved {
            ImageProtocol::HalfBlocks => self.halfblocks.finalize(screen)?,
            ImageProtocol::Kitty => self.kitty.finalize(screen)?,
            ImageProtocol::Iterm2 => self.iterm2.finalize(screen)?,
            #[cfg(feature = "sixel")]
            ImageProtocol::Sixel => self.sixel.finalize(screen)?,
            _ => {}
        }

        self.last_resolved = Some(resolved);
        Ok(())
    }

    /// Convenience wrapper: `reserve` → `screen.render()` → `paint`.
    /// Returns whatever `screen.render()` and `paint` produce.
    pub fn render<W: Write>(
        &mut self,
        caps: &Capabilities,
        screen: &mut Screen<W>,
    ) -> io::Result<()> {
        self.reserve(caps, screen)?;
        screen.render()?;
        self.paint(caps, screen)
    }

    /// Release any terminal-side resources owned by the layer (e.g.
    /// the Kitty image registry). Idempotent.
    pub fn shutdown<W: Write>(&mut self, screen: &mut Screen<W>) -> io::Result<()> {
        self.halfblocks.shutdown(screen)?;
        self.kitty.shutdown(screen)?;
        self.iterm2.shutdown(screen)?;
        #[cfg(feature = "sixel")]
        self.sixel.shutdown(screen)?;
        Ok(())
    }
}

/// Fill `area` with blank cells. Used by raster protocols to make the
/// renderer wipe pixels under a placement so the backend's payload
/// lands on a clean surface.
fn fill_blanks<W: Write>(screen: &mut Screen<W>, area: Rect) {
    let sw = screen.width();
    let sh = screen.height();
    for y in area.y..area.y.saturating_add(area.height).min(sh) {
        for x in area.x..area.x.saturating_add(area.width).min(sw) {
            screen.set_cell((x, y), &Cell::BLANK);
        }
    }
}
