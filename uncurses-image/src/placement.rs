use uncurses::Rect;

use crate::resize::Resize;

/// Stable handle for an image registered with [`crate::ImageLayer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImageId(pub(crate) u64);

impl ImageId {
    /// The raw integer value. Useful for logging or when bridging
    /// across an FFI boundary.
    pub fn raw(self) -> u64 {
        self.0
    }
}

/// Where an image is currently placed on screen, plus the resize
/// policy to apply when encoding.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Placement {
    pub area: Rect,
    pub resize: Resize,
    /// Hash of the source image at the time of last successful paint.
    /// `None` means "never painted yet".
    pub last_painted_hash: Option<u64>,
    /// `cell_pixel_size` at the time of last paint, for raster
    /// protocols. A change forces re-encoding.
    pub last_cell_px: Option<(u16, u16)>,
}

impl Placement {
    pub fn new(area: Rect, resize: Resize) -> Self {
        Self {
            area,
            resize,
            last_painted_hash: None,
            last_cell_px: None,
        }
    }

    /// Returns true if the placement needs to be repainted given the
    /// current image hash and cell-pixel size.
    pub fn needs_repaint(&self, hash: u64, cell_px: Option<(u16, u16)>) -> bool {
        self.last_painted_hash != Some(hash) || self.last_cell_px != cell_px
    }

    pub fn mark_painted(&mut self, hash: u64, cell_px: Option<(u16, u16)>) {
        self.last_painted_hash = Some(hash);
        self.last_cell_px = cell_px;
    }
}

/// A queued erasure for a placement that was removed or moved. The
/// renderer's diff naturally wipes the cells; raster protocols may
/// need an additional registry-side delete (Kitty).
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // backend_handle used by raster backends
pub(crate) struct Erase {
    pub area: Rect,
    /// Backend-specific handle (e.g. Kitty image id) for terminal-side
    /// cleanup. `None` for protocols without a registry.
    pub backend_handle: Option<u32>,
}
