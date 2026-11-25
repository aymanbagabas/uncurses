use super::*;
use crate::renderer::RenderBuffer;
use crate::style::Style;
use crate::text::{Painter, TextSurface, WidthMode, WrapMode};

fn link_of(s: &Style) -> Option<(&str, &str)> {
    s.link
        .as_deref()
        .map(|l| (l.url.as_str(), l.params.as_str()))
}

#[test]
fn test_new_buffer() {
    let buf = Buffer::new(80, 24);
    assert_eq!(buf.width(), 80);
    assert_eq!(buf.height(), 24);
    assert_eq!(buf.height(), 24);
    assert_eq!(buf.line(0).map(|l| l.len()), Some(80));
}

#[test]
fn test_set_get() {
    let mut buf = Buffer::new(10, 5);
    let cell = Cell::new("X", 1);
    buf.set((3, 2), &cell.clone());
    assert_eq!(buf.cell(Position::new(3, 2)).unwrap().content(), "X");
}

#[test]
fn test_wide_char_set() {
    let mut buf = Buffer::new(10, 1);
    let cell = Cell::new("中", 2);
    buf.set((3, 0), &cell);
    assert_eq!(buf.cell(Position::new(3, 0)).unwrap().content(), "中");
    assert_eq!(buf.cell(Position::new(3, 0)).unwrap().width(), 2);
    assert!(buf.cell(Position::new(4, 0)).unwrap().is_continuation());
}

#[test]
fn test_overwrite_wide_char() {
    let mut buf = Buffer::new(10, 1);
    buf.set((3, 0), &Cell::new("中", 2));
    // Overwrite continuation cell
    buf.set((4, 0), &Cell::new("A", 1));
    // Primary cell should be blanked
    assert!(buf.cell(Position::new(3, 0)).unwrap().is_blank());
    assert_eq!(buf.cell(Position::new(4, 0)).unwrap().content(), "A");
}

#[test]
fn test_broken_wide_cell_keeps_bg() {
    use crate::color::Color;
    let styled = Style::EMPTY.bg(Color::Red);

    // Overwriting a wide cell's continuation blanks the primary; the blank
    // must keep the wide cell's background instead of resetting to default.
    let mut buf = Buffer::new(10, 1);
    buf.set((3, 0), &Cell::new("中", 2).style(styled.clone()));
    buf.set((4, 0), &Cell::new("A", 1));
    let primary = buf.cell(Position::new(3, 0)).unwrap();
    assert!(primary.is_blank());
    assert_eq!(primary.style.bg, Some(Color::Red));

    // Overwriting the primary blanks the trailing continuation; that blank
    // keeps the wide cell's background too.
    let mut buf = Buffer::new(10, 1);
    buf.set((3, 0), &Cell::new("中", 2).style(styled.clone()));
    buf.set((3, 0), &Cell::new("A", 1));
    assert_eq!(
        buf.cell(Position::new(4, 0)).unwrap().style.bg,
        Some(Color::Red)
    );

    // A wide cell that doesn't fit at the right edge is stored as a blank
    // that still carries its background.
    let mut buf = Buffer::new(4, 1);
    buf.set((3, 0), &Cell::new("中", 2).style(styled.clone()));
    let edge = buf.cell(Position::new(3, 0)).unwrap();
    assert!(edge.is_blank());
    assert_eq!(edge.style.bg, Some(Color::Red));
}

fn check_partial_fill_style(use_view: bool) {
    use crate::color::Color;
    use crate::style::AttrFlags;

    let style = Style {
        attrs: AttrFlags::BOLD,
        ..Style::EMPTY
            .fg(Color::White)
            .bg(Color::Red)
            .link("https://example.com/fill", "")
    };
    for width in [2, 3, 8] {
        for columns in [1, 5, 10] {
            let mut buf = Buffer::new(16, 3);
            let rect = Rect::new(2, 1, columns, 1);
            let fill = Cell::new("W", width).style(style.clone());
            if use_view {
                View::new(&mut buf, rect).fill_rect(Rect::new(0, 0, 16, 3), &fill);
            } else {
                buf.fill_rect(rect, &fill);
            }
            let full_columns = columns - columns % u16::from(width);
            for y in 0..3 {
                for x in 0..16 {
                    let pos = Position::new(x, y);
                    let expected = if rect.contains(pos) {
                        let offset = x - rect.left();
                        if offset >= full_columns {
                            Cell::BLANK.style(style.clone())
                        } else if offset.is_multiple_of(u16::from(width)) {
                            fill.clone()
                        } else {
                            Cell::CONTINUATION.style(style.clone())
                        }
                    } else {
                        Cell::BLANK
                    };
                    assert_eq!(
                        buf.cell(pos).unwrap(),
                        &expected,
                        "view={use_view}, width={width}, columns={columns}, pos={pos:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn fill_rect_preserves_fill_style_in_partial_slots() {
    check_partial_fill_style(false);
}

#[test]
fn default_fill_rect_preserves_fill_style_in_partial_slots() {
    check_partial_fill_style(true);
}

fn check_fill_boundary_style(rect: Rect) {
    use crate::color::Color;
    use crate::style::AttrFlags;

    let old_style = Style {
        attrs: AttrFlags::BOLD,
        ..Style::EMPTY
            .fg(Color::Yellow)
            .bg(Color::Blue)
            .link("https://example.com/old", "")
    };
    let fill = Cell::new("x", 1).style(
        Style::EMPTY
            .fg(Color::White)
            .bg(Color::Red)
            .link("https://example.com/fill", ""),
    );
    let mut buf = Buffer::new(12, 3);
    buf.set((2, 1), &Cell::new("W", 8).style(old_style.clone()));
    buf.fill_rect(rect, &fill);
    for y in 0..3 {
        for x in 0..12 {
            let pos = Position::new(x, y);
            let expected = if rect.contains(pos) {
                fill.clone()
            } else if y == 1 && (2..10).contains(&x) {
                Cell::BLANK.style(old_style.clone())
            } else {
                Cell::BLANK
            };
            assert_eq!(
                buf.cell(pos).unwrap(),
                &expected,
                "rect={rect:?}, pos={pos:?}"
            );
        }
    }
}

#[test]
fn fill_rect_preserves_old_style_at_left_edge() {
    check_fill_boundary_style(Rect::new(3, 1, 7, 1));
}

#[test]
fn fill_rect_preserves_old_style_at_right_edge() {
    check_fill_boundary_style(Rect::new(2, 1, 7, 1));
}

#[test]
fn fill_rect_preserves_old_style_outside_both_edges() {
    check_fill_boundary_style(Rect::new(4, 1, 2, 1));
}

#[test]
fn test_overwrite_continuation_with_continuation_keeps_primary() {
    // When a render buffer mirrors a model buffer cell-by-cell, the
    // primary wide cell is written first and the continuation marker is
    // then written into the next column — which already holds the
    // continuation produced by the previous set(). That second write must
    // not blank the wide primary we just placed.
    let mut buf = Buffer::new(10, 1);
    buf.set((3, 0), &Cell::new("中", 2));
    // Now write a continuation into col 4 (where one already lives).
    let cont = Cell::CONTINUATION;
    buf.set((4, 0), &cont);
    assert_eq!(buf.cell(Position::new(3, 0)).unwrap().content(), "中");
    assert_eq!(buf.cell(Position::new(3, 0)).unwrap().width(), 2);
    assert!(buf.cell(Position::new(4, 0)).unwrap().is_continuation());
}

#[test]
fn test_resize() {
    let mut buf = Buffer::new(10, 5);
    buf.set((0, 0), &Cell::new("X", 1));
    buf.resize(20, 10);
    assert_eq!(buf.width(), 20);
    assert_eq!(buf.height(), 10);
    assert_eq!(buf.cell(Position::new(0, 0)).unwrap().content(), "X");
}

fn fill_with_marker(buf: &mut Buffer) {
    for y in 0..buf.height() {
        for x in 0..buf.width() {
            buf.set((x, y), &Cell::new(format!("{x},{y}"), 1));
        }
    }
}

fn assert_marker(buf: &Buffer, x: u16, y: u16) {
    assert_eq!(
        buf.cell(Position::new(x, y)).unwrap().content(),
        format!("{x},{y}"),
        "cell ({x},{y}) lost its marker"
    );
}

#[test]
fn resize_grow_width_grow_height_preserves_topleft_blanks_rest() {
    let mut buf = Buffer::new(4, 3);
    fill_with_marker(&mut buf);
    buf.resize(7, 5);
    assert_eq!((buf.width(), buf.height()), (7, 5));
    for y in 0..3 {
        for x in 0..4 {
            assert_marker(&buf, x, y);
        }
    }
    for x in 4..7 {
        assert!(buf.cell(Position::new(x, 0)).unwrap().is_blank());
    }
    for x in 0..7 {
        assert!(buf.cell(Position::new(x, 4)).unwrap().is_blank());
    }
}

#[test]
fn resize_shrink_width_grow_height_compacts_rows_and_blanks_new_rows() {
    let mut buf = Buffer::new(6, 2);
    fill_with_marker(&mut buf);
    buf.resize(3, 4);
    assert_eq!((buf.width(), buf.height()), (3, 4));
    for y in 0..2 {
        for x in 0..3 {
            assert_marker(&buf, x, y);
        }
    }
    for y in 2..4 {
        for x in 0..3 {
            assert!(buf.cell(Position::new(x, y)).unwrap().is_blank());
        }
    }
}

#[test]
fn resize_grow_width_shrink_height_pads_rows_and_drops_bottom() {
    let mut buf = Buffer::new(3, 4);
    fill_with_marker(&mut buf);
    buf.resize(6, 2);
    assert_eq!((buf.width(), buf.height()), (6, 2));
    for y in 0..2 {
        for x in 0..3 {
            assert_marker(&buf, x, y);
        }
        for x in 3..6 {
            assert!(buf.cell(Position::new(x, y)).unwrap().is_blank());
        }
    }
}

#[test]
fn resize_shrink_both_drops_right_and_bottom() {
    let mut buf = Buffer::new(5, 5);
    fill_with_marker(&mut buf);
    buf.resize(2, 2);
    assert_eq!((buf.width(), buf.height()), (2, 2));
    for y in 0..2 {
        for x in 0..2 {
            assert_marker(&buf, x, y);
        }
    }
}

#[test]
fn resize_same_width_height_only() {
    let mut buf = Buffer::new(4, 3);
    fill_with_marker(&mut buf);
    buf.resize(4, 5);
    for y in 0..3 {
        for x in 0..4 {
            assert_marker(&buf, x, y);
        }
    }
    for y in 3..5 {
        for x in 0..4 {
            assert!(buf.cell(Position::new(x, y)).unwrap().is_blank());
        }
    }

    buf.resize(4, 2);
    assert_eq!(buf.height(), 2);
    for y in 0..2 {
        for x in 0..4 {
            assert_marker(&buf, x, y);
        }
    }
}

#[test]
fn test_write_string() {
    let mut buf = TextBuffer::new(20, 1).with_width_mode(WidthMode::Grapheme);
    let p =
        Painter::new(&mut buf).set_str_wrap((0, 0), "Hello", WrapMode::Truncate, Style::default());
    assert_eq!(p, Position::new(5, 0));
    assert_eq!(buf.cell(Position::new(0, 0)).unwrap().content(), "H");
    assert_eq!(buf.cell(Position::new(4, 0)).unwrap().content(), "o");
}

#[test]
fn test_view() {
    let mut buf = RenderBuffer::new(20, 10);
    {
        let mut v = View::new(&mut buf, (5, 2, 10, 5));
        v.set_cell(Position::new(5, 2), &Cell::new("W", 1));
        assert_eq!(v.cell(Position::new(5, 2)).unwrap().content(), "W");
    }
    assert_eq!(buf.cell(Position::new(5, 2)).unwrap().content(), "W");
}

#[test]
fn write_string_wc_mode_attaches_combining_marks_to_base() {
    // 'e' + U+0301 (combining acute) in Wc mode: the combining mark
    // has width 0 and must attach to the previous cell rather than
    // overwrite it.
    let mut buf = TextBuffer::new(10, 1);
    let p = Painter::new(&mut buf).set_str_wrap(
        (0, 0),
        "e\u{0301}f",
        WrapMode::Truncate,
        Style::default(),
    );
    assert_eq!(p, Position::new(2, 0));
    assert_eq!(
        buf.cell(Position::new(0, 0)).unwrap().content(),
        "e\u{0301}"
    );
    assert_eq!(buf.cell(Position::new(1, 0)).unwrap().content(), "f");
}

#[test]
fn write_string_wc_mode_skips_leading_combining_mark() {
    // No base character to attach to — the combining mark is dropped
    // rather than corrupting an unrelated cell.
    let mut buf = TextBuffer::new(10, 1);
    let p = Painter::new(&mut buf).set_str_wrap(
        (3, 0),
        "\u{0301}a",
        WrapMode::Truncate,
        Style::default(),
    );
    assert_eq!(p, Position::new(4, 0));
    assert_eq!(buf.cell(Position::new(3, 0)).unwrap().content(), "a");
}

#[test]
fn write_string_truncates_at_right_edge() {
    let mut buf = TextBuffer::new(5, 1);
    let p = Painter::new(&mut buf).set_str_wrap(
        (0, 0),
        "Hello, World!",
        WrapMode::Truncate,
        Style::default(),
    );
    assert_eq!(p, Position::new(5, 0));
    assert_eq!(buf.cell(Position::new(4, 0)).unwrap().content(), "o");
}

#[test]
fn write_string_wraps_to_next_row() {
    let mut buf = TextBuffer::new(5, 3);
    let p =
        Painter::new(&mut buf).set_str_wrap((0, 0), "abcdefghij", WrapMode::Wrap, Style::default());
    assert_eq!(p, Position::new(5, 1));
    assert_eq!(buf.cell(Position::new(4, 0)).unwrap().content(), "e");
    assert_eq!(buf.cell(Position::new(0, 1)).unwrap().content(), "f");
    assert_eq!(buf.cell(Position::new(4, 1)).unwrap().content(), "j");
}

#[test]
fn write_string_wrap_stops_at_bottom() {
    let mut buf = TextBuffer::new(3, 2);
    let p =
        Painter::new(&mut buf).set_str_wrap((0, 0), "abcdefghi", WrapMode::Wrap, Style::default());
    // Two full rows consumed, then we run out.
    assert_eq!(p, Position::new(0, 2));
    assert_eq!(buf.cell(Position::new(2, 1)).unwrap().content(), "f");
}

#[test]
fn write_string_wrap_inside_view() {
    // View with non-zero origin: wrap must respect bounds, not
    // wrap to the underlying buffer's left edge.
    let mut tb = TextBuffer::new(20, 5);
    let p = Painter::new(&mut tb).set_str_rect_wrap(
        (10, 1, 4, 2),
        "abcdefgh",
        WrapMode::Wrap,
        Style::default(),
    );
    assert_eq!(p, Position::new(14, 2));
    assert_eq!(tb.cell(Position::new(10, 1)).unwrap().content(), "a");
    assert_eq!(tb.cell(Position::new(13, 1)).unwrap().content(), "d");
    assert_eq!(tb.cell(Position::new(10, 2)).unwrap().content(), "e");
    assert_eq!(tb.cell(Position::new(13, 2)).unwrap().content(), "h");
}

#[test]
fn write_string_with_link() {
    let mut buf = TextBuffer::new(10, 1);
    Painter::new(&mut buf).set_str_wrap(
        (0, 0),
        "hi",
        WrapMode::Truncate,
        Style::default().link("https://example.com", ""),
    );
    assert_eq!(
        link_of(&buf.cell(Position::new(0, 0)).unwrap().style),
        Some(("https://example.com", ""))
    );
    assert_eq!(
        link_of(&buf.cell(Position::new(1, 0)).unwrap().style),
        Some(("https://example.com", ""))
    );
}

#[test]
fn a_cell_too_wide_for_the_row_leaves_no_continuation_behind() {
    // `set` decides a cell does not fit by looking at the columns the row
    // has left, and replaces it with a blank. The continuations it would
    // have owned must never reach the grid: a continuation with no primary
    // to its left is a column no cell accounts for, and every later column
    // on the row reads one place off.
    let mut buf = Buffer::new(5, 1);
    buf.set(
        Position::new(0, 0),
        &Cell::new("\u{1f468}\u{200d}\u{1f469}", 8),
    );

    assert!(buf.cell(Position::new(0, 0)).unwrap().is_blank());
    for x in 0..5 {
        assert!(
            !buf.cell(Position::new(x, 0)).unwrap().is_continuation(),
            "column {x} holds a continuation with no primary that owns it"
        );
    }
}

#[test]
fn a_wide_fill_spans_a_row_as_wide_as_the_address_space() {
    // The stepped fill walks a row by the fill's own width. On a row this
    // wide the column reaches far enough that adding the step to it leaves
    // a `u16` behind, so the walk has to ask whether the next cell fits
    // without computing where it would end.
    let mut buf = Buffer::new(u16::MAX, 1);
    buf.fill_rect(
        Rect::new(0, 0, u16::MAX, 1),
        &Cell::new("\u{1f468}\u{200d}\u{1f469}", 255),
    );

    assert!(buf.cell(Position::new(0, 0)).unwrap().is_wide());
}

#[test]
fn a_fill_inside_a_wide_cell_leaves_no_orphan_continuation() {
    // A cell may own more than two columns, so a fill can land wholly
    // inside one: it starts after the primary and stops before the last
    // column the primary holds. The left-edge pass blanks the primary, and
    // the right-edge pass walks back only as far as the fill's own left
    // edge, so it never reaches a primary sitting before that. The columns
    // past the fill would keep pointing at an owner that no longer exists.
    let fam = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
    let mut buf = Buffer::new(12, 1);
    buf.set(Position::new(0, 0), &Cell::new(fam, 8));
    buf.fill_rect(Rect::new(2, 0, 2, 1), &Cell::new("x", 1));

    for x in 0..2 {
        assert!(buf.cell(Position::new(x, 0)).unwrap().is_blank());
    }
    for x in 2..4 {
        assert_eq!(buf.cell(Position::new(x, 0)).unwrap().content(), "x");
    }
    for x in 4..8 {
        let c = buf.cell(Position::new(x, 0)).unwrap();
        assert!(
            !c.is_continuation(),
            "column {x} still continues a cell that was blanked: {c:?}"
        );
    }
}

#[test]
fn no_fill_of_any_width_leaves_a_continuation_without_an_owner() {
    // Every fill that overlaps a cell has to leave the row well formed:
    // each continuation still sits inside the footprint of a primary to
    // its left. Two columns can never produce the gap above, because a
    // fill strictly inside a two-column cell is empty.
    for w in 2u8..=6 {
        for lo in 0..w as u16 {
            for hi in (lo + 1)..=(w as u16) {
                let mut buf = Buffer::new(12, 1);
                buf.set(Position::new(0, 0), &Cell::new("W", w));
                buf.fill_rect(Rect::new(lo, 0, hi - lo, 1), &Cell::new("x", 1));

                let mut owner_reaches = 0u16;
                for x in 0..12u16 {
                    let c = buf.cell(Position::new(x, 0)).unwrap();
                    if c.is_continuation() {
                        assert!(
                            x < owner_reaches,
                            "w={w} fill=[{lo},{hi}): column {x} has no owner"
                        );
                    } else {
                        owner_reaches = x + c.width() as u16;
                    }
                }
            }
        }
    }
}

#[test]
fn a_cell_that_stores_nothing_still_reserves_the_columns_it_claims() {
    // Width is what decides a cell's footprint, so a cell holding no
    // content owns the columns it names just as a grapheme would. Were the
    // continuations left off, the next write would land inside the claim
    // and every column after it would read one place off.
    let mut buf = Buffer::new(6, 1);
    buf.set(Position::new(0, 0), &Cell::new("", 3));

    for x in 1..3 {
        assert!(
            buf.cell(Position::new(x, 0)).unwrap().is_continuation(),
            "column {x} is inside the claim but no cell reserved it"
        );
    }

    // Writing into the claim breaks it, so the whole cell gives way to
    // blanks. Had the columns never been reserved, this write would have
    // landed inside a cell that still claimed them, and every column after
    // it would read one place off.
    buf.set(Position::new(1, 0), &Cell::new("B", 1));

    assert_eq!(buf.cell(Position::new(1, 0)).unwrap().content(), "B");
    for x in [0, 2] {
        assert!(
            buf.cell(Position::new(x, 0)).unwrap().is_blank(),
            "column {x} kept part of a cell that is gone"
        );
    }
}
