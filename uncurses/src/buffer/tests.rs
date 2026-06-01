use super::*;
use crate::renderer::RenderBuffer;
use crate::style::Style;
use crate::text::{Painter, WidthMode, WrapMode};

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
fn test_overwrite_continuation_with_continuation_keeps_primary() {
    // When a render buffer mirrors a model buffer cell-by-cell, the
    // primary wide cell is written first and the continuation marker is
    // then written into the next column — which already holds the
    // continuation produced by the previous set(). That second write must
    // not blank the wide primary we just placed.
    let mut buf = Buffer::new(10, 1);
    buf.set((3, 0), &Cell::new("中", 2));
    // Now write a continuation into col 4 (where one already lives).
    let cont = Cell::new("", 0);
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
    let mut buf = Buffer::new(20, 1);
    let p = Painter::new(&mut buf)
        .with_mode(WidthMode::Grapheme)
        .set_str_with((0, 0), "Hello", WrapMode::Truncate, Style::EMPTY);
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
    let mut buf = Buffer::new(10, 1);
    let p =
        Painter::new(&mut buf).set_str_with((0, 0), "e\u{0301}f", WrapMode::Truncate, Style::EMPTY);
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
    let mut buf = Buffer::new(10, 1);
    let p =
        Painter::new(&mut buf).set_str_with((3, 0), "\u{0301}a", WrapMode::Truncate, Style::EMPTY);
    assert_eq!(p, Position::new(4, 0));
    assert_eq!(buf.cell(Position::new(3, 0)).unwrap().content(), "a");
}

#[test]
fn write_string_truncates_at_right_edge() {
    let mut buf = Buffer::new(5, 1);
    let p = Painter::new(&mut buf).set_str_with(
        (0, 0),
        "Hello, World!",
        WrapMode::Truncate,
        Style::EMPTY,
    );
    assert_eq!(p, Position::new(5, 0));
    assert_eq!(buf.cell(Position::new(4, 0)).unwrap().content(), "o");
}

#[test]
fn write_string_wraps_to_next_row() {
    let mut buf = Buffer::new(5, 3);
    let p = Painter::new(&mut buf).set_str_with((0, 0), "abcdefghij", WrapMode::Wrap, Style::EMPTY);
    assert_eq!(p, Position::new(5, 1));
    assert_eq!(buf.cell(Position::new(4, 0)).unwrap().content(), "e");
    assert_eq!(buf.cell(Position::new(0, 1)).unwrap().content(), "f");
    assert_eq!(buf.cell(Position::new(4, 1)).unwrap().content(), "j");
}

#[test]
fn write_string_wrap_stops_at_bottom() {
    let mut buf = Buffer::new(3, 2);
    let p = Painter::new(&mut buf).set_str_with((0, 0), "abcdefghi", WrapMode::Wrap, Style::EMPTY);
    // Two full rows consumed, then we run out.
    assert_eq!(p, Position::new(0, 2));
    assert_eq!(buf.cell(Position::new(2, 1)).unwrap().content(), "f");
}

#[test]
fn write_string_wrap_inside_view() {
    // View with non-zero origin: wrap must respect bounds, not
    // wrap to the underlying buffer's left edge.
    let mut rb = RenderBuffer::new(20, 5);
    let p = {
        let mut v = View::new(&mut rb, (10, 1, 4, 2));
        Painter::new(&mut v).set_str_with((10, 1), "abcdefgh", WrapMode::Wrap, Style::EMPTY)
    };
    assert_eq!(p, Position::new(14, 2));
    assert_eq!(rb.cell(Position::new(10, 1)).unwrap().content(), "a");
    assert_eq!(rb.cell(Position::new(13, 1)).unwrap().content(), "d");
    assert_eq!(rb.cell(Position::new(10, 2)).unwrap().content(), "e");
    assert_eq!(rb.cell(Position::new(13, 2)).unwrap().content(), "h");
}

#[test]
fn write_string_with_link() {
    let mut buf = Buffer::new(10, 1);
    Painter::new(&mut buf).set_str_with(
        (0, 0),
        "hi",
        WrapMode::Truncate,
        Style::EMPTY.with_link("https://example.com", ""),
    );
    assert_eq!(
        buf.cell(Position::new(0, 0)).unwrap().style().link(),
        Some(("https://example.com", ""))
    );
    assert_eq!(
        buf.cell(Position::new(1, 0)).unwrap().style().link(),
        Some(("https://example.com", ""))
    );
}
