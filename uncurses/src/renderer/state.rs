//! Renderer state container: the [`Renderer`] struct, its cursor model,
//! and the constructor / [`Default`] impl. Behavior methods live in the
//! sibling submodules.

use crate::Position;
use crate::cell::Cell;
use crate::color::{Color, Profile};
use crate::renderer::buffer::RenderBuffer;
use crate::renderer::caps::Optimizations;
use crate::renderer::color_cache::ColorCache;
use crate::renderer::{scroll, tabstops};
use crate::style::Style;

/// The renderer's tracked cursor: the on-screen position, the active
/// pen (style — including any open hyperlink) used for subsequent
/// glyph output, the right-margin phantom flag, and the per-axis
/// "unknown" bits that say whether each coordinate of `pos` is a
/// trustworthy reflection of where the terminal cursor actually sits.
#[derive(Debug, Clone)]
pub(super) struct Cursor {
    /// Active style applied to subsequent glyph output, including any
    /// open hyperlink. Kept in sync with the terminal's SGR + OSC 8
    /// state by [`Renderer::update_pen`]. Mutate via
    /// [`Cursor::set_style`] so the pen-cache invariant holds.
    style: Style,
    pub(super) pos: Position,
    /// Whether the cursor is parked in the right-margin "phantom" cell.
    ///
    /// After printing a glyph at the last column most terminals leave
    /// the cursor visually at that column but logically "pending wrap"
    /// — the next printable character will move it to column 0 of the
    /// next row. Tracking this state lets us avoid issuing redundant
    /// moves and lets us emit content correctly when we are about to
    /// write more glyphs.
    pub(super) at_phantom: bool,
    /// Tracks whether our model of the terminal cursor is reliable on
    /// each axis. When set, the corresponding coordinate of `pos` is
    /// "unknown" and must be reasserted before any relative move can
    /// use it as a starting point.
    ///
    /// Per-axis instead of a single flag because a `\r`-snap fixes
    /// only the column; the row is whatever the terminal happened to
    /// be on, so it stays unknown until a CUP or a deterministic
    /// vertical move clears it.
    ///
    /// Both bits are set:
    ///   * Initially: the cursor is wherever the previous program
    ///     left it.
    ///   * After a DECSTBM bracket: the terminal homes the cursor
    ///     under origin mode (DECOM set) and leaves it put without
    ///     it; we cannot tell which.
    ///
    /// Forces the next [`Renderer::move_to`] to reassert position —
    /// absolute CUP in fullscreen, `\r`-snap + relative move in
    /// inline.
    pub(super) x_unknown: bool,
    pub(super) y_unknown: bool,
    /// Cached blank cell matching the active pen.
    /// Lazily rebuilt by [`Cursor::current_blank`] when
    /// [`Cursor::blank_dirty`] is set; callers that mutate
    /// `style` directly must call
    /// [`Cursor::mark_pen_changed`] to invalidate the cache.
    blank: Cell,
    /// Set whenever the pen mutates; cleared on the next
    /// [`Cursor::current_blank`] call after a rebuild.
    blank_dirty: bool,
    /// Cached cell template matching what a back-color-erase actually
    /// paints into freed cells: a plain space carrying the active
    /// pen's bg when BCE is on, or a default blank otherwise.
    bce_blank: Cell,
    /// Cache key for [`Cursor::bce_blank`]: `(bce_flag, bg)`. `None`
    /// forces a rebuild on first access.
    bce_blank_key: Option<(bool, Option<Color>)>,
}

impl Cursor {
    /// Active style applied to subsequent glyph output (including any
    /// open hyperlink).
    #[inline]
    pub(super) fn style(&self) -> &Style {
        &self.style
    }

    /// Adopt `style` as the active pen style, invalidating any cached
    /// pen-derived blanks so they are rebuilt on next access.
    pub(super) fn set_style(&mut self, style: Style) {
        self.style = style;
        self.mark_pen_changed();
    }

    /// Invalidate the cached pen-derived blanks so the next
    /// [`Cursor::current_blank`] / [`Cursor::bce_blank`] call rebuilds.
    /// Called automatically by [`Cursor::set_style`].
    pub(super) fn mark_pen_changed(&mut self) {
        self.blank_dirty = true;
        // bce_blank also depends on style.bg; clearing its key forces
        // a re-derivation on the next access.
        self.bce_blank_key = None;
    }

    /// Return the cached blank cell for the active pen, rebuilding it
    /// lazily on first access after a pen change.
    pub(super) fn current_blank(&mut self) -> &Cell {
        if self.blank_dirty {
            self.blank = Cell::BLANK.with_style(self.style.clone());
            self.blank_dirty = false;
        }
        &self.blank
    }

    /// Return the cached BCE fill cell: a plain space carrying the
    /// active pen's bg when `bce` is on, or a default blank otherwise.
    /// `bce` is supplied by the caller since it lives on Renderer
    /// state, not Cursor.
    pub(super) fn bce_blank(&mut self, bce: bool) -> &Cell {
        let want_bg = if bce { self.style.bg } else { None };
        if self.bce_blank_key != Some((bce, want_bg)) {
            self.bce_blank = match want_bg {
                Some(bg) => {
                    let mut s = Style::EMPTY;
                    s.bg = Some(bg);
                    Cell::BLANK.with_style(s)
                }
                None => Cell::BLANK,
            };
            self.bce_blank_key = Some((bce, want_bg));
        }
        &self.bce_blank
    }
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            style: Style::default(),
            pos: Position::default(),
            at_phantom: false,
            // Until proven otherwise the cursor is wherever the
            // previous program left it; both axes are untrusted.
            x_unknown: true,
            y_unknown: true,
            blank: Cell::BLANK,
            blank_dirty: false,
            bce_blank: Cell::BLANK,
            bce_blank_key: Some((false, None)),
        }
    }
}

/// The terminal renderer — turns buffer diffs into minimal ANSI output.
pub struct Renderer {
    /// The current (on-screen) buffer state.
    pub(super) cur_buf: Option<RenderBuffer>,
    /// Staging buffer: equals the caller's front buffer after
    /// [`Renderer::sync_front`], but only the cells that genuinely
    /// differ from `cur_buf` carry `touched` flags. The diff pipeline
    /// (prepare / diff / finalize) consumes this, so rows the caller
    /// touched without actually changing are never re-emitted.
    pub(super) back_buf: RenderBuffer,
    /// Line content hashes for the current buffer.
    pub(super) old_hashes: Vec<u64>,
    /// Line content hashes for the new buffer.
    pub(super) new_hashes: Vec<u64>,
    /// Scroll mapping: oldnum[new_row] = old_row that matches, or -1.
    pub(super) oldnum: scroll::ScrollMap,
    /// Current cursor: on-screen position, active pen, plus the
    /// phantom and per-axis "unknown" tracking flags. Also owns the
    /// cached pen-derived blanks (see [`Cursor::current_blank`] /
    /// [`Cursor::bce_blank`]).
    pub(super) cur: Cursor,
    /// Snapshot of [`Renderer::cur`] taken by [`Renderer::save_cursor`].
    /// The terminal saves its own cursor across DECSET/DECRST 1049
    /// (alt-screen mode), so we mirror that here to keep our tracked
    /// position, pen, and phantom/unknown flags in sync with what the
    /// terminal will restore.
    pub(super) saved: Cursor,
    /// Terminal optimizations.
    pub(super) opts: Optimizations,
    /// Color profile for downsampling cell styles. Wrapped in a
    /// private cache so the per-frame palette lookups for Ansi /
    /// Ansi256 are memoized.
    pub(super) color_profile: ColorCache,
    /// Whether we're in fullscreen mode (alt screen).
    pub(super) fullscreen: bool,
    /// Whether to use relative cursor positioning.
    pub(super) relative_cursor: bool,
    /// Whether scroll optimization is enabled.
    pub(super) scroll_optimize: bool,
    /// Force clear on next render.
    pub(super) force_clear: bool,
    /// Width at last render (for resize detection).
    pub(super) last_width: u16,
    /// Height at last render.
    pub(super) last_height: u16,
    /// Configurable horizontal tab stops for the current line width.
    /// Used by [`super::cursor_opt`] when [`Optimizations::tabs`] is on
    /// so the planner can land exactly on a stop via `\t` characters.
    pub(super) tabs: tabstops::TabStops,
    /// Linear-probe scratch table for scroll-detection matching.
    /// Hoisted out of [`Renderer::update_hashmap`] so the storage is
    /// reused across frames; cleared and resized to `(H + 1) * 2`
    /// entries at the start of each call.
    pub(super) hashtab: Vec<scroll::HashEntry>,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            cur_buf: None,
            back_buf: RenderBuffer::new(0, 0),
            old_hashes: Vec::new(),
            new_hashes: Vec::new(),
            oldnum: scroll::ScrollMap::new(),
            cur: Cursor::default(),
            saved: Cursor::default(),
            opts: Optimizations::default(),
            color_profile: ColorCache::new(Profile::TrueColor),
            fullscreen: false,
            relative_cursor: true,
            scroll_optimize: true,
            force_clear: false,
            last_width: 0,
            last_height: 0,
            tabs: tabstops::TabStops::default_for(0),
            hashtab: Vec::new(),
        }
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub(crate) fn set_optimizations(&mut self, opts: Optimizations) {
        self.opts = opts;
    }

    /// Return the current capability set.
    pub(crate) fn optimizations(&self) -> Optimizations {
        self.opts
    }

    pub(crate) fn set_color_profile(&mut self, profile: Profile) {
        self.color_profile.set_profile(profile);
    }

    /// The active color profile.
    pub(crate) fn color_profile(&self) -> Profile {
        self.color_profile.profile()
    }

    pub(crate) fn set_fullscreen(&mut self, fullscreen: bool) {
        self.fullscreen = fullscreen;
    }

    pub(crate) fn set_relative_cursor(&mut self, relative: bool) {
        self.relative_cursor = relative;
    }

    #[allow(dead_code)] // toggle reserved for future runtime override
    pub(crate) fn set_scroll_optimize(&mut self, enabled: bool) {
        self.scroll_optimize = enabled;
    }

    /// Current cursor position as last tracked by the renderer.
    pub(crate) fn cursor_position(&self) -> Position {
        self.cur.pos
    }

    /// Override the renderer's idea of where the cursor currently is. The
    /// caller is responsible for having actually moved the terminal cursor
    /// to that position; this only updates the bookkeeping.
    pub(crate) fn set_cursor_position(&mut self, pos: Position) {
        self.cur.pos = pos;
        self.cur.at_phantom = false;
        // The caller is asserting the terminal cursor is now at `pos`
        // authoritatively. Both axes must become known so the next
        // move_to can compute a relative path from here instead of
        // falling through to a redundant absolute CUP.
        self.cur.x_unknown = false;
        self.cur.y_unknown = false;
    }

    /// Snapshot the current cursor (position + pen + phantom and
    /// per-axis "unknown" flags). Mirrors the terminal's own save
    /// behavior triggered by DECSET 1049 so that after a later
    /// [`Renderer::restore_cursor`] our tracked state matches what
    /// the terminal will have restored.
    pub(crate) fn save_cursor(&mut self) {
        self.saved = self.cur.clone();
    }

    /// Restore the cursor snapshotted by [`Renderer::save_cursor`].
    pub(crate) fn restore_cursor(&mut self) {
        self.cur = self.saved.clone();
    }
}
