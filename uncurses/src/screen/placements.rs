use std::io::{self, Write};
use std::sync::Arc;

use super::{PlacementResource, Screen, TextSizing};
use crate::ansi::text_sizing::write_sized_text;
use crate::buffer::Buffer;
use crate::layout::{Position, Rect};
use crate::style::Style;

/// A nonzero, caller-selected placement identifier within one screen.
pub type PlacementId = u64;

/// Resolved geometry for one visible fragment of a placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlacementContext {
    /// The complete placement rectangle in screen coordinates.
    pub area: Rect,
    /// The visible fragment, relative to the complete rectangle's origin.
    pub clip: Rect,
    /// Screen-owned identity of this displayed instance.
    ///
    /// Movement preserves the identity. Removal or commitment of the complete
    /// placement ends it; registration under the same application ID creates
    /// a new instance.
    pub instance: u64,
}

/// Commands for one visible placement fragment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlacementOutput {
    /// Replay-safe draw commands within the fragment.
    pub draw: Vec<u8>,
    /// Commands that erase this fragment at its previous origin.
    pub erase: Vec<u8>,
}

/// Prepare bounded output after the screen resolves cell ownership.
///
/// The screen prepares each visible rectangular fragment when its content
/// or geometry changes. It replays prepared commands for surrounding damage
/// and completes all preparation before any frame output.
/// Preparation must perform no terminal I/O and must preserve caller settings.
///
/// The renderer positions each payload at the fragment's origin. Commands
/// must stay inside the fragment, preserve terminal modes and the saved
/// cursor, and avoid scrolling. Encode resource setup in
/// [`PlacementResource`] instead of the draw payload.
///
/// Keep prepared content immutable. Reuse its `Arc` across frames and copies
/// so an unchanged placement remains an unchanged frame.
/// The screen reuses output while the instance, area, options, and visible
/// rectangles match the displayed placement.
pub trait PlacementEncoder: std::fmt::Debug + Send + Sync {
    /// Prepare a fragment, or return an error before terminal output.
    fn prepare(&self, context: PlacementContext) -> io::Result<PlacementOutput>;
}

/// Encoded placement output and overwrite behavior.
///
/// The screen copies these options. Reuse them when moving a placement.
/// Payloads must contain complete ANSI sequences, preserve terminal modes
/// and the saved cursor, and stay within the declared rectangle without
/// scrolling. The renderer positions the cursor at the rectangle's origin,
/// then restores its cursor and pen after each payload.
///
/// The screen emits shared uploads before frame output and deletes resources
/// after their final placement removal succeeds. Cloned resource handles
/// identify the same shared resource.
/// Resources used by [`Screen::commit_rows`] stay in the terminal for committed output.
#[derive(Clone, Debug, Default)]
pub struct PlacementOptions {
    /// Bytes emitted before ordinary text that covers the placement.
    pub draw: Vec<u8>,
    /// Bytes that remove this placement, emitted at its previous origin.
    ///
    /// An empty payload uses ordinary cell erasure. For shared images, encode
    /// placement removal here and resource deletion in [`PlacementResource`].
    pub erase: Vec<u8>,
    /// Remove the whole placement when ordinary text overwrites any part.
    ///
    /// Use this for indivisible sized text. Static payloads otherwise remain
    /// beneath text; encoders prepare their remaining visible cells.
    /// Complete coverage removes the placement; register it again to restore it.
    pub erase_on_overwrite: bool,
    /// Ordinary cells occupying the rectangle, instead of an opaque draw payload.
    ///
    /// Dimensions must match the rectangle. Use empty draw and erase payloads,
    /// leave `encoder` unset, and leave `erase_on_overwrite` false.
    /// Private placement markers are invalid.
    /// Removal replaces matching cells with default-style blanks and preserves
    /// changed overlay cells.
    pub cells: Option<Arc<Buffer>>,
    /// Prepare opaque fragments from the final visible cell regions.
    ///
    /// Use empty `draw` and `erase` payloads and leave `cells` unset.
    /// Later cell writes exclude those cells from the prepared fragments.
    pub encoder: Option<Arc<dyn PlacementEncoder>>,
    /// Shared resource required by this placement.
    ///
    /// Clones share uploads and cleanup. The screen restores managed uploads
    /// after invalidation or output recovery.
    pub resource: Option<PlacementResource>,
}

impl PartialEq for PlacementOptions {
    fn eq(&self, other: &Self) -> bool {
        self.draw == other.draw
            && self.erase == other.erase
            && self.erase_on_overwrite == other.erase_on_overwrite
            && self.resource == other.resource
            && match (&self.encoder, &other.encoder) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
            && match (&self.cells, &other.cells) {
                (None, None) => true,
                (Some(a), Some(b)) => {
                    Arc::ptr_eq(a, b)
                        || (a.width() == b.width()
                            && a.height() == b.height()
                            && (0..a.height()).all(|y| a.line(y) == b.line(y)))
                }
                _ => false,
            }
    }
}

impl Eq for PlacementOptions {}

impl<W: Write> Screen<W> {
    /// Stage an ANSI placement for the next [`render`](Self::render).
    ///
    /// The ID must be nonzero and the nonempty rectangle must fit the screen.
    /// One ID describes one rectangle. Use another ID for another instance.
    /// Calls establish emission order. Later cell writes replace ownership
    /// of the cells they cover. Encoders prepare only owned cells; static
    /// payloads follow their protocol's visual layer rules.
    /// Repaint the background when moving or removing content.
    /// Resize removes placements whose complete rectangle no longer fits.
    ///
    /// Opaque placement cells retain the existing cell styles. Erasure replaces
    /// them with blanks of those styles and preserves ordinary overlay cells.
    /// Set the area to styled blanks before registration to choose its erase
    /// style. Encode the payload's own style in its draw bytes.
    ///
    /// Placement cells belong to this screen and their declared coordinates.
    /// Register a separate placement instead of translating copied markers.
    /// Invalid geometry and IDs fail before changing the desired frame.
    ///
    /// ```
    /// use uncurses::{layout::Rect, screen::{PlacementOptions, Screen}};
    ///
    /// let mut screen = Screen::new(Vec::new(), (20, 4));
    /// let options = PlacementOptions {
    ///     draw: b"hello".to_vec(),
    ///     ..Default::default()
    /// };
    /// screen.set_placement(1, Rect::new(2, 1, 5, 1), &options)?;
    /// screen.render()?;
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn set_placement(
        &mut self,
        id: PlacementId,
        area: Rect,
        options: &PlacementOptions,
    ) -> io::Result<()> {
        self.renderer
            .set_placement(&mut self.front_buf, id, area, options)
    }

    /// Stage styled Kitty OSC 66 text for the next [`render`](Self::render).
    ///
    /// `id` uses the same namespace as [`set_placement`](Self::set_placement).
    /// The block starts at `position` and occupies [`TextSizing::size`].
    /// Its explicit width must be nonzero, and the complete block must fit.
    /// Text follows the requirements of [`write_sized_text`].
    /// Invalid input leaves the desired frame unchanged.
    ///
    /// An ordinary cell overwrite removes the complete block. Remaining cells
    /// become blanks with `style`; overlay cells keep their own styles.
    /// Use [`remove_placement`](Self::remove_placement) for explicit removal.
    ///
    /// The draw bytes use the current color profile. Register the text again
    /// after a profile change. The terminal must support Kitty text sizing.
    ///
    /// ```
    /// use uncurses::{
    ///     screen::{Screen, TextSizing},
    ///     style::Style,
    /// };
    ///
    /// let mut screen = Screen::new(Vec::new(), (20, 6));
    /// let sizing = TextSizing { scale: 2, width: 4, ..Default::default() };
    /// screen.set_sized_text(1, (2, 1), "Demo", sizing, Style::new().bold())?;
    /// screen.render()?;
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn set_sized_text(
        &mut self,
        id: PlacementId,
        position: impl Into<Position>,
        text: &str,
        sizing: TextSizing,
        style: impl Into<Style>,
    ) -> io::Result<()> {
        let size = sizing.size()?;
        let position = position.into();
        let area = Rect::new(position.x, position.y, size.width, size.height);
        let style = style.into();
        let mut draw = crate::style::diff::convert_style(&style, self.color_profile())
            .to_string()
            .into_bytes();
        write_sized_text(&mut draw, text, sizing)?;
        self.set_placement(
            id,
            area,
            &PlacementOptions {
                draw,
                erase_on_overwrite: true,
                ..Default::default()
            },
        )?;
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                self.front_buf.cell_mut((x, y)).unwrap().style = style.clone();
            }
        }
        Ok(())
    }

    /// Stage removal of a placement while preserving text above it.
    ///
    /// The next render emits its erase bytes and repaints the damaged cells.
    /// Opaque placement cells become blanks with their existing styles.
    /// Cell-backed placements use default-style blanks for matching cells.
    /// Removing an absent ID is a no-op.
    pub fn remove_placement(&mut self, id: PlacementId) {
        self.renderer.remove_placement(&mut self.front_buf, id);
    }

    pub(crate) fn release_placements(&mut self) -> io::Result<()> {
        if !self.renderer.placements.has_output() {
            return Ok(());
        }
        let result = self.render_frame(true);
        self.front_buf.touch_all();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Surface;
    use crate::cell::Cell;
    use crate::text::TextSurface;

    fn options() -> PlacementOptions {
        PlacementOptions {
            draw: b"<DRAW>".to_vec(),
            erase: b"<ERASE>".to_vec(),
            ..Default::default()
        }
    }

    fn cell_options() -> PlacementOptions {
        let mut cells = Buffer::new(2, 1);
        cells.set((0, 0), &Cell::new("A", 1));
        cells.set((1, 0), &Cell::new("B", 1));
        PlacementOptions {
            cells: Some(Arc::new(cells)),
            resource: Some(PlacementResource::new(
                b"<UPLOAD>".as_slice(),
                b"<DELETE>".as_slice(),
            )),
            ..Default::default()
        }
    }

    #[test]
    fn committed_resources_release_their_upload_buffers() {
        let mut screen = Screen::new(Vec::new(), (10, 4));
        let upload: Arc<[u8]> = Arc::from(b"<UPLOAD>".as_slice());
        let retained = Arc::downgrade(&upload);
        let options = PlacementOptions {
            draw: b"AB".to_vec(),
            resource: Some(PlacementResource::new(upload, b"<DELETE>".as_slice())),
            ..Default::default()
        };
        screen
            .set_placement(1, Rect::new(0, 0, 2, 1), &options)
            .unwrap();
        screen.commit_rows(1).unwrap();
        drop(options);
        assert!(retained.upgrade().is_none());
        screen.writer_mut().clear();
        screen.release_placements().unwrap();
        assert!(screen.writer().is_empty());
    }

    #[test]
    fn three_overlapping_static_placements_follow_registration_order() {
        let mut screen = Screen::new(Vec::new(), (10, 4));
        for (id, x, text) in [(1, 1, b"AAA"), (2, 2, b"BBB"), (3, 3, b"CCC")] {
            screen
                .set_placement(
                    id,
                    Rect::new(x, 1, 3, 1),
                    &PlacementOptions {
                        draw: text.to_vec(),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert!(output.find("AAA").unwrap() < output.find("BBB").unwrap());
        assert!(output.find("BBB").unwrap() < output.find("CCC").unwrap());
        screen.writer_mut().clear();
        screen.render().unwrap();
        assert!(screen.writer().is_empty());
    }

    #[test]
    fn three_cell_placements_commit_only_the_visible_resources() {
        use crate::buffer::SurfaceMut;

        let mut screen = Screen::new(Vec::new(), (10, 4));
        let first = cell_options();
        let mut lower_cells = Buffer::new(2, 2);
        lower_cells.fill(&Cell::new("X", 1));
        let lower = PlacementOptions {
            cells: Some(Arc::new(lower_cells)),
            resource: Some(PlacementResource::new(
                b"<LOWER UPLOAD>".as_slice(),
                b"<LOWER DELETE>".as_slice(),
            )),
            ..Default::default()
        };
        let mut upper_cells = Buffer::new(2, 1);
        upper_cells.fill(&Cell::new("X", 1));
        let upper = PlacementOptions {
            cells: Some(Arc::new(upper_cells)),
            resource: Some(PlacementResource::new(
                b"<UPPER UPLOAD>".as_slice(),
                b"<UPPER DELETE>".as_slice(),
            )),
            ..Default::default()
        };
        screen
            .set_placement(1, Rect::new(0, 0, 2, 1), &first)
            .unwrap();
        screen
            .set_placement(2, Rect::new(3, 0, 2, 2), &lower)
            .unwrap();
        screen
            .set_placement(3, Rect::new(3, 0, 2, 1), &upper)
            .unwrap();
        screen.commit_rows(1).unwrap();
        screen.writer_mut().clear();
        screen.remove_placement(2);
        screen.render().unwrap();
        assert!(screen.writer().ends_with(b"<LOWER DELETE>"));
        assert!(!String::from_utf8_lossy(screen.writer()).contains("<UPPER DELETE>"));

        screen
            .set_placement(3, Rect::new(3, 0, 2, 1), &upper)
            .unwrap();
        screen.render().unwrap();
        screen.writer_mut().clear();
        screen.remove_placement(3);
        screen.render().unwrap();
        assert!(!String::from_utf8_lossy(screen.writer()).contains("<UPPER DELETE>"));
    }

    #[test]
    fn removal_and_movement_preserve_later_identical_cell_placements() {
        use crate::buffer::SurfaceMut;

        for render_first in [false, true] {
            for move_first in [false, true] {
                let mut screen = Screen::new(Vec::new(), (10, 4));
                let mut cells = Buffer::new(4, 1);
                cells.fill(&Cell::new("A", 1));
                let options = PlacementOptions {
                    cells: Some(Arc::new(cells)),
                    ..cell_options()
                };
                screen
                    .set_placement(1, Rect::new(1, 1, 4, 1), &options)
                    .unwrap();
                screen
                    .set_placement(2, Rect::new(3, 1, 4, 1), &options)
                    .unwrap();
                if render_first {
                    screen.render().unwrap();
                }
                if move_first {
                    screen
                        .set_placement(1, Rect::new(0, 0, 4, 1), &options)
                        .unwrap();
                } else {
                    screen.remove_placement(1);
                }
                for x in 1..3 {
                    assert!(screen.cell((x, 1).into()).unwrap().is_blank());
                }
                for x in 3..7 {
                    assert_eq!(screen.cell((x, 1).into()).unwrap().content(), "A");
                }
                screen.render().unwrap();
                screen.writer_mut().clear();
                screen.render().unwrap();
                assert!(screen.writer().is_empty());
            }
        }
    }

    #[derive(Debug, Default)]
    struct EncodedText(std::sync::Mutex<Vec<PlacementContext>>);

    impl PlacementEncoder for EncodedText {
        fn prepare(&self, context: PlacementContext) -> io::Result<PlacementOutput> {
            self.0.lock().unwrap().push(context);
            Ok(PlacementOutput {
                draw: vec![b'X'; usize::from(context.clip.width)],
                ..Default::default()
            })
        }
    }

    #[test]
    fn encoders_receive_visible_regions_and_fresh_instances_after_commit() {
        let mut screen = Screen::new(Vec::new(), (10, 6));
        let encoder = Arc::new(EncodedText::default());
        let options = PlacementOptions {
            encoder: Some(encoder.clone()),
            ..Default::default()
        };
        let area = Rect::new(1, 1, 6, 3);
        screen.set_placement(1, area, &options).unwrap();
        screen.set_str((3, 2), "T", Style::EMPTY);
        assert!(encoder.0.lock().unwrap().is_empty());
        screen.render().unwrap();
        let first = encoder.0.lock().unwrap().clone();
        assert_eq!(
            first.iter().map(|context| context.clip).collect::<Vec<_>>(),
            [
                Rect::new(0, 0, 6, 1),
                Rect::new(0, 1, 2, 1),
                Rect::new(3, 1, 3, 1),
                Rect::new(0, 2, 6, 1),
            ]
        );
        assert!(first.iter().all(|context| context.area == area));
        screen.writer_mut().clear();
        screen.set_placement(1, area, &options).unwrap();
        screen.set_str((3, 2), "T", Style::EMPTY);
        screen.render().unwrap();
        assert!(screen.writer().is_empty());
        assert_eq!(encoder.0.lock().unwrap().len(), first.len());
        screen.set_str((9, 1), "Z", Style::EMPTY);
        screen.render().unwrap();
        assert_eq!(encoder.0.lock().unwrap().len(), first.len());

        screen
            .set_placement(1, Rect::new(2, 1, 6, 3), &options)
            .unwrap();
        screen.render().unwrap();
        assert_eq!(
            encoder.0.lock().unwrap().last().unwrap().instance,
            first[0].instance
        );
        screen.commit_rows(4).unwrap();
        screen.resize_height(6);
        screen.set_placement(1, area, &options).unwrap();
        screen.render().unwrap();
        assert_ne!(
            encoder.0.lock().unwrap().last().unwrap().instance,
            first[0].instance
        );
    }

    #[test]
    fn encoder_errors_leave_output_untouched_and_allow_replacement() {
        #[derive(Debug)]
        struct Invalid;
        impl PlacementEncoder for Invalid {
            fn prepare(&self, _: PlacementContext) -> io::Result<PlacementOutput> {
                Err(io::Error::other("cannot encode this fragment"))
            }
        }
        let mut screen = Screen::new(Vec::new(), (10, 4));
        let mut options = PlacementOptions {
            encoder: Some(Arc::new(Invalid)),
            resource: cell_options().resource,
            ..Default::default()
        };
        let area = Rect::new(1, 1, 2, 1);
        screen.set_placement(1, area, &options).unwrap();
        for _ in 0..2 {
            assert!(screen.render().is_err());
            assert!(screen.writer().is_empty());
        }
        options.encoder = Some(Arc::new(EncodedText::default()));
        screen.set_placement(1, area, &options).unwrap();
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert!(output.find("<UPLOAD>").unwrap() < output.find("XX").unwrap());
    }

    #[test]
    fn encoders_reprepare_rebased_geometry_on_the_next_repaint() {
        let mut screen = Screen::new(Vec::new(), (10, 6));
        let encoder = Arc::new(EncodedText::default());
        screen
            .set_placement(
                1,
                Rect::new(1, 2, 2, 1),
                &PlacementOptions {
                    encoder: Some(encoder.clone()),
                    ..Default::default()
                },
            )
            .unwrap();
        screen.render().unwrap();
        screen.commit_rows(1).unwrap();
        screen.writer_mut().clear();
        screen.render().unwrap();
        assert!(screen.writer().is_empty());
        assert_eq!(encoder.0.lock().unwrap().len(), 1);
        screen.invalidate();
        screen.render().unwrap();
        let prepared = encoder.0.lock().unwrap();
        assert_eq!(prepared.len(), 2);
        assert_eq!(prepared[1].area, Rect::new(1, 1, 2, 1));
        assert_eq!(prepared[1].instance, prepared[0].instance);
    }

    #[test]
    fn cell_placements_share_resources_until_the_last_visible_copy_is_gone() {
        let mut screen = Screen::new(Vec::new(), (10, 4));
        let options = cell_options();
        screen
            .set_placement(1, Rect::new(1, 1, 2, 1), &options)
            .unwrap();
        screen
            .set_placement(2, Rect::new(5, 1, 2, 1), &options)
            .unwrap();
        assert!(screen.writer().is_empty());
        screen.render().unwrap();
        assert_eq!(
            String::from_utf8_lossy(screen.writer())
                .matches("<UPLOAD>")
                .count(),
            1
        );
        screen.writer_mut().clear();
        screen.remove_placement(1);
        screen.render().unwrap();
        assert!(!String::from_utf8_lossy(screen.writer()).contains("<DELETE>"));
        assert_eq!(screen.cell((5, 1).into()).unwrap().content(), "A");
        screen.writer_mut().clear();
        screen.set_str((5, 1), "XY", Style::new().bold());
        screen.render().unwrap();
        assert!(screen.writer().ends_with(b"<DELETE>"));
        assert_eq!(screen.cell((5, 1).into()).unwrap().content(), "X");
        screen.writer_mut().clear();
        screen.render().unwrap();
        assert!(screen.writer().is_empty());
    }

    #[test]
    fn wide_cell_placements_retire_after_the_last_primary_is_overwritten() {
        let mut screen = Screen::new(Vec::new(), (10, 4));
        let mut cells = Buffer::new(4, 1);
        cells.set((0, 0), &Cell::new("界", 2));
        cells.set((2, 0), &Cell::new("界", 2));
        let options = PlacementOptions {
            cells: Some(Arc::new(cells)),
            resource: Some(PlacementResource::new(
                b"<UPLOAD>".as_slice(),
                b"<DELETE>".as_slice(),
            )),
            ..Default::default()
        };
        screen
            .set_placement(1, Rect::new(1, 1, 4, 1), &options)
            .unwrap();
        screen.render().unwrap();
        screen.writer_mut().clear();

        screen.set_cell((1, 1), &Cell::new("語", 2));
        screen.render().unwrap();
        assert!(!screen.writer().ends_with(b"<DELETE>"));
        screen.writer_mut().clear();

        screen.set_cell((3, 1), &Cell::new("語", 2));
        screen.render().unwrap();
        assert!(screen.writer().ends_with(b"<DELETE>"));
        for x in [1, 3] {
            assert_eq!(screen.cell((x, 1).into()).unwrap().content(), "語");
            assert!(screen.cell((x + 1, 1).into()).unwrap().is_continuation());
        }
        screen.writer_mut().clear();
        screen.render().unwrap();
        assert!(screen.writer().is_empty());
    }

    #[test]
    fn committed_placements_keep_shared_resources_and_rebase_retained_rows() {
        for cell_backed in [false, true] {
            let mut screen = Screen::new(Vec::new(), (10, 4));
            let mut options = cell_options();
            if !cell_backed {
                options.cells = None;
                options.draw = b"AB".to_vec();
            }
            screen
                .set_placement(1, Rect::new(1, 0, 2, 1), &options)
                .unwrap();
            screen
                .set_placement(2, Rect::new(1, 2, 2, 1), &options)
                .unwrap();
            screen.render().unwrap();
            screen.writer_mut().clear();
            screen.commit_rows(1).unwrap();
            screen.writer_mut().clear();
            screen.render().unwrap();
            assert!(screen.writer().is_empty());

            screen.invalidate();
            screen.render().unwrap();
            assert!(!String::from_utf8_lossy(screen.writer()).contains("<UPLOAD>"));
            screen.writer_mut().clear();
            screen.remove_placement(2);
            screen.render().unwrap();
            assert!(!String::from_utf8_lossy(screen.writer()).contains("<DELETE>"));
            screen.writer_mut().clear();
            screen.release_placements().unwrap();
            assert!(screen.writer().is_empty());
        }
    }

    #[test]
    fn height_shrink_releases_placements_outside_the_retained_rows() {
        for cell_backed in [false, true] {
            let mut screen = Screen::new(Vec::new(), (10, 4));
            let mut options = cell_options();
            if !cell_backed {
                options.cells = None;
                options.draw = b"AB".to_vec();
            }
            screen
                .set_placement(1, Rect::new(1, 2, 2, 1), &options)
                .unwrap();
            screen.render().unwrap();
            screen.writer_mut().clear();
            screen.resize_height(2);
            screen.render().unwrap();
            assert!(screen.writer().ends_with(b"<DELETE>"));
        }
    }

    #[test]
    fn overwritten_committed_cells_do_not_pin_the_remaining_resource() {
        let mut screen = Screen::new(Vec::new(), (10, 4));
        let options = cell_options();
        screen
            .set_placement(1, Rect::new(1, 0, 2, 1), &options)
            .unwrap();
        screen
            .set_placement(2, Rect::new(1, 2, 2, 1), &options)
            .unwrap();
        screen.render().unwrap();
        screen.set_str((1, 0), "XY", Style::EMPTY);
        screen.commit_rows(1).unwrap();
        screen.writer_mut().clear();
        screen.remove_placement(2);
        screen.render().unwrap();
        assert!(screen.writer().ends_with(b"<DELETE>"));
    }

    #[test]
    fn committed_resources_survive_full_release_and_screen_switches() {
        let mut screen = Screen::new(Vec::new(), (10, 2));
        let options = cell_options();
        screen
            .set_placement(1, Rect::new(1, 0, 2, 1), &options)
            .unwrap();
        screen.commit_rows(2).unwrap();
        screen.writer_mut().clear();
        screen.remove_placement(1);
        screen.render().unwrap();
        screen.release_placements().unwrap();
        assert!(screen.writer().is_empty());

        screen.resize_height(2);
        screen.set_fullscreen(true);
        screen
            .set_placement(2, Rect::new(1, 0, 2, 1), &options)
            .unwrap();
        screen.render().unwrap();
        assert!(screen.writer().starts_with(b"<UPLOAD>"));
        screen.writer_mut().clear();
        screen.release_placements().unwrap();
        assert!(screen.writer().ends_with(b"<DELETE>"));

        screen.set_fullscreen(false);
        screen.writer_mut().clear();
        screen.render().unwrap();
        assert!(!String::from_utf8_lossy(screen.writer()).contains("<UPLOAD>"));
        screen.writer_mut().clear();
        screen.remove_placement(2);
        screen.render().unwrap();
        assert!(!String::from_utf8_lossy(screen.writer()).contains("<DELETE>"));
    }

    #[test]
    fn commit_rows_splits_cell_placements_without_deleting_committed_resources() {
        let mut screen = Screen::new(Vec::new(), (10, 4));
        let mut cells = Buffer::new(2, 3);
        for (y, text) in ["A", "B", "C"].into_iter().enumerate() {
            cells.set((0, y as u16), &Cell::new(text, 1));
            cells.set((1, y as u16), &Cell::new(text, 1));
        }
        let options = PlacementOptions {
            cells: Some(Arc::new(cells)),
            ..cell_options()
        };
        screen
            .set_placement(1, Rect::new(1, 0, 2, 3), &options)
            .unwrap();
        screen.commit_rows(1).unwrap();
        assert_eq!(screen.cell((1, 0).into()).unwrap().content(), "B");
        assert_eq!(screen.cell((1, 1).into()).unwrap().content(), "C");
        screen.writer_mut().clear();
        screen.render().unwrap();
        assert!(screen.writer().is_empty());
        screen.remove_placement(1);
        screen.render().unwrap();
        for y in 0..2 {
            assert!(screen.cell((1, y).into()).unwrap().is_blank());
            assert!(screen.cell((2, y).into()).unwrap().is_blank());
        }
        assert!(!String::from_utf8_lossy(screen.writer()).contains("<DELETE>"));
    }

    #[test]
    fn commit_rows_rejects_split_opaque_placements_before_output() {
        let mut screen = Screen::new(Vec::new(), (10, 4));
        screen
            .set_placement(1, Rect::new(1, 0, 2, 2), &options())
            .unwrap();
        let original = screen.cell((1, 0).into()).unwrap().clone();
        screen.write_all(b"pending").unwrap();
        let error = screen.commit_rows(1).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(screen.size(), (10, 4).into());
        assert_eq!(screen.cell((1, 0).into()), Some(&original));
        assert!(screen.writer().is_empty());
        screen.commit_rows(2).unwrap();
        assert!(screen.writer().starts_with(b"pending"));
        screen.writer_mut().clear();
        screen.render().unwrap();
        assert!(screen.writer().is_empty());
    }

    #[test]
    fn handoff_and_invalidation_restore_managed_uploads() {
        for cells in [false, true] {
            let mut screen = Screen::new(Vec::new(), (10, 4));
            let mut options = cell_options();
            if !cells {
                options.cells = None;
                options.draw = b"AB".to_vec();
            }
            screen
                .set_placement(1, Rect::new(1, 1, 2, 1), &options)
                .unwrap();
            screen.render().unwrap();
            let desired = screen.cell((1, 1).into()).unwrap().clone();
            screen.writer_mut().clear();
            screen.release_placements().unwrap();
            assert!(screen.writer().ends_with(b"<DELETE>"));
            assert_eq!(screen.cell((1, 1).into()), Some(&desired));
            screen.writer_mut().clear();
            screen.render().unwrap();
            assert!(screen.writer().starts_with(b"<UPLOAD>"));
            screen.writer_mut().clear();
            screen.invalidate();
            screen.render().unwrap();
            assert!(screen.writer().starts_with(b"<UPLOAD>"));
            screen.writer_mut().clear();
            screen.resize((2, 2));
            screen.render().unwrap();
            assert!(!String::from_utf8_lossy(screen.writer()).contains("<UPLOAD>"));
            assert!(screen.writer().ends_with(b"<DELETE>"));
        }
    }

    #[test]
    fn invalid_cell_placements_leave_existing_content_unchanged() {
        let mut screen = Screen::new(Vec::new(), (10, 4));
        screen
            .set_placement(1, Rect::new(1, 1, 2, 1), &cell_options())
            .unwrap();
        for kind in ["dimensions", "marker", "draw", "erase", "atomic"] {
            let mut options = cell_options();
            match kind {
                "dimensions" => options.cells = Some(Arc::new(Buffer::new(1, 1))),
                "marker" => {
                    Arc::make_mut(options.cells.as_mut().unwrap()).set((0, 0), &Cell::placement(99))
                }
                "draw" => options.draw = b"AB".to_vec(),
                "erase" => options.erase = b"  ".to_vec(),
                "atomic" => options.erase_on_overwrite = true,
                _ => unreachable!(),
            }
            assert!(
                screen
                    .set_placement(1, Rect::new(5, 1, 2, 1), &options)
                    .is_err()
            );
            assert_eq!(screen.cell((1, 1).into()).unwrap().content(), "A");
            assert!(screen.cell((5, 1).into()).unwrap().is_blank());
            assert!(screen.writer().is_empty());
        }
    }

    #[test]
    fn sized_text_is_staged_and_erased_as_styled_blanks() {
        use crate::color::Color;

        let mut screen = Screen::new(Vec::new(), (20, 6));
        let sizing = TextSizing {
            scale: 2,
            width: 4,
            ..Default::default()
        };
        let style = Style::new().bold().fg(Color::White).bg(Color::Black);
        let original_style = style.clone();
        screen
            .set_sized_text(1, (2, 1), "Demo", sizing, &style)
            .unwrap();
        for y in 1..3 {
            for x in 2..10 {
                let cell = screen.cell((x, y).into()).unwrap();
                assert!(cell.is_placement());
                assert_eq!(cell.style, style);
            }
        }
        screen.flush().unwrap();
        assert!(screen.writer().is_empty());
        screen.render().unwrap();
        assert!(
            String::from_utf8_lossy(screen.writer())
                .contains("\x1b]66;s=2:w=4:n=0:d=0:v=0:h=0;Demo\x1b\\")
        );
        screen.writer_mut().clear();
        screen
            .set_sized_text(1, (2, 1), "Demo", sizing, &style)
            .unwrap();
        screen.render().unwrap();
        assert!(screen.writer().is_empty());

        let overlay = Style::new().bg(Color::Blue);
        screen.set_str((3, 2), "X", &overlay);
        screen.render().unwrap();
        assert!(!String::from_utf8_lossy(screen.writer()).contains("\x1b]66;"));
        for y in 1..3 {
            for x in 2..10 {
                let cell = screen.cell((x, y).into()).unwrap();
                if (x, y) == (3, 2) {
                    assert_eq!(cell.content(), "X");
                    assert_eq!(cell.style, overlay);
                } else {
                    assert!(cell.is_blank());
                    assert_eq!(cell.style, style);
                }
            }
        }
        assert_eq!(style, original_style);
        assert_eq!(sizing.size().unwrap(), (8, 2).into());
    }

    #[test]
    fn invalid_sized_text_preserves_the_existing_frame() {
        let mut screen = Screen::new(Vec::new(), (20, 6));
        let sizing = TextSizing {
            scale: 2,
            width: 2,
            ..Default::default()
        };
        screen
            .set_sized_text(1, (2, 1), "Hi", sizing, Style::new().bold())
            .unwrap();
        screen.render().unwrap();
        screen.writer_mut().clear();
        let before: Vec<_> = (0..6)
            .map(|y| screen.front_buf.line(y).unwrap().to_vec())
            .collect();
        let oversized = "x".repeat(4097);
        for (id, position, text, sizing) in [
            (0, (2, 1), "Hi", sizing),
            (1, (19, 5), "Hi", sizing),
            (1, (u16::MAX, u16::MAX), "Hi", sizing),
            (1, (2, 1), "\x1b", sizing),
            (1, (2, 1), "\n", sizing),
            (1, (2, 1), oversized.as_str(), sizing),
            (1, (2, 1), "Hi", TextSizing { width: 0, ..sizing }),
            (1, (2, 1), "Hi", TextSizing { width: 8, ..sizing }),
            (1, (2, 1), "Hi", TextSizing { scale: 0, ..sizing }),
            (1, (2, 1), "Hi", TextSizing { scale: 8, ..sizing }),
            (
                1,
                (2, 1),
                "Hi",
                TextSizing {
                    numerator: 2,
                    denominator: 1,
                    ..sizing
                },
            ),
        ] {
            assert_eq!(
                screen
                    .set_sized_text(id, position, text, sizing, Style::EMPTY)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
            for (y, line) in before.iter().enumerate() {
                assert_eq!(screen.front_buf.line(y as u16).unwrap(), line);
            }
            screen.render().unwrap();
            assert!(screen.writer().is_empty());
        }
    }

    #[test]
    fn sized_text_uses_the_current_color_profile_and_keeps_the_original_style() {
        use crate::color::{Color, Profile};

        let style = Style::new()
            .bold()
            .fg(Color::Rgb(255, 0, 0))
            .bg(Color::Rgb(0, 0, 255))
            .link("https://example.com", "");
        for profile in [
            Profile::Disabled,
            Profile::Ascii,
            Profile::Ansi,
            Profile::Ansi256,
            Profile::TrueColor,
        ] {
            let mut screen = Screen::new(Vec::new(), (8, 2));
            screen.set_color_profile(profile);
            screen
                .set_sized_text(1, (1, 0), "", TextSizing::default(), &style)
                .unwrap();
            assert_eq!(screen.cell((1, 0).into()).unwrap().style, style);
            screen.render().unwrap();
            let converted = crate::style::diff::convert_style(&style, profile);
            let expected = format!("{converted}\x1b]66;s=1:w=1:n=0:d=0:v=0:h=0; \x1b\\");
            assert!(String::from_utf8_lossy(screen.writer()).contains(&expected));
        }
    }

    #[test]
    fn sized_text_moves_and_removes_with_placement_ids() {
        let mut screen = Screen::new(Vec::new(), (10, 3));
        let sizing = TextSizing::default();
        let style = Style::new().bold();
        screen
            .set_sized_text(1, (1, 1), "A", sizing, &style)
            .unwrap();
        screen.render().unwrap();
        screen
            .set_sized_text(1, (5, 1), "B", sizing, &style)
            .unwrap();
        assert!(screen.cell((1, 1).into()).unwrap().is_blank());
        assert_eq!(screen.cell((1, 1).into()).unwrap().style, style);
        screen.writer_mut().clear();
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert!(output.contains(";B\x1b\\"));
        assert!(!output.contains(";A\x1b\\"));
        screen.remove_placement(1);
        screen.writer_mut().clear();
        screen.render().unwrap();
        let blank = screen.cell((5, 1).into()).unwrap();
        assert!(blank.is_blank());
        assert_eq!(blank.style, style);
        assert!(!String::from_utf8_lossy(screen.writer()).contains("\x1b]66;"));
    }

    #[test]
    fn overlay_replays_after_draw_and_unchanged_frame_is_empty() {
        let mut screen = Screen::new(Vec::new(), (20, 5));
        screen.set_fullscreen(true);
        let options = options();
        screen
            .set_placement(1, Rect::new(1, 1, 8, 2), &options)
            .unwrap();
        screen.set_str((1, 1), "TOP", Style::EMPTY);
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert!(output.rfind("TOP").unwrap() > output.find("<DRAW>").unwrap());
        assert!(!output.contains("\0p"));
        assert_eq!(options, options.clone());
        screen.writer_mut().clear();
        screen.render().unwrap();
        assert!(screen.writer().is_empty());
        screen
            .set_placement(1, Rect::new(2, 1, 8, 2), &options)
            .unwrap();
        screen.set_str((2, 1), "TOP", Style::EMPTY);
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert!(output.find("<ERASE>").unwrap() < output.find("<DRAW>").unwrap());
        assert!(output.rfind("TOP").unwrap() > output.find("<DRAW>").unwrap());
    }

    #[test]
    fn text_outside_placements_is_not_replayed_over_them() {
        for fullscreen in [false, true] {
            let mut screen = Screen::new(Vec::new(), (16, 2));
            screen.set_fullscreen(fullscreen);
            for (id, x, draw) in [(1, 4, b"P"), (2, 12, b"Q")] {
                screen
                    .set_placement(
                        id,
                        Rect::new(x, 0, 1, 1),
                        &PlacementOptions {
                            draw: draw.to_vec(),
                            ..Default::default()
                        },
                    )
                    .unwrap();
            }
            screen.set_str((0, 0), "❤️", Style::EMPTY);
            screen.set_str((7, 0), "❤️", Style::EMPTY);
            screen.render().unwrap();
            let output = String::from_utf8_lossy(screen.writer());
            assert_eq!(output.matches("❤️").count(), 2);
            assert!(output.rfind("❤️").unwrap() < output.find('P').unwrap());
            screen.writer_mut().clear();
            screen.render().unwrap();
            assert!(screen.writer().is_empty());
        }
    }

    #[test]
    fn overlay_replays_a_wide_glyph_when_only_its_continuation_overlaps() {
        let mut screen = Screen::new(Vec::new(), (12, 2));
        screen
            .set_placement(
                1,
                Rect::new(4, 0, 2, 1),
                &PlacementOptions {
                    draw: b"AB".to_vec(),
                    ..Default::default()
                },
            )
            .unwrap();
        screen.set_str((3, 0), "界", Style::EMPTY);
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert!(output.rfind('界').unwrap() > output.find("AB").unwrap());
    }

    #[test]
    fn replacement_uses_old_erase_and_removal_preserves_overlay() {
        let mut screen = Screen::new(Vec::new(), (20, 5));
        let area = Rect::new(1, 1, 8, 2);
        screen.set_placement(1, area, &options()).unwrap();
        screen.render().unwrap();
        screen.writer_mut().clear();
        let new = PlacementOptions {
            erase: b"<NEW ERASE>".to_vec(),
            ..options()
        };
        screen.set_placement(1, area, &new).unwrap();
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert!(output.contains("<ERASE>"));
        assert!(!output.contains("<NEW ERASE>"));
        screen.set_str((2, 1), "TOP", Style::EMPTY);
        screen.remove_placement(1);
        screen.writer_mut().clear();
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert!(output.contains("<NEW ERASE>"));
        assert!(!output.contains("<DRAW>"));
        assert_eq!(screen.cell((2, 1).into()).unwrap().content(), "T");
    }

    #[test]
    fn atomic_overwrite_removes_whole_placement_before_text() {
        let mut screen = Screen::new(Vec::new(), (20, 5));
        let options = PlacementOptions {
            erase_on_overwrite: true,
            ..options()
        };
        screen
            .set_placement(1, Rect::new(1, 1, 8, 2), &options)
            .unwrap();
        screen.render().unwrap();
        screen.writer_mut().clear();
        screen.set_str((3, 2), "X", Style::EMPTY);
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert!(output.find("<ERASE>").unwrap() < output.find('X').unwrap());
        assert!(!output.contains("<DRAW>"));
        assert!(screen.cell((1, 1).into()).unwrap().is_blank());
    }

    #[test]
    fn placement_erasure_preserves_cell_styles() {
        use crate::buffer::SurfaceMut;
        use crate::color::Color;
        let style = Style::new().bold().fg(Color::White).bg(Color::Black);
        let overlay = Style::new().bg(Color::Blue);
        for operation in ["overwrite", "remove", "move", "resize"] {
            let mut screen = Screen::new(Vec::new(), (10, 5));
            let area = Rect::new(1, 1, 4, 2);
            let options = PlacementOptions {
                draw: b"TEXT".to_vec(),
                erase_on_overwrite: true,
                ..Default::default()
            };
            screen.fill_rect(area, &Cell::BLANK.style(&style));
            screen.set_placement(1, area, &options).unwrap();
            assert_eq!(screen.cell((1, 1).into()).unwrap().style, style);
            screen.render().unwrap();
            match operation {
                "overwrite" => {
                    screen.set_str((2, 2), "X", &overlay);
                }
                "remove" => screen.remove_placement(1),
                "move" => screen
                    .set_placement(1, Rect::new(5, 1, 4, 2), &options)
                    .unwrap(),
                "resize" => screen.resize((4, 5)),
                _ => unreachable!(),
            }
            screen.render().unwrap();
            for y in area.y..area.bottom() {
                for x in area.x..area.right().min(screen.size().width) {
                    let cell = screen.cell((x, y).into()).unwrap();
                    if operation == "overwrite" && (x, y) == (2, 2) {
                        assert_eq!(cell.style, overlay);
                        assert_eq!(cell.content(), "X");
                    } else {
                        assert_eq!(cell.style, style, "{operation}: {x},{y}");
                        assert!(cell.is_blank());
                    }
                }
            }
        }
    }

    #[test]
    fn equal_rebuild_and_external_text_edit_do_not_replay_placement() {
        let mut screen = Screen::new(Vec::new(), (20, 5));
        let area = Rect::new(1, 1, 8, 2);
        screen.set_placement(1, area, &options()).unwrap();
        screen.render().unwrap();
        screen.writer_mut().clear();
        screen.set_placement(1, area, &options()).unwrap();
        screen.render().unwrap();
        assert!(screen.writer().is_empty());
        screen.set_str((0, 4), "outside", Style::EMPTY);
        screen.render().unwrap();
        assert!(!String::from_utf8_lossy(screen.writer()).contains("<DRAW>"));
    }

    #[test]
    fn resize_drops_clipped_payload_and_handoff_preserves_pending_edits() {
        let mut screen = Screen::new(Vec::new(), (20, 5));
        screen
            .set_placement(1, Rect::new(1, 1, 8, 2), &options())
            .unwrap();
        screen.render().unwrap();
        screen
            .set_placement(2, Rect::new(10, 1, 8, 2), &options())
            .unwrap();
        screen.insert_above("history").unwrap();
        assert!(screen.cell((10, 1).into()).unwrap().is_placement());
        screen.writer_mut().clear();
        screen.resize((5, 4));
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert!(output.contains("<ERASE>"));
        assert!(!output.contains("<DRAW>"));
        assert!(!output.contains("\0p"));
    }

    #[test]
    fn blank_overlay_changes_coverage_even_when_backing_cells_match() {
        let mut screen = Screen::new(Vec::new(), (20, 5));
        let area = Rect::new(1, 1, 8, 2);
        screen.set_placement(1, area, &options()).unwrap();
        screen.render().unwrap();
        screen.writer_mut().clear();
        screen.set_cell((2, 1), &Cell::BLANK);
        screen.render().unwrap();
        assert!(String::from_utf8_lossy(screen.writer()).contains("<DRAW>"));
        screen.writer_mut().clear();
        screen.set_placement(1, area, &options()).unwrap();
        screen.render().unwrap();
        assert!(String::from_utf8_lossy(screen.writer()).contains("<DRAW>"));
    }

    #[test]
    fn foreign_and_malformed_markers_fail_before_output() {
        let mut first = Screen::new(Vec::new(), (20, 5));
        let mut second = Screen::new(Vec::new(), (20, 5));
        let area = Rect::new(1, 1, 8, 2);
        first.set_placement(1, area, &options()).unwrap();
        second.set_placement(1, area, &options()).unwrap();
        second.set_cell((1, 1), first.cell((1, 1).into()).unwrap());
        assert!(second.render().is_err());
        assert!(second.writer().is_empty());
        let mut empty = Screen::new(Vec::new(), (20, 5));
        empty.set_cell((1, 1), &Cell::new("\0p2:unknown", 0));
        assert!(empty.render().is_err());
        assert!(empty.writer().is_empty());
    }

    #[test]
    fn invalid_overlay_keeps_failing_until_corrected() {
        let mut screen = Screen::new(Vec::new(), (20, 5));
        screen
            .set_placement(1, Rect::new(1, 1, 8, 2), &options())
            .unwrap();
        screen.render().unwrap();
        screen.writer_mut().clear();
        screen.set_cell((2, 1), &Cell::new("\n", 1));
        for _ in 0..2 {
            assert!(screen.render().is_err());
            assert!(screen.writer().is_empty());
        }
        screen.set_str((2, 1), "X", Style::EMPTY);
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert!(output.find("<DRAW>").unwrap() < output.rfind('X').unwrap());
    }

    #[test]
    fn mixed_frames_share_one_bracket_and_restore_the_cursor() {
        for synchronized in [false, true] {
            let mut screen = Screen::new(Vec::new(), (20, 5));
            screen.set_fullscreen(true);
            screen.set_cursor_visible(true);
            screen.set_synchronized_output(synchronized);
            screen.set_cursor_position((19, 4));
            for stage in 0..4 {
                screen.writer_mut().clear();
                screen.set_str((0, 0), &stage.to_string(), Style::EMPTY);
                if stage == 1 {
                    screen
                        .set_placement(1, Rect::new(1, 1, 8, 2), &options())
                        .unwrap();
                    screen.set_str((2, 1), "TOP", Style::EMPTY);
                } else if stage == 3 {
                    screen.remove_placement(1);
                }
                screen.render().unwrap();
                let output = String::from_utf8_lossy(screen.writer());
                let (open, close) = if synchronized {
                    ("\x1b[?2026h", "\x1b[?2026l")
                } else {
                    ("\x1b[?25l", "\x1b[?25h")
                };
                assert_eq!(output.matches(open).count(), 1);
                assert_eq!(output.matches(close).count(), 1);
                assert_eq!(screen.tracked_cursor(), Some((19, 4).into()));
                if stage == 1 {
                    assert!(output.find("<DRAW>").unwrap() < output.rfind("TOP").unwrap());
                } else if stage == 2 {
                    assert!(!output.contains("<DRAW>"));
                } else if stage == 3 {
                    assert!(output.contains("<ERASE>"));
                    assert!(!output.contains("<DRAW>"));
                }
                screen.writer_mut().clear();
                screen.render().unwrap();
                assert!(screen.writer().is_empty());
            }
        }
    }

    #[test]
    fn synchronized_frame_contains_payload_and_cursor_without_visibility_toggle() {
        let mut screen = Screen::new(Vec::new(), (20, 5));
        screen.set_synchronized_output(true);
        screen.set_cursor_visible(true);
        screen.set_cursor_position((0, 4));
        screen
            .set_placement(1, Rect::new(1, 1, 8, 2), &options())
            .unwrap();
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert_eq!(output.matches("\x1b[?2026h").count(), 1);
        assert_eq!(output.matches("\x1b[?2026l").count(), 1);
        assert!(!output.contains("\x1b[?25"));
        assert!(output.find("<DRAW>").unwrap() < output.find("\x1b[?2026l").unwrap());
        assert_eq!(screen.tracked_cursor(), Some((0, 4).into()));
    }

    #[test]
    fn failed_output_preserves_old_cleanup_and_requires_explicit_recovery() {
        #[derive(Default)]
        struct Writer {
            bytes: Vec<u8>,
            limit: Option<usize>,
            fail_flush: bool,
        }
        impl Write for Writer {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                let count = self.limit.unwrap_or(bytes.len()).min(bytes.len());
                if count == 0 {
                    return Err(io::Error::other("injected write failure"));
                }
                self.bytes.extend_from_slice(&bytes[..count]);
                if let Some(limit) = &mut self.limit {
                    *limit -= count;
                }
                Ok(count)
            }
            fn flush(&mut self) -> io::Result<()> {
                if self.fail_flush {
                    Err(io::Error::other("injected flush failure"))
                } else {
                    Ok(())
                }
            }
        }
        for fullscreen in [false, true] {
            for fail_flush in [false, true] {
                let mut screen = Screen::new(Writer::default(), (20, 5));
                screen.set_fullscreen(fullscreen);
                screen
                    .set_placement(1, Rect::new(1, 1, 8, 2), &options())
                    .unwrap();
                screen.render().unwrap();
                screen.writer_mut().bytes.clear();
                screen.writer_mut().fail_flush = fail_flush;
                screen.writer_mut().limit = if fail_flush { None } else { Some(8) };
                screen.resize((20, 4));
                screen
                    .set_placement(
                        1,
                        Rect::new(3, 2, 8, 2),
                        &PlacementOptions {
                            erase: b"<NEW ERASE>".to_vec(),
                            ..options()
                        },
                    )
                    .unwrap();
                assert!(screen.render().is_err());
                assert_eq!(screen.renderer.last_size().unwrap().1, 5);
                screen.writer_mut().limit = None;
                screen.writer_mut().fail_flush = false;
                screen.writer_mut().bytes.clear();
                assert!(screen.render().is_err());
                assert!(screen.writer().bytes.is_empty());
                screen.invalidate();
                screen.render().unwrap();
                let output = String::from_utf8_lossy(&screen.writer().bytes);
                assert!(output.contains("<ERASE>"));
                assert!(output.contains("<NEW ERASE>"));
                assert_eq!(output.matches("<DRAW>").count(), 1);
                screen.writer_mut().bytes.clear();
                screen.remove_placement(1);
                screen.render().unwrap();
                let output = String::from_utf8_lossy(&screen.writer().bytes);
                assert!(output.contains("<NEW ERASE>"));
                assert!(!output.contains("<ERASE>"));
            }
        }
    }

    #[test]
    fn clipped_wide_text_on_a_placement_row_is_normalized() {
        let mut screen = Screen::new(Vec::new(), (20, 5));
        screen
            .set_placement(1, Rect::new(1, 1, 8, 2), &options())
            .unwrap();
        screen.set_str((18, 1), "界", Style::EMPTY);
        screen.render().unwrap();
        screen.resize((19, 5));
        screen.render().unwrap();
    }

    #[test]
    fn zero_dimensions_release_placements_without_panicking() {
        for size in [(0, 5), (5, 0), (0, 0)] {
            let mut screen = Screen::new(Vec::new(), (20, 5));
            screen
                .set_placement(1, Rect::new(1, 1, 8, 2), &options())
                .unwrap();
            screen.render().unwrap();
            screen.resize(size);
            screen.render().unwrap();
            screen.render().unwrap();
        }
    }

    #[test]
    fn options_remain_unchanged_and_empty_wide_cells_emit_their_full_width() {
        use crate::color::Color;
        let mut screen = Screen::new(Vec::new(), (20, 5));
        let options = options();
        let original = options.clone();
        screen
            .set_placement(1, Rect::new(1, 1, 8, 2), &options)
            .unwrap();
        screen.set_cell(
            (14, 1),
            &Cell::new("", 3).style(Style::default().fg(Color::Red)),
        );
        screen.render().unwrap();
        assert_eq!(options, original);
        assert!(String::from_utf8_lossy(screen.writer()).contains("\x1b[31m   "));
    }
}
