use std::io::{self, Write};

use super::{PreparedRegion, Region, RegionContext, RegionOp, Screen};
use crate::ansi;
pub use crate::ansi::text_sizing::TextSizing;
use crate::layout::{Position, Rect, Size};
use crate::style::Style;

/// Owned text and style for one explicitly sized Kitty OSC 66 block.
///
/// Use [`Screen::set_sized_text`](crate::screen::Screen::set_sized_text) for
/// normal application code. It derives the rectangle and delegates to the
/// public region API.
///
/// Text contains at most 4096 UTF-8 bytes and must be escape-code safe.
/// An empty string draws a space. A leading combining character is rejected
/// because it would modify adjacent content instead of creating an independent
/// block. Interior combining characters are allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizedText {
    text: String,
    style: Style,
    sizing: TextSizing,
}

impl SizedText {
    /// Validate and own the content of a sized text block.
    pub fn new(text: &str, style: impl Into<Style>, sizing: TextSizing) -> io::Result<Self> {
        sizing.size()?;
        ansi::text_sizing::validate_text(text)?;
        if text
            .chars()
            .next()
            .is_some_and(|ch| crate::text::char_width(ch, false) == 0)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sized text regions must start with a spacing character",
            ));
        }
        Ok(Self {
            text: text.to_owned(),
            style: style.into(),
            sizing,
        })
    }

    /// The exact rectangle size derived from the validated metadata.
    pub fn size(&self) -> Size {
        Size::new(
            u16::from(self.sizing.scale) * u16::from(self.sizing.width),
            u16::from(self.sizing.scale),
        )
    }
}

impl Region for SizedText {
    fn same(&self, other: &dyn Region) -> bool {
        other.downcast_ref::<Self>() == Some(self)
    }

    fn prepare(&self, size: Size, _: RegionContext) -> io::Result<PreparedRegion> {
        if size != self.size() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sized text rectangle must match its metadata",
            ));
        }
        let mut bytes = Vec::new();
        ansi::text_sizing::write_sized_text(&mut bytes, &self.text, self.sizing)?;
        Ok(PreparedRegion {
            draw: vec![
                RegionOp::SetStyle(self.style.clone()),
                RegionOp::Write(bytes),
            ],
            erase: vec![RegionOp::Erase(Rect::new(0, 0, size.width, size.height))],
        })
    }
}

impl<W: Write> Screen<W> {
    /// Draw sized text through the generic region API.
    ///
    /// `sizing` determines the rectangle. The caller supplies only a stable
    /// key, position, content, style, and sizing metadata.
    /// The terminal must support the requested OSC 66 width and scale.
    ///
    /// # Errors
    ///
    /// Returns invalid metadata, text, or geometry errors.
    pub fn set_sized_text(
        &mut self,
        key: &str,
        pos: impl Into<Position>,
        text: &str,
        style: impl Into<crate::style::Style>,
        sizing: TextSizing,
    ) -> io::Result<()> {
        let content = SizedText::new(text, style, sizing)?;
        let size = content.size();
        let pos = pos.into();
        self.set_region(
            key,
            Rect::new(pos.x, pos.y, size.width, size.height),
            content,
        )
    }
}

#[cfg(uncurses_bench)]
mod bench {
    extern crate test;

    use super::*;
    use test::{Bencher, black_box};

    fn cursor_frames(b: &mut Bencher, size: (u16, u16), region: bool) {
        let mut screen = Screen::new(io::sink(), size);
        screen.set_fullscreen(true);
        if region {
            screen
                .set_sized_text(
                    "title",
                    (2, 2),
                    "X",
                    Style::default(),
                    TextSizing::default(),
                )
                .unwrap();
        }
        screen.render().unwrap();
        let mut x = 0;
        b.iter(|| {
            x ^= 1;
            screen.set_cursor_position((x, 0));
            black_box(screen.render().unwrap());
        });
    }

    #[bench]
    fn cursor_only_text_160x48(b: &mut Bencher) {
        cursor_frames(b, (160, 48), false);
    }

    #[bench]
    fn cursor_only_region_160x48(b: &mut Bencher) {
        cursor_frames(b, (160, 48), true);
    }

    #[bench]
    fn cursor_only_text_240x80(b: &mut Bencher) {
        cursor_frames(b, (240, 80), false);
    }

    #[bench]
    fn cursor_only_region_240x80(b: &mut Bencher) {
        cursor_frames(b, (240, 80), true);
    }

    fn overlay_frames(b: &mut Bencher, rebuild: bool) {
        use crate::buffer::SurfaceMut;
        use crate::cell::Cell;

        let mut screen = Screen::new(io::sink(), (240, 80));
        screen.set_fullscreen(true);
        let place = |screen: &mut Screen<io::Sink>| {
            screen
                .set_sized_text(
                    "title",
                    (2, 2),
                    "Build",
                    Style::default(),
                    TextSizing {
                        scale: 2,
                        width: 5,
                        ..Default::default()
                    },
                )
                .unwrap();
        };
        let cells = [Cell::new("0", 1), Cell::new("1", 1)];
        place(&mut screen);
        screen.set_cell((3, 3), &cells[0]);
        screen.render().unwrap();
        let mut tick = 0;
        b.iter(|| {
            if rebuild {
                screen.clear();
                place(&mut screen);
            } else {
                tick ^= 1;
            }
            screen.set_cell((3, 3), &cells[tick]);
            black_box(screen.render().unwrap());
        });
    }

    #[bench]
    fn sparse_text_overlay_240x80(b: &mut Bencher) {
        overlay_frames(b, false);
    }

    #[bench]
    fn equal_overlay_rebuild_240x80(b: &mut Bencher) {
        overlay_frames(b, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Surface, SurfaceMut, View, Window};
    use crate::cell::Cell;
    use crate::color::Profile;
    use crate::screen::RegionContext;
    use crate::screen::regions::tests::{TestRegion, output, screen};
    use crate::style::Style;
    use crate::text::TextSurface;
    use std::sync::Arc;

    fn sized(screen: &mut Screen<Vec<u8>>, key: &str, pos: (u16, u16), text: &str) {
        screen
            .set_sized_text(
                key,
                pos,
                text,
                Style::default(),
                TextSizing {
                    scale: 2,
                    width: 3,
                    ..Default::default()
                },
            )
            .unwrap();
    }

    fn text(screen: &Screen<Vec<u8>>, pos: (u16, u16)) -> &str {
        screen.cell(pos.into()).unwrap().content()
    }

    #[test]
    fn regions_identical_rebuild_is_empty() {
        let mut s = screen();
        for i in 0..3 {
            s.clear();
            s.set_str((0, 0), "base cells", Style::default());
            sized(&mut s, "title", (2, 2), "Build");
            s.set_cursor_position((0, 7));
            let bytes = output(&mut s);
            assert_eq!(bytes.is_empty(), i != 0);
        }
    }

    #[test]
    fn regions_overwrite_keeps_the_region_and_equal_rebuild_is_empty() {
        let mut s = screen();
        for i in 0..3 {
            s.clear();
            sized(&mut s, "title", (2, 2), "Build");
            s.set_str((2, 2), "X", Style::default());
            assert_eq!(s.renderer.regions.as_ref().unwrap().entries.len(), 1);
            let bytes = output(&mut s);
            if i == 0 {
                let draw = bytes.windows(4).position(|b| b == b"]66;").unwrap();
                let text = bytes.iter().rposition(|b| *b == b'X').unwrap();
                assert!(draw < text);
            }
            assert_eq!(bytes.is_empty(), i != 0);
        }
        s.set_str((2, 2), "Y", Style::default());
        assert!(!output(&mut s).windows(4).any(|b| b == b"]66;"));
        sized(&mut s, "title", (2, 2), "Build");
        assert!(output(&mut s).windows(4).any(|b| b == b"]66;"));
        assert!(output(&mut s).is_empty());
    }

    #[test]
    fn regions_equal_text_is_still_an_ordered_write() {
        let mut s = screen();
        s.set_str((2, 2), "X", Style::default());
        sized(&mut s, "title", (2, 2), "Build");
        output(&mut s);
        s.set_str((2, 2), "X", Style::default());
        let bytes = output(&mut s);
        assert!(bytes.contains(&b'X'));
        assert!(bytes.windows(4).any(|b| b == b"]66;"));
        assert!(output(&mut s).is_empty());
    }

    #[test]
    fn regions_lower_row_strings_retain_coordinates_and_restore_base() {
        for incremental in [false, true] {
            let mut s = screen();
            s.set_str((2, 2), "old", Style::default());
            sized(&mut s, "title", (2, 2), "Build");
            if incremental {
                output(&mut s);
            }
            s.set_str((3, 3), "abc", Style::default());
            assert_eq!(s.renderer.regions.as_ref().unwrap().entries.len(), 1);
            let bytes = output(&mut s);
            assert!(bytes.windows(3).any(|b| b == b"abc"));
            assert!(!bytes.windows(3).any(|b| b == b"old"));
            assert!(bytes.windows(4).any(|b| b == b"]66;"));
            for (x, value) in [(3, "a"), (4, "b"), (5, "c"), (8, " ")] {
                assert_eq!(text(&s, (x, 3)), value);
            }
            s.remove_region("title");
            assert!(output(&mut s).windows(3).any(|b| b == b"old"));
            assert!(s.renderer.regions.is_none());
        }
    }

    #[test]
    fn regions_lower_row_view_write_stays_at_its_requested_position() {
        let mut s = screen();
        sized(&mut s, "title", (2, 2), "Build");
        output(&mut s);
        View::new(&mut s, (3, 3, 1, 1)).set_cell((3, 3).into(), &Cell::new("X", 1));
        let bytes = output(&mut s);
        assert!(bytes.contains(&b'X'));
        assert!(s.renderer.has_displayed_regions());
        assert_eq!(text(&s, (3, 3)), "X");
        assert_eq!(text(&s, (8, 3)), " ");
    }

    #[test]
    fn regions_replacement_on_a_lower_row_keeps_exact_geometry() {
        let mut s = screen();
        s.set_str((2, 2), "base", Style::default());
        sized(&mut s, "old", (2, 2), "Old");
        output(&mut s);
        sized(&mut s, "new", (3, 3), "New");
        let bytes = output(&mut s);
        assert!(bytes.windows(4).any(|b| b == b"base"));
        let displayed = &s.renderer.regions.as_ref().unwrap().displayed;
        assert_eq!(displayed.len(), 1);
        assert_eq!(displayed["new"].rect, Rect::new(3, 3, 6, 2));
    }

    #[test]
    fn regions_retain_hidden_base_without_redrawing_equal_content() {
        let mut s = screen();
        s.set_str((2, 2), "before", Style::default());
        sized(&mut s, "title", (2, 2), "Build");
        output(&mut s);
        s.set_str((2, 2), "after!", Style::default());
        sized(&mut s, "title", (2, 2), "Build");
        assert!(output(&mut s).is_empty());
        assert_eq!(text(&s, (2, 2)), "a");
        s.remove_region("title");
        assert!(output(&mut s).windows(6).any(|b| b == b"after!"));
    }

    #[test]
    fn regions_move_and_remove_restore_current_base() {
        let mut s = screen();
        s.set_str((2, 2), "before", Style::default());
        sized(&mut s, "title", (2, 2), "Build");
        output(&mut s);
        s.set_str((2, 2), "after!", Style::default());
        sized(&mut s, "title", (2, 2), "Build");
        output(&mut s);
        sized(&mut s, "title", (20, 2), "Build");
        assert_eq!(text(&s, (2, 2)), "a");
        let bytes = output(&mut s);
        assert!(bytes.windows(6).any(|b| b == b"after!"));
        s.remove_region("title");
        output(&mut s);
        assert!(s.renderer.regions.is_none());
    }

    #[test]
    fn regions_same_position_has_distinct_keys_and_swaps_work() {
        let mut s = screen();
        sized(&mut s, "a", (2, 2), "A");
        sized(&mut s, "b", (2, 2), "B");
        output(&mut s);
        let scene = s.renderer.regions.as_ref().unwrap();
        assert!(!scene.entries.contains_key("a"));
        assert!(scene.entries.contains_key("b"));
        sized(&mut s, "a", (20, 2), "A");
        output(&mut s);
        sized(&mut s, "a", (2, 2), "A");
        sized(&mut s, "b", (20, 2), "B");
        output(&mut s);
        assert_eq!(s.renderer.regions.as_ref().unwrap().displayed.len(), 2);
        assert!(output(&mut s).is_empty());
    }

    #[test]
    fn regions_view_window_and_mutable_access_establish_order() {
        let mut s = screen();
        sized(&mut s, "title", (2, 2), "Build");
        {
            let mut view = View::new(&mut s, (2, 2, 1, 1));
            view.set_cell((2, 2).into(), &Cell::BLANK);
        }
        assert_eq!(s.renderer.regions.as_ref().unwrap().entries.len(), 1);
        sized(&mut s, "title", (2, 2), "Build");
        let mut window = Window::new(1, 1);
        window.set_position((2, 2));
        window.present(&mut s);
        assert_eq!(s.renderer.regions.as_ref().unwrap().entries.len(), 1);
        sized(&mut s, "title", (2, 2), "Build");
        let _ = s.cell_mut((2, 2)).unwrap();
        output(&mut s);
        assert!(s.renderer.has_displayed_regions());
    }

    #[test]
    fn regions_mutable_continuation_access_overlays_the_whole_glyph() {
        let mut s = screen();
        s.set_cell((1, 2), &Cell::new("\u{754c}", 2));
        sized(&mut s, "title", (2, 2), "Build");
        output(&mut s);
        assert!(s.cell_mut((2, 2)).unwrap().is_continuation());
        assert_eq!(s.renderer.regions.as_ref().unwrap().entries.len(), 1);
        let bytes = output(&mut s);
        assert!(bytes.windows(3).any(|b| b == "\u{754c}".as_bytes()));
        assert!(s.renderer.has_displayed_regions());
        assert!(output(&mut s).is_empty());
    }

    #[test]
    fn regions_unrendered_clear_preserves_cleanup_ownership() {
        let mut s = screen();
        sized(&mut s, "title", (2, 2), "Build");
        output(&mut s);
        s.clear();
        s.release_regions().unwrap();
        assert!(!s.writer().is_empty());
        assert!(!s.renderer.has_displayed_regions());
        output(&mut s);
        assert!(s.renderer.regions.is_none());
    }

    #[test]
    fn regions_resize_validation_preserves_staged_prefix() {
        let mut s = screen();
        sized(&mut s, "title", (20, 2), "Build");
        output(&mut s);
        s.resize((10, 8));
        s.write_all(b"prefix").unwrap();
        assert_eq!(s.render().unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert!(s.writer().is_empty());
        assert_eq!(s.out_buf, b"prefix");
        s.remove_region("title");
        assert!(output(&mut s).starts_with(b"prefix"));
    }

    #[test]
    fn regions_reject_uncertain_text_tail_before_output() {
        let mut s = screen();
        s.set_cell((0, 2), &Cell::new("ab", 2));
        output(&mut s);
        sized(&mut s, "title", (8, 2), "Build");
        assert_eq!(s.render().unwrap_err().kind(), io::ErrorKind::Unsupported);
        assert!(s.writer().is_empty());
        s.set_grapheme_clusters(true);
        output(&mut s);
    }

    #[test]
    fn region_overlays_reject_uncertain_width_at_the_right_edge_before_output() {
        let mut s = screen();
        sized(&mut s, "title", (2, 2), "Build");
        output(&mut s);
        s.set_cell((6, 2), &Cell::new("ab", 2));
        assert_eq!(s.render().unwrap_err().kind(), io::ErrorKind::Unsupported);
        assert!(s.writer().is_empty());
        s.set_grapheme_clusters(true);
        output(&mut s);
        s.set_grapheme_clusters(false);
        assert_eq!(s.render().unwrap_err().kind(), io::ErrorKind::Unsupported);
        assert!(s.writer().is_empty());
    }

    #[test]
    fn regions_external_implementation_and_type_replacement() {
        let mut s = screen();
        s.set_region("same", Rect::new(2, 2, 1, 1), TestRegion(b'T', false))
            .unwrap();
        output(&mut s);
        sized(&mut s, "same", (10, 2), "Build");
        let bytes = output(&mut s);
        let cleanup = b"\x1b[X";
        let erase = bytes
            .windows(cleanup.len())
            .position(|b| b == cleanup)
            .unwrap();
        let draw = bytes.windows(4).position(|b| b == b"]66;").unwrap();
        assert!(erase < draw);
    }

    #[test]
    fn regions_metadata_and_text_boundaries() {
        let mut s = screen();
        for pos in [(u16::MAX, 0), (0, u16::MAX)] {
            assert_eq!(
                s.set_sized_text(
                    "overflow",
                    pos,
                    "X",
                    Style::default(),
                    TextSizing::default()
                )
                .unwrap_err()
                .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        assert!(s.renderer.regions.is_none());
        for (scale, width, valid) in [
            (1, 1, true),
            (7, 7, true),
            (0, 1, false),
            (1, 0, false),
            (8, 1, false),
            (1, 8, false),
        ] {
            assert_eq!(
                TextSizing {
                    scale,
                    width,
                    ..Default::default()
                }
                .size()
                .is_ok(),
                valid
            );
        }
        for text in ["\x1b", "\x07", "\n", "\u{9c}", "\u{301}"] {
            assert!(SizedText::new(text, Style::default(), TextSizing::default()).is_err());
        }
        for text in ["", "a\u{301}", "hello", &"x".repeat(4096)] {
            assert!(SizedText::new(text, Style::default(), TextSizing::default()).is_ok());
        }
        assert!(
            SizedText::new(&"x".repeat(4097), Style::default(), TextSizing::default()).is_err()
        );
        assert!(
            TextSizing {
                numerator: 2,
                denominator: 2,
                ..Default::default()
            }
            .size()
            .is_err()
        );
    }

    #[test]
    fn regions_failed_resize_frame_preserves_ownership_and_retries() {
        let mut s = screen();
        sized(&mut s, "old", (2, 2), "Build");
        output(&mut s);
        let old = Arc::clone(&s.renderer.regions.as_ref().unwrap().displayed["old"]);
        s.resize((45, 9));
        s.set_str((0, 0), "new base", Style::default());
        s.set_region("a", Rect::new(20, 2, 1, 1), TestRegion(b'A', false))
            .unwrap();
        s.set_region("z", Rect::new(30, 2, 1, 1), TestRegion(b'Z', true))
            .unwrap();
        assert!(s.render().is_err());
        assert!(s.writer().is_empty());
        assert!(s.out_buf.is_empty());
        let scene = s.renderer.regions.as_ref().unwrap();
        assert_eq!(scene.displayed.len(), 1);
        assert!(Arc::ptr_eq(&scene.displayed["old"], &old));
        assert!(s.renderer.region_intersects(old.rect));
        assert!(!s.renderer.region_intersects(Rect::new(20, 2, 1, 1)));
        assert!(!s.renderer.region_intersects(Rect::new(30, 2, 1, 1)));
        assert!(s.renderer.redraw_pending());

        s.set_region("z", Rect::new(30, 2, 1, 1), TestRegion(b'Z', false))
            .unwrap();
        let bytes = output(&mut s);
        assert!(bytes.windows(8).any(|b| b == b"new base"));
        assert!(bytes.windows(4).any(|b| b == b"]66;"));
        assert_eq!(s.renderer.regions.as_ref().unwrap().displayed.len(), 3);
        assert!(output(&mut s).is_empty());
    }

    #[test]
    fn regions_keep_only_retained_physical_barriers_during_cell_emission() {
        let mut s = screen();
        sized(&mut s, "moving", (0, 2), "Build");
        sized(&mut s, "retained", (20, 2), "Build");
        output(&mut s);
        sized(&mut s, "moving", (0, 4), "Build");
        let mut frame = s.renderer.prepare_regions(&s.front_buf).unwrap().unwrap();
        s.renderer.sync_frame(&mut s.front_buf);
        let size = s.size();
        s.renderer.retire_regions(&mut s.out_buf, &mut frame, size);
        assert!(s.renderer.region_intersects(Rect::new(20, 2, 10, 2)));
        assert!(!s.renderer.region_intersects(Rect::new(0, 2, 10, 2)));
        assert!(!s.renderer.region_intersects(Rect::new(0, 4, 10, 2)));
        s.renderer.draw_regions(&mut s.out_buf, Some(&frame), size);
        assert!(s.renderer.region_intersects(Rect::new(20, 2, 10, 2)));
        assert!(s.renderer.region_intersects(Rect::new(0, 4, 10, 2)));
    }

    #[test]
    fn regions_restore_wide_base_and_apply_bulk_writes() {
        let mut s = screen();
        s.set_cell((1, 2), &Cell::new("\u{754c}", 2));
        sized(&mut s, "title", (2, 2), "Build");
        output(&mut s);
        s.remove_region("title");
        let bytes = output(&mut s);
        assert!(bytes.windows(3).any(|b| b == "\u{754c}".as_bytes()));
        assert!(s.renderer.regions.is_none());
        sized(&mut s, "title", (2, 2), "Build");
        output(&mut s);
        s.insert_lines(2, 1, 6, &Cell::BLANK);
        let bytes = output(&mut s);
        assert!(bytes.windows(4).any(|b| b == b"]66;"));
        assert!(s.renderer.has_displayed_regions());
        assert_eq!(text(&s, (1, 3)), "\u{754c}");
    }

    #[test]
    fn regions_keep_safe_prefix_erase_and_suffix_line_erase() {
        let mut s = screen();
        s.set_optimizations(crate::screen::Optimizations::xterm());
        s.set_str((0, 2), &"x".repeat(20), Style::default());
        sized(&mut s, "title", (25, 2), "Build");
        output(&mut s);
        s.clear_rect(Rect::new(0, 2, 20, 1));
        let bytes = output(&mut s);
        assert!(
            bytes.windows(5).any(|b| b == b"\x1b[20X"),
            "{:?}",
            bytes.escape_ascii()
        );
        assert!(!bytes.windows(4).any(|b| b == b"]66;"));

        s.set_str((32, 2), "xxxxxxx", Style::default());
        output(&mut s);
        s.clear_rect(Rect::new(32, 2, 8, 1));
        let bytes = output(&mut s);
        assert!(
            bytes.windows(3).any(|b| b == b"\x1b[K"),
            "{:?}",
            bytes.escape_ascii()
        );
        assert!(!bytes.windows(4).any(|b| b == b"]66;"));
        assert!(output(&mut s).is_empty());
    }

    #[test]
    fn regions_keep_safe_suffix_insert_delete() {
        let mut s = screen();
        s.set_optimizations(crate::screen::Optimizations::xterm());
        sized(&mut s, "title", (2, 2), "Build");
        s.set_str((12, 2), "A", Style::default());
        output(&mut s);
        s.insert_cells((12, 2).into(), 8, 40, &Cell::new("x", 1));
        let inserted = output(&mut s);
        assert!(
            inserted.windows(4).any(|b| b == b"\x1b[8@"),
            "{}",
            inserted.escape_ascii()
        );
        s.set_str((12, 2), "abcdefghijklmnopqrstuvwxy", Style::default());
        output(&mut s);
        s.delete_cells((12, 2).into(), 1, 40, &Cell::BLANK);
        let deleted = output(&mut s);
        assert!(
            deleted.windows(3).any(|b| b == b"\x1b[P"),
            "{}",
            deleted.escape_ascii()
        );
        assert!(!inserted.windows(4).any(|b| b == b"]66;"));
        assert!(!deleted.windows(4).any(|b| b == b"]66;"));
    }

    #[test]
    fn regions_cursor_moves_never_reprint_a_covered_placeholder() {
        let mut s = screen();
        s.set_sized_text("one", (1, 0), "A", Style::default(), TextSizing::default())
            .unwrap();
        s.set_cursor_position((0, 0));
        output(&mut s);
        s.move_cursor_to((2, 0)).unwrap();
        assert!(
            !s.writer().contains(&b' '),
            "{:?}",
            s.writer().escape_ascii()
        );
        assert!(
            s.renderer
                .regions
                .as_ref()
                .unwrap()
                .entries
                .contains_key("one")
        );
    }

    struct FailingWriter {
        bytes: Vec<u8>,
        remaining: usize,
        fail_flush: bool,
    }

    #[test]
    fn regions_insert_above_preserves_displayed_content_and_pending_edits() {
        let mut s = screen();
        s.set_str((0, 4), "displayed", Style::default());
        sized(&mut s, "title", (1, 0), "Build");
        output(&mut s);
        s.set_str((0, 4), "pending", Style::default());
        sized(&mut s, "title", (20, 2), "Next");
        s.insert_above("log").unwrap();
        let bytes = std::mem::take(s.writer_mut());
        assert!(bytes.windows(3).any(|b| b == b"log"));
        assert!(bytes.windows(5).any(|b| b == b"Build"));
        assert!(!bytes.windows(4).any(|b| b == b"Next"));
        assert!(!bytes.windows(7).any(|b| b == b"pending"));
        assert!(s.front_buf.has_changes());
        assert_eq!(
            s.renderer.regions.as_ref().unwrap().displayed["title"]
                .rect
                .position(),
            Position::new(1, 0)
        );
        let bytes = output(&mut s);
        assert!(bytes.windows(4).any(|b| b == b"Next"));
        assert!(bytes.windows(7).any(|b| b == b"pending"));
        s.insert_above("").unwrap();
        assert!(s.writer().is_empty());
    }

    #[test]
    fn regions_insert_above_after_encoder_error_preserves_displayed_cells() {
        for invalid_geometry in [false, true] {
            let mut s = screen();
            s.set_str((0, 4), "displayed", Style::default());
            sized(&mut s, "title", (1, 0), "Build");
            output(&mut s);
            s.set_str((0, 4), "PENDING!!", Style::default());
            sized(&mut s, "title", (20, 2), "Next");
            if invalid_geometry {
                s.set_region(
                    "bad",
                    Rect::new(30, 5, 2, 1),
                    SizedText::new("X", Style::default(), TextSizing::default()).unwrap(),
                )
                .unwrap();
            } else {
                s.set_region("bad", Rect::new(30, 5, 1, 1), TestRegion(b'X', true))
                    .unwrap();
            }
            assert!(s.render().is_err());
            assert!(s.writer().is_empty());
            s.insert_above("log").unwrap();
            let bytes = std::mem::take(s.writer_mut());
            assert!(bytes.windows(9).any(|b| b == b"displayed"));
            assert!(!bytes.windows(9).any(|b| b == b"PENDING!!"));
            assert!(bytes.windows(5).any(|b| b == b"Build"));
            assert!(!bytes.windows(4).any(|b| b == b"Next"));
            assert!(s.front_buf.has_changes());
            s.remove_region("bad");
            let bytes = output(&mut s);
            assert!(bytes.windows(9).any(|b| b == b"PENDING!!"));
            assert!(bytes.windows(4).any(|b| b == b"Next"));
            assert!(output(&mut s).is_empty());
        }
    }

    #[test]
    fn regions_insert_above_accepts_unrendered_declarations() {
        let mut s = screen();
        sized(&mut s, "title", (1, 0), "Build");
        s.insert_above("log").unwrap();
        assert!(!s.writer().windows(4).any(|b| b == b"]66;"));
        assert!(!s.renderer.has_displayed_regions());
        s.writer_mut().clear();
        assert!(output(&mut s).windows(4).any(|b| b == b"]66;"));
    }

    #[test]
    fn regions_replay_fitting_blocks_after_resize_without_pending_edits() {
        let mut s = screen();
        s.set_str((0, 6), "displayed", Style::default());
        sized(&mut s, "title", (1, 0), "Build");
        output(&mut s);
        for size in [(60, 12), (20, 7)] {
            s.resize(size);
            s.set_str((0, 6), "PENDING!!", Style::default());
            sized(&mut s, "title", (10, 2), "Next");
            s.insert_above("log").unwrap();
            let bytes = std::mem::take(s.writer_mut());
            assert!(bytes.windows(9).any(|bytes| bytes == b"displayed"));
            assert!(bytes.windows(5).any(|bytes| bytes == b"Build"));
            assert!(!bytes.windows(9).any(|bytes| bytes == b"PENDING!!"));
            assert!(!bytes.windows(4).any(|bytes| bytes == b"Next"));
            assert!(s.front_buf.has_changes());
            assert!(s.renderer.redraw_pending());
            assert_eq!(s.renderer.last_size(), size);
        }
        let bytes = output(&mut s);
        assert!(bytes.windows(9).any(|bytes| bytes == b"PENDING!!"));
        assert!(bytes.windows(4).any(|bytes| bytes == b"Next"));
        assert!(output(&mut s).is_empty());
    }

    #[test]
    fn regions_insert_above_waits_for_the_resized_frame() {
        for (old_size, new_size, old_position, new_position) in [
            ((10, 4), (20, 2), (2, 3), (15, 1)),
            ((40, 8), (10, 8), (20, 2), (2, 2)),
        ] {
            for failed_frame in [false, true] {
                let mut s = Screen::new(Vec::new(), old_size);
                s.set_fullscreen(true);
                s.set_sized_text(
                    "title",
                    old_position,
                    "X",
                    Style::default(),
                    TextSizing::default(),
                )
                .unwrap();
                output(&mut s);
                s.resize(new_size);
                s.remove_region("title");
                if failed_frame {
                    s.set_region("bad", Rect::new(0, 0, 1, 1), TestRegion(b'X', true))
                        .unwrap();
                    assert!(s.render().is_err());
                    s.remove_region("bad");
                }
                s.write_all(b"prefix").unwrap();
                let error = s.insert_above("log").unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
                assert_eq!(s.out_buf, b"prefix");
                assert!(s.writer().is_empty());
                assert!(s.renderer.redraw_pending());
                s.set_sized_text(
                    "title",
                    new_position,
                    "Y",
                    Style::default(),
                    TextSizing::default(),
                )
                .unwrap();
                output(&mut s);
                s.insert_above("log").unwrap();
                assert!(s.writer().windows(3).any(|b| b == b"log"));
                s.writer_mut().clear();
                assert!(output(&mut s).is_empty());
            }
        }
    }

    #[test]
    fn regions_cursor_only_frames_match_the_full_frame_path() {
        for synchronized in [false, true] {
            let make_screen = || {
                let mut s = screen();
                s.set_synchronized_output(synchronized);
                s.set_cell((3, 2), &Cell::new("ab", 2));
                sized(&mut s, "title", (2, 2), "Build");
                output(&mut s);
                s
            };
            let mut fast = make_screen();
            let mut full = make_screen();
            for position in [(0, 2), (4, 2), (15, 2), (0, 7)] {
                fast.set_cursor_position(position);
                full.set_cursor_position(position);
                full.front_buf.touch_all();
                let expected = output(&mut full);
                let actual = output(&mut fast);
                assert_eq!(actual, expected);
                assert!(!actual.windows(4).any(|b| b == b"]66;"));
                assert!(!actual.windows(2).any(|b| b == b"ab"));
                assert!(fast.renderer.region_intersects(Rect::new(2, 2, 1, 1)));
                assert!(output(&mut fast).is_empty());
            }
        }
    }

    impl Write for FailingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::other("injected write failure"));
            }

            let count = self.remaining.min(bytes.len());
            self.bytes.extend_from_slice(&bytes[..count]);
            self.remaining -= count;
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

    #[test]
    fn output_failures_preserve_io_errors_and_allow_caller_recovery() {
        for with_regions in [false, true] {
            for (remaining, fail_flush) in [(0, false), (25, false), (usize::MAX, true)] {
                let mut s = Screen::new(
                    FailingWriter {
                        bytes: Vec::new(),
                        remaining,
                        fail_flush,
                    },
                    (40, 8),
                );
                s.set_fullscreen(true);
                let text = "desired text after an output error";
                s.set_str((0, 0), text, Style::default());
                if with_regions {
                    s.set_sized_text(
                        "title",
                        (2, 2),
                        "Build",
                        Style::default(),
                        TextSizing::default(),
                    )
                    .unwrap();
                }
                for _ in 0..2 {
                    s.invalidate();
                    let error = s.render().unwrap_err();
                    assert_eq!(error.kind(), io::ErrorKind::Other);
                    assert_eq!(
                        error.to_string(),
                        if fail_flush {
                            "injected flush failure"
                        } else {
                            "injected write failure"
                        }
                    );
                    assert!(s.out_buf.is_empty());
                    if with_regions {
                        assert!(!s.renderer.has_displayed_regions());
                    }
                }
                let count = s.writer().bytes.len();
                s.writer_mut().remaining = usize::MAX;
                s.writer_mut().fail_flush = false;
                s.write_all(b"\x1b[?1049l").unwrap();
                s.flush().unwrap();
                assert_eq!(&s.writer().bytes[count..], b"\x1b[?1049l");
                s.release_regions().unwrap();
                s.writer_mut().bytes.clear();
                s.invalidate();
                s.render().unwrap();
                assert!(
                    s.writer()
                        .bytes
                        .windows(text.len())
                        .any(|bytes| bytes == text.as_bytes())
                );
                if with_regions {
                    assert_eq!(s.renderer.regions.as_ref().unwrap().displayed.len(), 1);
                    s.release_regions().unwrap();
                    assert!(!s.renderer.has_displayed_regions());
                }
            }
        }
    }

    #[test]
    fn regions_disabled_profile_removes_style_and_links() {
        let text = SizedText::new(
            "A",
            Style::default().bold().link("https://example.com", ""),
            TextSizing::default(),
        )
        .unwrap();
        let mut s = screen();
        s.set_color_profile(Profile::Disabled);
        s.set_region("styled", Rect::new(0, 0, 1, 1), text).unwrap();
        let bytes = output(&mut s);
        assert!(!bytes.windows(3).any(|b| b == b"]8;"));
        assert!(!bytes.windows(4).any(|b| b == b"\x1b[1m"));
    }

    #[test]
    fn regions_reject_out_of_bounds_recipes_before_frame_mutation() {
        #[derive(Clone)]
        struct Recipe(PreparedRegion);
        impl Region for Recipe {
            fn same(&self, other: &dyn Region) -> bool {
                other
                    .downcast_ref::<Self>()
                    .is_some_and(|other| self.0 == other.0)
            }
            fn prepare(&self, _: Size, _: RegionContext) -> io::Result<PreparedRegion> {
                Ok(self.0.clone())
            }
        }
        for op in [
            RegionOp::MoveTo(Position::new(1, 0)),
            RegionOp::SetCursor(Position::new(0, 1)),
            RegionOp::Erase(Rect::new(0, 0, 2, 1)),
        ] {
            for cleanup in [false, true] {
                let mut s = screen();
                sized(&mut s, "old", (2, 2), "Build");
                output(&mut s);
                s.set_str((0, 0), "pending", Style::default());
                s.write_all(b"prefix").unwrap();
                let recipe = if cleanup {
                    PreparedRegion {
                        erase: vec![op.clone()],
                        ..Default::default()
                    }
                } else {
                    PreparedRegion {
                        draw: vec![op.clone()],
                        ..Default::default()
                    }
                };
                s.set_region("bad", Rect::new(20, 2, 1, 1), Recipe(recipe))
                    .unwrap();
                assert_eq!(s.render().unwrap_err().kind(), io::ErrorKind::InvalidInput);
                assert!(s.writer().is_empty());
                assert_eq!(s.out_buf, b"prefix");
                assert!(s.front_buf.has_changes());
                assert_eq!(s.renderer.regions.as_ref().unwrap().displayed.len(), 1);
            }
        }
    }

    #[test]
    fn regions_clear_and_remove_bound_unrendered_storage() {
        let mut s = screen();
        for n in 0..100 {
            s.clear();
            sized(&mut s, &format!("clear-{n}"), (2, 2), "Build");
            assert_eq!(s.renderer.regions.as_ref().unwrap().entries.len(), 1);
        }
        s.clear();
        for n in 0..100 {
            let key = format!("remove-{n}");
            sized(&mut s, &key, (2, 2), "Build");
            s.remove_region(&key);
            assert!(s.renderer.regions.as_ref().unwrap().entries.is_empty());
        }
    }
}
