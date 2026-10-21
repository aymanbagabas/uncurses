//! Region content owned and drawn by [`Screen`](crate::screen::Screen).
//!
//! A [`Region`] describes one opaque native block.
//! [`Screen::set_region`](crate::screen::Screen::set_region) supplies its stable
//! placement key and rectangle.
//!
//! Regions use the screen's inline or fullscreen coordinates. Applications
//! select a supported protocol before they place content.

use std::any::Any;
use std::io::{self, Write};
use std::sync::Arc;

use super::Screen;
use crate::buffer::{Bounded, Surface};
use crate::color::Profile;
use crate::layout::{Position, Rect, Size};
use crate::style::Style;

/// An immutable, opaque block with a native terminal representation.
///
/// Implementations can live in other crates. The renderer tracks desired
/// placements and displayed ownership. Later cell writes appear over the
/// affected parts of a region at their requested coordinates. A later region
/// placement replaces intersecting blocks. Redeclaring a region draws it over
/// previous text edits.
///
/// # Implementation contract
///
/// [`prepare`](Self::prepare) returns owned draw and cleanup operations.
/// All positions are local to the block. The renderer applies its cursor and
/// style planners when it emits these operations.
///
/// Cleanup must leave default blank cells throughout the block. The renderer
/// then restores exposed desired text. [`RegionOp::Erase`] supplies rectangular
/// character erasure. A native protocol that needs other cleanup can prepare
/// its own bytes under the same effect contract.
///
/// Native payloads must preserve modes and pen state, stay inside the block,
/// and avoid scrolling, wrapping, or affecting other placements. These effects
/// are the implementer's responsibility; the renderer validates operation
/// coordinates, not arbitrary protocol bytes.
///
/// Inline output uses the terminal's DEC saved cursor to recover after native
/// writes. Payloads must preserve that save slot. Applications must not retain
/// their own saved cursor across region output.
///
/// Content must remain immutable. `same` must compare every property that
/// affects output. Equal content can retain the
/// displayed placement without another call to `prepare`.
///
/// Text overlays use cell erasure before replacement. The native protocol must
/// support this operation; a terminal may erase an entire multi-cell glyph
/// when an overlay touches part of it.
///
/// Removing a region exposes the current desired cells beneath its footprint,
/// including text edited after placement. Full-surface `clear` and `fill`
/// replace the desired cells and remove region declarations together.
///
/// # Example
///
/// A one-cell marker can use the same interface as sized text:
///
/// ```
/// use std::io;
/// use uncurses::layout::{Position, Rect, Size};
/// use uncurses::screen::{PreparedRegion, Region, RegionContext, RegionOp, Screen};
///
/// struct Marker;
///
/// impl Region for Marker {
///     fn same(&self, other: &dyn Region) -> bool {
///         other.downcast_ref::<Self>().is_some()
///     }
///
///     fn prepare(&self, size: Size, _: RegionContext) -> io::Result<PreparedRegion> {
///         if size != Size::new(1, 1) {
///             return Err(io::Error::new(io::ErrorKind::InvalidInput, "expected one cell"));
///         }
///         Ok(PreparedRegion {
///             draw: vec![RegionOp::MoveTo(Position::ORIGIN), RegionOp::Write(b"#".to_vec())],
///             erase: vec![RegionOp::Erase(Rect::new(0, 0, 1, 1))],
///         })
///     }
/// }
///
/// let mut screen = Screen::new(Vec::new(), (20, 4));
/// screen.set_fullscreen(true);
/// screen.set_region("marker", Rect::new(2, 1, 1, 1), Marker)?;
/// screen.render()?;
/// assert!(screen.writer().contains(&b'#'));
/// # Ok::<(), io::Error>(())
/// ```
pub trait Region: Any + Send + Sync {
    /// Compare immutable content, independently of its placement key.
    fn same(&self, other: &dyn Region) -> bool;

    /// Optional disjoint local rectangles occupied by this content.
    ///
    /// `None` occupies the whole declaration. Holes preserve ordinary cells.
    /// Rectangles must be nonempty, disjoint, and inside the declaration.
    fn coverage(&self) -> Option<&[Rect]> {
        None
    }

    /// Prepare draw and cleanup operations for a block of `size`.
    ///
    /// Preparation runs before the renderer changes its frame state. An error
    /// preserves the displayed frame, queued bytes, and desired edits.
    ///
    /// Equal content with the same size and context reuses its prepared
    /// operations, including when it moves. Cleanup and replay use the retained
    /// operations without another call to this method.
    fn prepare(&self, size: Size, context: RegionContext) -> io::Result<PreparedRegion>;

    /// Prepare native content clipped to the cells still visible above text.
    ///
    /// Rectangles use local coordinates and contain disjoint row spans. Return
    /// `Some` when the protocol needs explicit clipping instead of character
    /// erasure. Return `None` to use the cached ordinary preparation.
    ///
    /// This method has the same effect and error guarantees as `prepare`.
    fn prepare_visible(
        &self,
        _size: Size,
        _context: RegionContext,
        _visible: &mut dyn Iterator<Item = Rect>,
    ) -> io::Result<Option<PreparedRegion>> {
        Ok(None)
    }
}

/// Resolve a native group from anchors in the current ordinary cells.
///
/// The screen refreshes these declarations before ordered edits and rendering.
/// Return `None` while all anchors are absent. A later anchor restores the
/// declaration. Full `clear`, `fill`, or explicit removal removes it permanently.
///
/// Resolution must be read-only and inexpensive. Decode images and prepare
/// reusable data before installing the declaration. Errors surface from
/// `render` before terminal output.
pub trait AnchoredRegion: Send + Sync {
    /// Resolve the current footprint and immutable native content.
    fn resolve(&self, cells: RegionCells<'_>) -> io::Result<Option<RegionPlacement>>;
}

/// Read-only desired cells for anchor resolution.
#[derive(Clone, Copy)]
pub struct RegionCells<'a>(pub(crate) &'a crate::renderer::RenderBuffer);

impl RegionCells<'_> {
    /// Rows touched since the previous frame.
    ///
    /// Scan all rows on first resolution and after a bounds change.
    /// Later resolutions can reuse cached matches in untouched rows.
    /// The screen resolves anchors before it clears these flags.
    pub fn changed_rows(&self) -> impl Iterator<Item = u16> + '_ {
        self.0.touched_lines().map(|(y, _)| y)
    }
}

impl Bounded for RegionCells<'_> {
    fn bounds(&self) -> Rect {
        self.0.bounds()
    }
}

impl Surface for RegionCells<'_> {
    fn cell(&self, position: Position) -> Option<&crate::cell::Cell> {
        self.0.cell(position)
    }
}

/// One resolved declaration from an [`AnchoredRegion`].
pub struct RegionPlacement {
    /// Reference cell that establishes placement order when it moves.
    pub anchor: Position,
    /// The visible native bounds, including any local operation coordinates.
    pub rect: Rect,
    /// Immutable content within those bounds.
    pub content: Arc<dyn Region>,
}

impl dyn Region {
    /// Borrow concrete content when it has type `T`, for a `same` implementation.
    pub fn downcast_ref<T: Region>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref()
    }
}

/// Immutable environment for region preparation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionContext {
    /// The managed viewport size.
    pub screen_size: Size,
    /// The active output color profile.
    pub color_profile: Profile,
}

/// An operation within a region's local coordinates.
///
/// The renderer supplies the placement origin during emission. Native bytes
/// must obey the [`Region`] effect contract. The cursor becomes unknown after
/// [`Write`](Self::Write); use [`SetCursor`](Self::SetCursor) only when the
/// protocol guarantees the position and clears pending wrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegionOp {
    /// Retain a shared terminal resource before any frame cells are emitted.
    ///
    /// This declaration emits bytes only when the resource needs creation.
    /// The renderer releases resources after their last placement or explicit
    /// screen binding disappears.
    Resource(Arc<RegionResource>),
    /// Emit a position-independent command that preserves cursor, pen, and modes.
    ///
    /// Use this for protocol cleanup by identifier. Unlike `Write`, this
    /// operation can run when an old placement lies outside a resized screen.
    Command(Vec<u8>),
    /// Move to a cell inside the block through the renderer's cursor planner.
    MoveTo(Position),
    /// Apply a style through the renderer's color and pen handling.
    SetStyle(Style),
    /// Emit a complete native payload at the current cursor.
    ///
    /// Consecutive writes retain their native cursor effects. In inline mode,
    /// a later move, erase, or recipe completion restores the saved cursor
    /// unless [`SetCursor`](Self::SetCursor) establishes a known position.
    Write(Vec<u8>),
    /// Erase a local rectangle to default blank cells with ECH.
    ///
    /// This operation resets the pen and can clip cleanup to a smaller
    /// viewport. Use protocol-specific payloads when ECH cannot remove content.
    Erase(Rect),
    /// Record a known local cursor position without emitting a move.
    SetCursor(Position),
}

/// Immutable, position-independent terminal resource recipes.
///
/// Both byte sequences must preserve the cursor, pen, and terminal modes.
/// Dependencies are created first and released last. Resource identity follows
/// the shared `Arc`, not byte equality. Share the same allocation across uses.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RegionResource {
    /// Resources required by this resource's creation or lifetime.
    pub dependencies: Vec<Arc<RegionResource>>,
    /// Bytes that create or replace this resource.
    pub create: Vec<u8>,
    /// Bytes that release this resource.
    pub release: Vec<u8>,
    /// Minimum output profile required by cells that encode resource identifiers.
    ///
    /// The renderer checks this before output, including after profile changes.
    pub minimum_color_profile: Option<Profile>,
}

/// Owned draw and cleanup recipes returned by [`Region::prepare`].
///
/// Operations run in order. Each recipe starts at the block's origin with the
/// default style. The renderer retains cleanup with displayed ownership, even
/// after the caller removes or replaces its declaration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreparedRegion {
    /// Operations that draw the complete opaque block.
    pub draw: Vec<RegionOp>,
    /// Operations that remove the block and leave default blanks.
    pub erase: Vec<RegionOp>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

impl<W: Write> Screen<W> {
    /// Attach native content to ordinary cells.
    ///
    /// Anchor motion establishes a new placement order. Later text overlays
    /// apply at the new position. Anchor removal exposes underlying text.
    pub fn set_anchored_region(&mut self, key: &str, region: impl AnchoredRegion + 'static) {
        self.refresh_anchored_regions();
        self.renderer
            .set_anchored_region(key, Arc::new(region), &mut self.front_buf);
    }

    #[inline]
    pub(super) fn refresh_anchored_regions(&mut self) {
        if self
            .renderer
            .regions
            .as_ref()
            .is_some_and(|regions| regions.has_anchors())
        {
            self.renderer.refresh_anchored_regions(&mut self.front_buf);
        }
    }

    /// Retain a resource for ordinary cells under a stable screen-local key.
    ///
    /// Creation precedes text output in `render`. Full `clear` and `fill`
    /// remove these bindings along with region declarations. This method
    /// stages ownership without writing to the terminal.
    pub fn set_region_resource(&mut self, key: &str, resource: Arc<RegionResource>) {
        self.renderer.set_region_resource(key, resource);
    }

    /// Remove an explicit resource binding.
    ///
    /// Regions and other bindings can still retain the same resource.
    pub fn remove_region_resource(&mut self, key: &str) {
        self.renderer.remove_region_resource(key);
    }

    pub(super) fn insert_above_regions(&mut self, content: &str) -> io::Result<()> {
        let size = self.size();
        self.renderer.validate_region_replay(size)?;
        let redraw = self.renderer.redraw_pending();
        self.renderer
            .begin_frame(&mut self.out_buf, self.cursor_visible);
        if self.renderer.last_size() != (size.width, size.height) {
            if self.fullscreen {
                self.renderer.invalidate_cursor();
            }
            self.stage_move_cursor_to(Position::ORIGIN);
            if !self.fullscreen && size.height > 0 {
                // Materialize new inline rows before insertion uses CUD,
                // which clamps at the terminal's bottom instead of scrolling.
                self.stage_move_cursor_to(Position::new(0, size.height - 1));
            }
        }
        self.stage_insert_above(content);
        self.renderer.replay_regions(&mut self.out_buf, size);
        self.stage_move_cursor_to(Position::ORIGIN);
        self.renderer
            .end_frame(&mut self.out_buf, self.cursor_visible);
        // Replay completes displayed work, not a pending desired invalidation.
        if redraw {
            self.renderer.request_clear();
        }
        self.flush()
    }

    /// Remove displayed placements before a terminal handoff.
    ///
    /// Desired content stays intact for a later render. An unrendered clear
    /// does not affect which placements this method removes.
    pub fn release_regions(&mut self) -> io::Result<()> {
        if !self.renderer.has_displayed_regions() {
            return Ok(());
        }
        let size = self.size();
        self.renderer.release_regions(&mut self.out_buf, size)?;
        self.flush()?;
        self.renderer.commit_region_release();
        Ok(())
    }

    /// Place or replace an opaque region under a stable, screen-local key.
    ///
    /// Keys are opaque strings in one namespace. Equal coordinates do not
    /// identify equal placements. An equal redeclaration retains displayed
    /// content; each call still establishes a new application drawing order.
    ///
    /// A later region placement retires every intersecting block. Later cell
    /// writes appear above the affected cells, and keep their coordinates.
    /// An equal redeclaration covers earlier text overlays again.
    ///
    /// # Errors
    ///
    /// Requires a nonempty rectangle inside the managed surface. The
    /// implementation validates its content when `render` prepares the frame.
    pub fn set_region(
        &mut self,
        key: &str,
        rect: Rect,
        region: impl Region + 'static,
    ) -> io::Result<()> {
        self.refresh_anchored_regions();
        if rect.is_empty() || rect.intersection(self.bounds()) != rect {
            return Err(invalid("region must fit inside the screen"));
        }
        crate::renderer::regions::validate_coverage(&region, rect)?;
        self.renderer.remove_anchor_declaration(key);
        self.renderer
            .set_region(key, rect, region, &mut self.front_buf);
        Ok(())
    }

    /// Remove a declaration and restore the current underlying desired text.
    ///
    /// The displayed placement remains owned until `render` removes it.
    pub fn remove_region(&mut self, key: &str) {
        self.refresh_anchored_regions();
        self.renderer.remove_anchor_declaration(key);
        self.renderer.remove_region(key, &mut self.front_buf);
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::buffer::SurfaceMut;
    use crate::text::TextSurface;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    pub(crate) fn screen() -> Screen<Vec<u8>> {
        let mut screen = Screen::new(Vec::new(), (40, 8));
        screen.set_fullscreen(true);
        screen.set_synchronized_output(true);
        screen
    }

    pub(crate) fn output(screen: &mut Screen<Vec<u8>>) -> Vec<u8> {
        screen.render().unwrap();
        std::mem::take(screen.writer_mut())
    }

    #[test]
    fn anchor_resolution_errors_preserve_output_and_pending_cells() {
        struct Follower;
        impl AnchoredRegion for Follower {
            fn resolve(&self, cells: RegionCells<'_>) -> io::Result<Option<RegionPlacement>> {
                match cells.cell(Position::ORIGIN).map(|cell| cell.content()) {
                    Some("!") => Err(invalid("invalid anchor")),
                    Some("#") => Ok(Some(RegionPlacement {
                        anchor: Position::ORIGIN,
                        rect: Rect::new(2, 1, 1, 1),
                        content: Arc::new(TestRegion(b'A', false)),
                    })),
                    _ => Ok(None),
                }
            }
        }
        let mut s = screen();
        s.set_str((2, 1), "base", Style::default());
        s.set_anchored_region("follower", Follower);
        s.set_str((0, 0), "#", Style::default());
        assert!(output(&mut s).contains(&b'A'));
        s.write_all(b"prefix").unwrap();
        s.set_str((0, 0), "!", Style::default());
        assert_eq!(s.render().unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert!(s.writer().is_empty());
        assert_eq!(s.out_buf, b"prefix");
        s.set_str((0, 0), " ", Style::default());
        let bytes = output(&mut s);
        assert!(bytes.starts_with(b"prefix"));
        assert!(bytes.contains(&b'b'));
        s.clear();
        output(&mut s);
        s.set_str((0, 0), "#", Style::default());
        assert!(!output(&mut s).contains(&b'A'));
    }

    #[derive(PartialEq)]
    pub(crate) struct TestRegion(pub(crate) u8, pub(crate) bool);

    impl Region for TestRegion {
        fn same(&self, other: &dyn Region) -> bool {
            other.downcast_ref::<Self>() == Some(self)
        }

        fn prepare(&self, size: Size, _: RegionContext) -> io::Result<PreparedRegion> {
            if self.1 {
                return Err(invalid("test preparation failure"));
            }
            Ok(PreparedRegion {
                draw: vec![RegionOp::Write(vec![self.0])],
                erase: vec![RegionOp::Erase(Rect::new(0, 0, size.width, size.height))],
            })
        }
    }

    #[test]
    fn shared_resources_precede_cells_and_follow_dependency_lifetimes() {
        let parent = Arc::new(RegionResource {
            create: b"<parent>".to_vec(),
            release: b"</parent>".to_vec(),
            ..Default::default()
        });
        let child = Arc::new(RegionResource {
            dependencies: vec![Arc::clone(&parent)],
            create: b"<child>".to_vec(),
            release: b"</child>".to_vec(),
            ..Default::default()
        });
        let mut s = screen();
        for frame in 0..3 {
            s.clear();
            s.set_region_resource("a", Arc::clone(&child));
            s.set_region_resource("b", Arc::clone(&child));
            s.set_str((2, 2), "marker", Style::default());
            let bytes = output(&mut s);
            if frame == 0 {
                let create = bytes
                    .windows(15)
                    .position(|b| b == b"<parent><child>")
                    .unwrap();
                let cells = bytes.windows(6).position(|b| b == b"marker").unwrap();
                assert!(create < cells);
            } else {
                assert!(bytes.is_empty());
            }
        }
        s.remove_region_resource("a");
        assert!(output(&mut s).is_empty());
        s.invalidate();
        let bytes = output(&mut s);
        assert!(bytes.windows(15).any(|b| b == b"<parent><child>"));
        s.insert_above("log").unwrap();
        let bytes = std::mem::take(s.writer_mut());
        let create = bytes
            .windows(15)
            .position(|b| b == b"<parent><child>")
            .unwrap();
        let cells = bytes.windows(6).rposition(|b| b == b"marker").unwrap();
        assert!(create < cells);
        s.remove_region_resource("b");
        let bytes = output(&mut s);
        assert!(bytes.windows(17).any(|b| b == b"</child></parent>"));
        assert!(s.renderer.regions.is_none());
    }

    #[test]
    fn replacement_removes_a_fully_overlaid_region() {
        let mut s = screen();
        let rect = Rect::new(2, 2, 1, 1);
        s.set_region("old", rect, TestRegion(b'A', false)).unwrap();
        output(&mut s);
        s.set_str((2, 2), "x", Style::default());
        output(&mut s);
        s.set_region("new", rect, TestRegion(b'B', false)).unwrap();
        let entries = &s.renderer.regions.as_ref().unwrap().entries;
        assert_eq!(entries.len(), 1);
        assert!(entries.contains_key("new"));
        assert!(output(&mut s).contains(&b'B'));
        s.remove_region("new");
        let bytes = output(&mut s);
        assert!(bytes.contains(&b'x'));
        assert!(!bytes.contains(&b'A'));
        assert!(s.renderer.regions.is_none());
    }

    #[test]
    fn regions_preparation_error_keeps_frame_and_prefix_uncommitted() {
        let mut s = screen();
        s.set_tracked_cursor((6, 4));
        s.write_all(b"prefix").unwrap();
        s.set_region("bad", Rect::new(2, 2, 1, 1), TestRegion(b'T', true))
            .unwrap();
        assert!(s.render().is_err());
        assert!(s.writer().is_empty());
        assert_eq!(s.out_buf, b"prefix");
        assert_eq!(s.tracked_cursor(), Some(Position::new(6, 4)));
        assert!(!s.renderer.has_displayed_regions());
        s.set_region("bad", Rect::new(2, 2, 1, 1), TestRegion(b'T', false))
            .unwrap();
        assert!(output(&mut s).starts_with(b"prefix"));
    }

    #[test]
    fn regions_prepare_once_and_reuse_operations_for_movement_replay_and_release() {
        #[derive(Clone)]
        struct Counted(Arc<AtomicUsize>);
        impl Region for Counted {
            fn same(&self, other: &dyn Region) -> bool {
                other
                    .downcast_ref::<Self>()
                    .is_some_and(|other| Arc::ptr_eq(&self.0, &other.0))
            }
            fn prepare(&self, size: Size, context: RegionContext) -> io::Result<PreparedRegion> {
                assert_eq!(size, Size::new(2, 1));
                assert_eq!(context.screen_size, Size::new(40, 8));
                self.0.fetch_add(1, Ordering::Relaxed);
                Ok(PreparedRegion {
                    draw: vec![
                        RegionOp::SetStyle(Style::default().bold()),
                        RegionOp::Write(b"X".to_vec()),
                        RegionOp::SetCursor(Position::new(1, 0)),
                    ],
                    erase: vec![RegionOp::Erase(Rect::new(0, 0, 2, 1))],
                })
            }
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let content = Counted(Arc::clone(&calls));
        let mut s = screen();
        for x in [2, 10] {
            let rect = Rect::new(x, 2, 2, 1);
            s.set_region("counted", rect, content.clone()).unwrap();
            assert!(s.writer().is_empty());
            output(&mut s);
            assert_eq!(s.tracked_cursor(), Some(Position::new(x + 1, 2)));
            s.set_str((0, 7), "ordinary edit", Style::default());
            output(&mut s);
            s.clear();
            s.set_str((0, 7), "ordinary edit", Style::default());
            s.set_region("counted", rect, content.clone()).unwrap();
            assert!(output(&mut s).is_empty());
            assert_eq!(calls.load(Ordering::Relaxed), 1);
        }
        s.insert_above("log").unwrap();
        s.release_regions().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn inline_recipes_preserve_the_anchor_and_explicit_cursor_assertions() {
        struct Recipe(Vec<RegionOp>);
        impl Region for Recipe {
            fn same(&self, other: &dyn Region) -> bool {
                other
                    .downcast_ref::<Self>()
                    .is_some_and(|other| self.0 == other.0)
            }
            fn prepare(&self, size: Size, _: RegionContext) -> io::Result<PreparedRegion> {
                Ok(PreparedRegion {
                    draw: self.0.clone(),
                    erase: vec![RegionOp::Erase(Rect::new(0, 0, size.width, size.height))],
                })
            }
        }
        for (ops, cursor, saves, restores) in [
            (
                vec![
                    RegionOp::Write(b"A".to_vec()),
                    RegionOp::Write(b"B".to_vec()),
                ],
                (5, 2),
                1,
                1,
            ),
            (
                vec![
                    RegionOp::Write(b"A".to_vec()),
                    RegionOp::SetStyle(Style::default().bold().link("https://example.com", "")),
                    RegionOp::MoveTo(Position::new(0, 1)),
                    RegionOp::Write(b"B".to_vec()),
                ],
                (5, 3),
                2,
                2,
            ),
            (
                vec![
                    RegionOp::Write(b"A".to_vec()),
                    RegionOp::SetStyle(Style::default().bold()),
                    RegionOp::SetCursor(Position::new(1, 0)),
                    RegionOp::Write(b"B".to_vec()),
                    RegionOp::SetCursor(Position::new(2, 0)),
                ],
                (7, 2),
                2,
                0,
            ),
            (vec![RegionOp::Write(Vec::new())], (5, 2), 0, 0),
        ] {
            let mut s = Screen::new(Vec::new(), (40, 8));
            s.set_region("inline", Rect::new(5, 2, 3, 2), Recipe(ops))
                .unwrap();
            let bytes = output(&mut s);
            assert_eq!(s.tracked_cursor(), Some(cursor.into()));
            assert_eq!(
                bytes.windows(2).filter(|bytes| *bytes == b"\x1b7").count(),
                saves
            );
            assert_eq!(
                bytes.windows(2).filter(|bytes| *bytes == b"\x1b8").count(),
                restores
            );
            assert!(output(&mut s).is_empty());
            s.remove_region("inline");
            output(&mut s);
            assert!(!s.renderer.has_displayed_regions());
        }
    }

    #[test]
    fn text_overlays_follow_native_content_and_survive_replay_and_removal() {
        struct Grid;
        impl Region for Grid {
            fn same(&self, other: &dyn Region) -> bool {
                other.downcast_ref::<Self>().is_some()
            }
            fn prepare(&self, _: Size, _: RegionContext) -> io::Result<PreparedRegion> {
                Ok(PreparedRegion {
                    draw: vec![
                        RegionOp::Write(b"ABCD".to_vec()),
                        RegionOp::MoveTo((0, 1).into()),
                        RegionOp::Write(b"abcd".to_vec()),
                    ],
                    erase: vec![RegionOp::Erase(Rect::new(0, 0, 4, 2))],
                })
            }
        }
        for fullscreen in [false, true] {
            let mut s = Screen::new(Vec::new(), (16, 6));
            s.set_fullscreen(fullscreen);
            for frame in 0..3 {
                s.clear();
                s.set_str((2, 2), "base", Style::default());
                s.set_str((2, 3), "next", Style::default());
                s.set_region("grid", Rect::new(2, 2, 4, 2), Grid).unwrap();
                s.set_str((3, 2), "XY", Style::default());
                let bytes = output(&mut s);
                if frame == 0 {
                    let native = bytes.windows(4).position(|bytes| bytes == b"ABCD").unwrap();
                    let text = bytes.windows(2).rposition(|bytes| bytes == b"XY").unwrap();
                    assert!(native < text);
                } else {
                    assert!(bytes.is_empty(), "{}", bytes.escape_ascii());
                }
            }
            s.set_cell((3, 2), &crate::cell::Cell::new("Z", 1));
            let bytes = output(&mut s);
            assert!(bytes.contains(&b'Z'));
            assert!(!bytes.windows(4).any(|bytes| bytes == b"ABCD"));
            s.fill_rect(Rect::new(2, 3, 2, 1), &crate::cell::Cell::new(".", 1));
            s.clear_rect(Rect::new(4, 3, 1, 1));
            output(&mut s);
            s.set_str((3, 2), "P", Style::default());
            s.insert_above("log").unwrap();
            let bytes = std::mem::take(s.writer_mut());
            assert!(bytes.windows(4).any(|bytes| bytes == b"ABCD"));
            assert!(bytes.windows(2).any(|bytes| bytes == b"ZY"));
            assert!(!bytes.contains(&b'P'));
            output(&mut s);
            assert!(output(&mut s).is_empty());
            s.remove_region("grid");
            let bytes = output(&mut s);
            assert!(bytes.windows(4).any(|bytes| bytes == b"bPYe"));
            assert!(bytes.windows(4).any(|bytes| bytes == b".. t"));
            assert!(s.renderer.regions.is_none());
            for fill in [false, true] {
                s.set_region("grid", Rect::new(2, 2, 4, 2), Grid).unwrap();
                output(&mut s);
                if fill {
                    s.fill(&crate::cell::Cell::new("x", 1));
                } else {
                    s.clear();
                }
                let bytes = output(&mut s);
                assert!(!bytes.windows(4).any(|bytes| bytes == b"ABCD"));
                assert!(s.renderer.regions.is_none());
            }
            s.clear();
            s.set_region("grid", Rect::new(2, 2, 4, 2), Grid).unwrap();
            s.set_str((2, 2), "X", Style::default());
            s.set_str((4, 2), "Y", Style::default());
            let bytes = output(&mut s);
            assert!(!bytes.windows(2).any(|bytes| bytes == b"X "));
            assert!(bytes.contains(&b'X') && bytes.contains(&b'Y'));
        }
    }
}
