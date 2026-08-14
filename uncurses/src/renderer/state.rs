//! Renderer state container: the [`Renderer`] struct, its cursor model,
//! and the constructor / [`Default`] impl. Behavior methods live in the
//! sibling submodules.

use crate::cell::Cell;
use crate::color::{Color, Profile};
use crate::layout::Position;
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
    /// Tracked cursor column, or `None` when the column is unknown.
    ///
    /// `None` means the value is untrustworthy and the position must be
    /// reasserted before a relative move can start from it: a `\r`-snap in
    /// inline mode, or an absolute `CUP` in fullscreen.
    pub(super) x: Option<u16>,
    /// Tracked cursor row, or `None` when the row is unknown. See [`x`](Self::x).
    ///
    /// Both axes are `None`:
    ///   * Initially: the cursor is wherever the previous program left it.
    ///   * After a DECSTBM bracket: the terminal homes the cursor under
    ///     origin mode (DECOM set) and leaves it put without it; we cannot
    ///     tell which.
    ///   * After [`Renderer::invalidate_cursor`] (e.g. a shell handoff): our
    ///     model is voided so the next move re-anchors instead of trusting a
    ///     stale row.
    pub(super) y: Option<u16>,
    /// Whether the cursor is parked in the right-margin "phantom" cell.
    ///
    /// After printing a glyph at the last column most terminals leave
    /// the cursor visually at that column but logically "pending wrap"
    /// — the next printable character will move it to column 0 of the
    /// next row. Tracking this state lets us avoid issuing redundant
    /// moves and lets us emit content correctly when we are about to
    /// write more glyphs.
    pub(super) at_phantom: bool,
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
    /// The tracked position with any unknown axis resolved to `0`.
    ///
    /// Only meaningful when the relevant axis is [`known`](Self::known); a
    /// `None` axis resolves to `0`, which the relative-move planner treats as
    /// "assume the top-left" when re-anchoring.
    #[inline]
    pub(super) fn pos(&self) -> Position {
        Position {
            x: self.x.unwrap_or(0),
            y: self.y.unwrap_or(0),
        }
    }

    /// Set both axes to a known position.
    #[inline]
    pub(super) fn set_pos(&mut self, pos: Position) {
        self.x = Some(pos.x);
        self.y = Some(pos.y);
    }

    /// Whether both axes of the tracked position are known.
    #[inline]
    pub(super) fn known(&self) -> bool {
        self.x.is_some() && self.y.is_some()
    }

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
            self.blank = Cell::BLANK.style(self.style.clone());
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
                    let s = Style {
                        bg: Some(bg),
                        ..Style::default()
                    };
                    Cell::BLANK.style(s)
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
            // Until proven otherwise the cursor is wherever the previous
            // program left it; both axes are untrusted (unknown).
            x: None,
            y: None,
            at_phantom: false,
            blank: Cell::BLANK,
            blank_dirty: false,
            bce_blank: Cell::BLANK,
            bce_blank_key: Some((false, None)),
        }
    }
}

/// Stateful terminal renderer that turns cell-buffer diffs into bytes.
///
/// `Renderer` owns the tracked on-screen buffer, the staging buffer used
/// by the buffer sync path, cursor/pen state, terminal capability flags,
/// color-profile conversion cache, scroll-detection scratch storage, and
/// tab-stop tables. It writes only to caller-provided byte buffers; the
/// caller decides when those bytes are flushed to an I/O sink.
///
/// # Usage notes
///
/// Most callers should use [`crate::screen::Screen`]. Direct use is
/// appropriate when another abstraction owns the output buffering but
/// wants the same diff, cursor, style, and scroll planning.
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
    /// Used by the cursor planner when [`Optimizations::TABS`] is on so
    /// the planner can land exactly on a stop via `\t` characters.
    pub(super) tabs: tabstops::TabStops,
    /// Linear-probe scratch table for scroll-detection matching.
    /// Hoisted out of [`Renderer::update_hashmap`] so the storage is
    /// reused across frames; cleared and resized to `(H + 1) * 2`
    /// entries at the start of each call.
    pub(super) hashtab: Vec<scroll::HashEntry>,
}

impl Renderer {
    /// Create a renderer with default state and no current buffer.
    ///
    /// # Returns
    ///
    /// A renderer using the default optimization set, true-color output,
    /// inline-relative cursor mode, scroll optimization enabled, no
    /// tracked terminal contents, and unknown cursor coordinates.
    ///
    /// # Panics
    ///
    /// Never panics.
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
    /// Replace the terminal capability set used by future plans.
    ///
    /// # Parameters
    ///
    /// - `opts`: capability flags to use for cursor movement, clears,
    ///   repeats, insert/delete operations, scrolls, and ONLCR handling.
    pub(crate) fn set_optimizations(&mut self, opts: Optimizations) {
        self.opts = opts;
    }

    /// Return the current capability set.
    ///
    /// # Returns
    ///
    /// The active [`Optimizations`] value.
    pub(crate) fn optimizations(&self) -> Optimizations {
        self.opts
    }

    /// Replace the color profile used for future style emission.
    ///
    /// Clears cached color conversions when `profile` differs from the
    /// current value.
    pub(crate) fn set_color_profile(&mut self, profile: Profile) {
        if self.color_profile.profile() != profile {
            self.request_clear();
        }
        self.color_profile.set_profile(profile);
    }

    /// Return the active color profile.
    ///
    /// # Returns
    ///
    /// The [`Profile`] currently used to convert cell styles before SGR
    /// and OSC 8 emission.
    pub(crate) fn color_profile(&self) -> Profile {
        self.color_profile.profile()
    }

    /// Set whether the renderer targets a fullscreen viewport.
    ///
    /// Fullscreen mode allows absolute screen assumptions and protects
    /// the lower-right cell from autowrap scrolling. Inline mode keeps
    /// output relative to the application surface.
    pub(crate) fn set_fullscreen(&mut self, fullscreen: bool) {
        self.fullscreen = fullscreen;
    }

    /// Set whether cursor moves should be planned relative to the
    /// current tracked position.
    ///
    /// Inline surfaces use relative movement so they do not address
    /// rows outside the managed surface. Fullscreen surfaces can disable
    /// this and allow absolute CUP/CHA/HPA/VPA candidates.
    pub(crate) fn set_relative_cursor(&mut self, relative: bool) {
        self.relative_cursor = relative;
    }

    #[allow(dead_code)] // toggle reserved for future runtime override
    /// Enable or disable hash-based scroll optimization.
    ///
    /// When disabled, touched rows fall through to direct row diffs even
    /// if line hashes indicate a scroll could be cheaper.
    pub(crate) fn set_scroll_optimize(&mut self, enabled: bool) {
        self.scroll_optimize = enabled;
    }

    /// Current cursor position as last tracked by the renderer.
    ///
    /// # Returns
    ///
    /// Buffer-relative zero-based position. The coordinates may be stale
    /// if [`Renderer::cursor_known`] is false.
    pub(crate) fn cursor_position(&self) -> Position {
        self.cur.pos()
    }

    /// Return whether the tracked cursor position is known on both axes.
    ///
    /// Returns `false` initially, after [`Renderer::invalidate_cursor`],
    /// and around operations such as scroll-region changes that may home
    /// the physical cursor under terminal modes the renderer cannot
    /// observe.
    pub(crate) fn cursor_known(&self) -> bool {
        self.cur.known()
    }

    /// Surface dimensions captured at the most recent render. Returns
    /// `(0, 0)` before the first render. Differs from the
    /// [`Screen`](crate::screen::Screen)'s live size when the terminal has
    /// resized but no frame has been rendered yet — useful when
    /// teardown needs to address the *rendered* surface rather than
    /// rows that were never drawn.
    pub(crate) fn last_size(&self) -> (u16, u16) {
        (self.last_width, self.last_height)
    }

    /// Override the renderer's idea of where the cursor currently is.
    ///
    /// The caller is responsible for having actually moved the terminal
    /// cursor to `pos`; this only updates bookkeeping and marks both
    /// axes known.
    pub(crate) fn set_cursor_position(&mut self, pos: Position) {
        // The caller is asserting the terminal cursor is now at `pos`
        // authoritatively. Both axes become known so the next move_to can
        // compute a relative path from here instead of falling through to a
        // redundant absolute CUP.
        self.cur.set_pos(pos);
        self.cur.at_phantom = false;
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
