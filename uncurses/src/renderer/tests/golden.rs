use super::*;
use crate::cell::Cell;
use crate::color::Color;
use crate::renderer::RenderBuffer;
use crate::style::Style;

fn renderer() -> Renderer {
    renderer_with(Optimizations::none())
}

/// `TABS` and `BS` come from the host's line discipline, granted by
/// `Screen::init`, never by a `$TERM` baseline. Tests that exercise
/// either one ask for it here and pin the fallback separately.
fn renderer_with_line_discipline() -> Renderer {
    renderer_with(Optimizations::none().with_tabs(true).with_bs(true))
}

fn renderer_with(opts: Optimizations) -> Renderer {
    let mut renderer = Renderer::new();
    renderer.set_optimizations(opts);
    renderer
}

fn render_to_vec(renderer: &mut Renderer, buf: &mut RenderBuffer) -> Vec<u8> {
    let mut out = Vec::new();
    renderer.render(&mut out, buf).unwrap();
    out
}

fn assert_golden(actual: Vec<u8>, expected: &[u8]) {
    assert_eq!(
        actual,
        expected,
        "actual bytes: {:?}\nactual text: {}",
        String::from_utf8_lossy(&actual),
        String::from_utf8_lossy(&actual),
    );
}

fn set_text(buf: &mut RenderBuffer, y: u16, text: &str) {
    for (x, ch) in text.chars().enumerate() {
        buf.set_cell((x as u16, y), &Cell::new(ch.to_string(), 1));
    }
}

fn fill_distinct_rows(buf: &mut RenderBuffer) {
    for y in 0..buf.height() {
        let text = format!("row {y:02}");
        set_text(buf, y, &text);
    }
}

#[test]
fn golden_empty_frame_80x24() {
    let mut renderer = renderer();
    let mut buf = RenderBuffer::new(80, 24);

    let actual = render_to_vec(&mut renderer, &mut buf);

    assert_golden(actual, b"");
}

#[test]
fn golden_single_cell_change_at_origin() {
    let mut renderer = renderer();
    let mut buf = RenderBuffer::new(80, 24);
    let _ = render_to_vec(&mut renderer, &mut buf);
    buf.set_cell((0, 0), &Cell::new("X", 1));

    let actual = render_to_vec(&mut renderer, &mut buf);

    assert_golden(
        actual,
        b"\rX\r\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n",
    );
}

#[test]
fn golden_single_cell_change_at_middle() {
    let mut renderer = renderer();
    let mut buf = RenderBuffer::new(80, 24);
    let _ = render_to_vec(&mut renderer, &mut buf);
    buf.set_cell((40, 12), &Cell::new("X", 1));

    let actual = render_to_vec(&mut renderer, &mut buf);

    assert_golden(
        actual,
        b"\r\n\n\n\n\n\n\n\n\n\n\n\n\x1b[40CX\r\n\n\n\n\n\n\n\n\n\n\n",
    );
}

fn scroll_up_by_1_full_width(renderer: &mut Renderer) -> Vec<u8> {
    let mut first = RenderBuffer::new(80, 24);
    fill_distinct_rows(&mut first);
    let _ = render_to_vec(renderer, &mut first);

    let mut second = RenderBuffer::new(80, 24);
    for y in 0..23 {
        let text = format!("row {:02}", y + 1);
        set_text(&mut second, y, &text);
    }

    render_to_vec(renderer, &mut second)
}

#[test]
fn golden_scroll_up_by_1_full_width() {
    let actual = scroll_up_by_1_full_width(&mut renderer_with_line_discipline());
    assert_golden(
        actual,
        b"\x1b[J\x1b[23A\x1b[5C1\n\x082\n\x083\n\x084\n\x085\n\x086\n\x087\n\x088\n\x089\n\x08\x0810\n\x081\n\x082\n\x083\n\x084\n\x085\n\x086\n\x087\n\x088\n\x089\n\x08\x0820\n\x081\n\x082\n\x083",
    );
}

/// Same frame with the host withholding `BS`: every one-column step back
/// onto the changed digit has to become an escape sequence instead.
#[test]
fn golden_scroll_up_by_1_full_width_without_line_discipline() {
    let actual = scroll_up_by_1_full_width(&mut renderer());
    assert_golden(actual, b"\x1b[J\x1b[23A\x1b[5C1\n\x1b[D2\n\x1b[D3\n\x1b[D4\n\x1b[D5\n\x1b[D6\n\x1b[D7\n\x1b[D8\n\x1b[D9\n\x1b[2D10\n\x1b[D1\n\x1b[D2\n\x1b[D3\n\x1b[D4\n\x1b[D5\n\x1b[D6\n\x1b[D7\n\x1b[D8\n\x1b[D9\n\x1b[2D20\n\x1b[D1\n\x1b[D2\n\x1b[D3");
}

#[test]
fn golden_force_clear_frame() {
    let mut renderer = renderer();
    let mut buf = RenderBuffer::new(80, 24);
    let _ = render_to_vec(&mut renderer, &mut buf);
    renderer.request_clear();

    let actual = render_to_vec(&mut renderer, &mut buf);

    assert_golden(
        actual,
        b"\r\x1b[J\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n",
    );
}

#[test]
fn golden_fullscreen_mode_first_frame() {
    let mut renderer = renderer();
    renderer.set_fullscreen(true);
    let mut buf = RenderBuffer::new(80, 24);
    set_text(&mut buf, 0, "top");
    set_text(&mut buf, 12, "middle");
    set_text(&mut buf, 23, "bottom");

    let actual = render_to_vec(&mut renderer, &mut buf);

    assert_golden(actual, b"top\r\x1b[12Bmiddle\r\x1b[11Bbottom");
}

#[test]
fn golden_relative_cursor_mode() {
    let mut renderer = renderer();
    renderer.set_relative_cursor(true);
    let mut buf = RenderBuffer::new(80, 24);
    let _ = render_to_vec(&mut renderer, &mut buf);
    buf.set_cell((0, 5), &Cell::new("X", 1));

    let actual = render_to_vec(&mut renderer, &mut buf);

    assert_golden(
        actual,
        b"\r\n\n\n\n\nX\r\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n",
    );
}

#[test]
fn golden_a_cell_that_stores_nothing_is_blanked_across_its_whole_width() {
    // The diff strides by the cell's width, so the blank standing in for an
    // empty cell has to cover every column that stride passes over. A
    // single space would put the cursor two columns left of the model and
    // paint the rest of the row there.
    let mut renderer = renderer();
    let mut buf = RenderBuffer::new(8, 1);
    buf.set_cell((0, 0), &Cell::new("", 3));
    buf.set_cell((3, 0), &Cell::new("X", 1));
    buf.set_cell((4, 0), &Cell::new("Y", 1));

    let out = render_to_vec(&mut renderer, &mut buf);
    assert_golden(out, b"\r   XY\r");
}

/// A cluster whose code points sum to more columns than a ligating
/// terminal draws. Under [`WidthMode::Wc`] this measures eight columns,
/// and a terminal that draws the family as one glyph advances over two.
const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F466}\u{200D}\u{1F466}";

/// Write `FAMILY` at `x` on row 0, followed by `tail`.
fn set_family_row(buf: &mut RenderBuffer, x: u16, tail: &str) {
    buf.set_cell((x, 0), &Cell::new(FAMILY, 8));
    for c in x + 1..x + 8 {
        buf.set_cell((c, 0), &Cell::new("", 0));
    }
    for (i, ch) in tail.chars().enumerate() {
        buf.set_cell((x + 8 + i as u16, 0), &Cell::new(ch.to_string(), 1));
    }
}

#[test]
fn golden_a_ligatable_cluster_repaints_the_row_from_the_cluster() {
    let mut renderer = renderer_with(Optimizations::none().with_ech(true));
    let mut buf = RenderBuffer::new(20, 2);
    let _ = render_to_vec(&mut renderer, &mut buf);

    set_family_row(&mut buf, 2, "Z");

    assert_golden(
        render_to_vec(&mut renderer, &mut buf),
        "\r  \x1b[K\x1b[?7l\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F466}\u{200D}\u{1F466}Z\x1b[?7h\r\n".as_bytes(),
    );
}

#[test]
fn golden_a_change_after_a_ligatable_cluster_repaints_from_the_cluster() {
    let mut renderer = renderer_with(Optimizations::none().with_ech(true));
    let mut buf = RenderBuffer::new(20, 2);
    let _ = render_to_vec(&mut renderer, &mut buf);
    set_family_row(&mut buf, 2, "Z");
    let _ = render_to_vec(&mut renderer, &mut buf);

    // The cluster itself does not change. A terminal that ligated it put
    // this `Y` at column four, so the diff's column ten is wrong and the
    // row has to be laid out again from the cluster.
    buf.set_cell((10, 0), &Cell::new("Y", 1));

    assert_golden(
        render_to_vec(&mut renderer, &mut buf),
        "\x1b[A  \x1b[K\x1b[?7l\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F466}\u{200D}\u{1F466}Y\x1b[?7h\r".as_bytes(),
    );
}

#[test]
fn golden_a_change_before_a_ligatable_cluster_keeps_the_diff() {
    let mut renderer = renderer_with(Optimizations::none().with_ech(true));
    let mut buf = RenderBuffer::new(20, 2);
    let _ = render_to_vec(&mut renderer, &mut buf);
    set_family_row(&mut buf, 2, "Z");
    let _ = render_to_vec(&mut renderer, &mut buf);

    // Every column left of the cluster sits where the diff believes it
    // does, so the ordinary diff still holds there.
    buf.set_cell((0, 0), &Cell::new("A", 1));

    assert_golden(render_to_vec(&mut renderer, &mut buf), b"\x1b[AA ");
}

#[test]
fn golden_removing_a_ligatable_cluster_repaints_from_where_it_stood() {
    let mut renderer = renderer_with(Optimizations::none().with_ech(true));
    let mut buf = RenderBuffer::new(20, 2);
    let _ = render_to_vec(&mut renderer, &mut buf);
    set_family_row(&mut buf, 2, "Z");
    let _ = render_to_vec(&mut renderer, &mut buf);

    // The cluster is gone from the new row, but the old row still had it
    // when the terminal drew `Z`, so `Z`'s column is unknown too.
    for x in 2..10u16 {
        buf.set_cell((x, 0), &Cell::BLANK);
    }

    assert_golden(
        render_to_vec(&mut renderer, &mut buf),
        b"\x1b[A  \x1b[K\x1b[?7l        Z\x1b[?7h\r",
    );
}

#[test]
fn golden_a_shrinking_tail_leaves_no_residue() {
    let mut renderer = renderer_with(Optimizations::none().with_ech(true));
    let mut buf = RenderBuffer::new(20, 2);
    let _ = render_to_vec(&mut renderer, &mut buf);
    set_family_row(&mut buf, 2, "HELLO");
    let _ = render_to_vec(&mut renderer, &mut buf);

    for x in 11..15u16 {
        buf.set_cell((x, 0), &Cell::BLANK);
    }

    assert_golden(
        render_to_vec(&mut renderer, &mut buf),
        "\x1b[A  \x1b[K\x1b[?7l\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F466}\u{200D}\u{1F466}H\x1b[?7h\r".as_bytes(),
    );
}

#[test]
fn golden_an_unclearable_right_edge_is_written_out() {
    let mut renderer = renderer_with(Optimizations::none().with_ech(true));
    let mut buf = RenderBuffer::new(12, 2);
    let _ = render_to_vec(&mut renderer, &mut buf);

    // An underlined blank is not what an erase leaves behind, so the tail
    // has to be written out to the right edge instead of erased.
    let underlined = Cell::new(" ", 1).style(Style::default().underline());
    for x in 0..12u16 {
        buf.set_cell((x, 0), &underlined.clone());
    }
    set_family_row(&mut buf, 2, "");

    assert_golden(
        render_to_vec(&mut renderer, &mut buf),
        "\r\x1b[4m  \x1b[?7l\x1b[m\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F466}\u{200D}\u{1F466}\x1b[4m  \x1b[?7h\r\n\x1b[m".as_bytes(),
    );
}

#[test]
fn golden_a_colored_tail_is_erased_with_bce() {
    let mut renderer = renderer_with(Optimizations::none().with_ech(true).with_bce(true));
    let mut buf = RenderBuffer::new(14, 2);
    let _ = render_to_vec(&mut renderer, &mut buf);

    let red = Style::default().bg(Color::Red);
    for x in 0..14u16 {
        buf.set_cell((x, 0), &Cell::new(" ", 1).style(red.clone()));
    }
    buf.set_cell((2, 0), &Cell::new(FAMILY, 8).style(red.clone()));
    for c in 3..10u16 {
        buf.set_cell((c, 0), &Cell::new("", 0).style(red.clone()));
    }

    assert_golden(
        render_to_vec(&mut renderer, &mut buf),
        "\r\x1b[41m  \x1b[K\x1b[?7l\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F466}\u{200D}\u{1F466}\x1b[?7h\r\x1b[m\n".as_bytes(),
    );
}

#[test]
fn golden_a_single_code_point_wide_cell_needs_no_repaint() {
    let mut renderer = renderer_with(Optimizations::none().with_ech(true));
    let mut buf = RenderBuffer::new(20, 2);
    let _ = render_to_vec(&mut renderer, &mut buf);

    // One code point is measured the way the terminal draws it, whatever
    // the terminal's shaping does with its neighbors.
    buf.set_cell((2, 0), &Cell::new("\u{4E16}", 2));
    buf.set_cell((3, 0), &Cell::new("", 0));
    buf.set_cell((4, 0), &Cell::new("Z", 1));

    assert_golden(
        render_to_vec(&mut renderer, &mut buf),
        "\r  \u{4E16}Z\r\n".as_bytes(),
    );
}

#[test]
fn golden_grapheme_widths_need_no_repaint() {
    let mut renderer = renderer_with(Optimizations::none().with_ech(true));
    renderer.set_width_mode(crate::text::WidthMode::Grapheme);
    let mut buf = RenderBuffer::new(20, 2);
    let _ = render_to_vec(&mut renderer, &mut buf);

    // Measured whole, the cluster claims the two columns the terminal
    // draws, so the diff's columns hold and it runs as usual.
    buf.set_cell((2, 0), &Cell::new(FAMILY, 2));
    buf.set_cell((3, 0), &Cell::new("", 0));
    buf.set_cell((4, 0), &Cell::new("Z", 1));

    assert_golden(
        render_to_vec(&mut renderer, &mut buf),
        "\r  \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F466}\u{200D}\u{1F466}Z\r\n".as_bytes(),
    );
}

/// A blank run long enough to earn ECH sits between a ligatable cluster
/// and a trailing glyph. ECH itself is safe, since it erases forward
/// from wherever the cursor really is, but the move that skips past the
/// run is not: with `TABS` granted, the cheapest way to cross 32 columns
/// is four tabs, and a tab lands on the terminal's stop, not on the
/// model's. The terminal counts columns from where it drew the cluster,
/// so tabbing from there arrives somewhere the model never names. The
/// repaint asks for a forward move by the run's own length instead,
/// which measures the same in both frames.
#[test]
fn golden_a_blank_run_after_a_ligatable_cluster_is_skipped_by_a_relative_move() {
    let mut renderer = renderer_with(
        Optimizations::none()
            .with_ech(true)
            .with_tabs(true)
            .with_bs(true),
    );
    renderer.fullscreen = true;
    renderer.set_width_mode(crate::text::WidthMode::Wc);

    let mut buf = RenderBuffer::new(70, 2);
    set_family_row(&mut buf, 0, "");
    buf.set_cell((40, 0), &Cell::new("Z", 1));

    assert_golden(
        render_to_vec(&mut renderer, &mut buf),
        "\x1b[K\x1b[?7l\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F466}\u{200D}\u{1F466}\x1b[32X\x1b[32CZ\x1b[?7h\r".as_bytes(),
    );
}

/// A heart followed by an emoji variation selector sums to one column,
/// and a terminal that honors the selector draws it in two. The sum can
/// be short as well as long, so a cluster is uncertain whenever its
/// shape leaves room for the terminal to disagree, not only when the
/// sum came out wide.
#[test]
fn golden_a_variation_selector_makes_its_row_uncertain() {
    let mut renderer = renderer_with(Optimizations::none().with_ech(true));
    renderer.set_width_mode(crate::text::WidthMode::Wc);
    let mut buf = RenderBuffer::new(20, 2);
    let _ = render_to_vec(&mut renderer, &mut buf);

    buf.set_cell((0, 0), &Cell::new("a", 1));
    buf.set_cell((1, 0), &Cell::new("\u{2764}\u{FE0F}", 1));
    buf.set_cell((2, 0), &Cell::new("Z", 1));
    let _ = render_to_vec(&mut renderer, &mut buf);

    // Column two is where the diff believes `Z` is; a terminal giving the
    // heart two columns put it at three. The row is laid out again from
    // the heart rather than addressed by column.
    buf.set_cell((2, 0), &Cell::new("Y", 1));

    assert_golden(
        render_to_vec(&mut renderer, &mut buf),
        "\x1b[Aa\x1b[K\x1b[?7l\u{2764}\u{FE0F}Y\x1b[?7h\r".as_bytes(),
    );
}

/// A base with a combining mark after it is not uncertain. No terminal
/// gives the mark a column of its own, so the sum is the base's width
/// and both measurements agree. Decomposed text must keep the ordinary
/// diff rather than repaint its row on every change.
#[test]
fn golden_a_combining_mark_keeps_the_diff() {
    let mut renderer = renderer_with(Optimizations::none().with_ech(true));
    renderer.set_width_mode(crate::text::WidthMode::Wc);
    let mut buf = RenderBuffer::new(20, 2);
    let _ = render_to_vec(&mut renderer, &mut buf);

    buf.set_cell((0, 0), &Cell::new("e\u{0301}", 1));
    buf.set_cell((1, 0), &Cell::new("Z", 1));
    let _ = render_to_vec(&mut renderer, &mut buf);

    buf.set_cell((1, 0), &Cell::new("Y", 1));

    assert_golden(
        render_to_vec(&mut renderer, &mut buf),
        "\x1b[A\x1b[CY".as_bytes(),
    );
}
