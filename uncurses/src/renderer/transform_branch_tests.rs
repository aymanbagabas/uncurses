use super::{Optimizations, RenderBuffer, Renderer};
use crate::cell::Cell;
use crate::color::{BasicColor, Color};
use crate::style::Style;

fn opts_with(mut f: impl FnMut(&mut Optimizations)) -> Optimizations {
    let mut opts = Optimizations::none();
    f(&mut opts);
    opts
}

fn renderer(width: u16, height: u16, opts: Optimizations) -> Renderer {
    let mut renderer = Renderer::new();
    renderer.set_optimizations(opts);
    renderer.set_relative_cursor(false);
    renderer.last_width = width;
    renderer.last_height = height;
    renderer.cur.x_unknown = false;
    renderer.cur.y_unknown = false;
    renderer
}

fn buffer_with_text(width: u16, height: u16, y: u16, text: &str) -> RenderBuffer {
    let mut buf = RenderBuffer::new(width, height);
    for (x, ch) in text.chars().enumerate() {
        buf.set_cell((x as u16, y), &Cell::narrow(ch.to_string()));
    }
    buf.clear_touched();
    buf
}

fn set_text(buf: &mut RenderBuffer, y: u16, text: &str) {
    for (x, ch) in text.chars().enumerate() {
        buf.set_cell((x as u16, y), &Cell::narrow(ch.to_string()));
    }
}

fn transform_output(renderer: &mut Renderer, new_buf: &RenderBuffer) -> Vec<u8> {
    let mut out = Vec::new();
    renderer
        .transform_line(&mut out, new_buf, 0, 0, new_buf.width().saturating_sub(1))
        .unwrap();
    out
}

#[test]
fn emit_range_plain_when_no_optimizations() {
    let width = 12;
    let mut renderer = renderer(width, 1, Optimizations::none());
    renderer.cur_buf = Some(RenderBuffer::new(width, 1));

    let mut new_buf = RenderBuffer::new(width, 1);
    set_text(&mut new_buf, 0, "abcde");

    let out = transform_output(&mut renderer, &new_buf);
    assert_eq!(out, b"abcde");
    assert!(!out.windows(2).any(|w| w == b"\x1b["));
}

#[test]
fn emit_range_ech_for_blank_run() {
    let width = 24;
    let mut renderer = renderer(width, 1, opts_with(|o| o.insert(Optimizations::ECH)));
    renderer.cur_buf = Some(buffer_with_text(width, 1, 0, "abcdeXXXXXXXXXXXXXXY"));

    let mut new_buf = RenderBuffer::new(width, 1);
    set_text(&mut new_buf, 0, "abcde              Z");

    let out = transform_output(&mut renderer, &new_buf);
    assert!(
        out.windows(b"\x1b[14X".len()).any(|w| w == b"\x1b[14X"),
        "expected ECH 14, got {out:?}"
    );
    assert!(
        !out.windows(b"              ".len())
            .any(|w| w == b"              ")
    );
}

#[test]
fn emit_range_plain_for_ten_blank_run_due_cursor_cost() {
    let width = 24;
    let mut renderer = renderer(width, 1, opts_with(|o| o.insert(Optimizations::ECH)));
    renderer.cur_buf = Some(buffer_with_text(width, 1, 0, "abcdeXXXXXXXXXXpqrstY"));

    let mut new_buf = RenderBuffer::new(width, 1);
    set_text(&mut new_buf, 0, "abcde          pqrstZ");

    let out = transform_output(&mut renderer, &new_buf);
    // The 10-blank middle run is shorter than `\x1b[10X` (ECH 10) so
    // it falls back to plain spaces. The 5-cell `pqrst` matching run
    // is longer than the in-place cursor-move cost, so it is jumped
    // over with a CUF rather than repainted.
    assert_eq!(out, b"\x1b[5C          \x1b[5CZ");
    assert!(!out.windows(b"\x1b[10X".len()).any(|w| w == b"\x1b[10X"));
}

#[test]
fn emit_range_rep_for_repeated_glyph() {
    let width = 12;
    let mut renderer = renderer(width, 1, opts_with(|o| o.insert(Optimizations::REP)));
    renderer.cur_buf = Some(RenderBuffer::new(width, 1));

    let mut new_buf = RenderBuffer::new(width, 1);
    set_text(&mut new_buf, 0, "AAAAAAAA");

    let out = transform_output(&mut renderer, &new_buf);
    assert!(
        out.windows(b"A\x1b[7b".len()).any(|w| w == b"A\x1b[7b"),
        "expected REP for eight As, got {out:?}"
    );
}

#[test]
fn emit_range_rep_skipped_for_non_ascii() {
    let width = 12;
    let mut renderer = renderer(width, 1, opts_with(|o| o.insert(Optimizations::REP)));
    renderer.cur_buf = Some(RenderBuffer::new(width, 1));

    let mut new_buf = RenderBuffer::new(width, 1);
    for x in 0..8u16 {
        new_buf.set_cell((x, 0), &Cell::narrow("é"));
    }

    let out = transform_output(&mut renderer, &new_buf);
    assert!(!out.windows(3).any(|w| w == b"\x1b[" && w[2] != b'm'));
    assert_eq!(String::from_utf8_lossy(&out), "éééééééé");
}

#[test]
fn emit_range_plain_beats_ech_for_short_blank() {
    let width = 8;
    let mut renderer = renderer(width, 1, opts_with(|o| o.insert(Optimizations::ECH)));
    renderer.cur_buf = Some(buffer_with_text(width, 1, 0, "XXcdefgY"));

    let mut new_buf = RenderBuffer::new(width, 1);
    set_text(&mut new_buf, 0, "  cdefgZ");

    let out = transform_output(&mut renderer, &new_buf);
    assert!(out.starts_with(b"  "), "expected plain spaces, got {out:?}");
    assert!(!out.windows(b"\x1b[2X".len()).any(|w| w == b"\x1b[2X"));
}

#[test]
fn emit_range_plain_beats_rep_for_short_repeat() {
    let width = 8;
    let mut renderer = renderer(width, 1, opts_with(|o| o.insert(Optimizations::REP)));
    renderer.cur_buf = Some(RenderBuffer::new(width, 1));

    let mut new_buf = RenderBuffer::new(width, 1);
    set_text(&mut new_buf, 0, "AAZ");

    let out = transform_output(&mut renderer, &new_buf);
    assert_eq!(out, b"AAZ");
    assert!(!out.windows(3).any(|w| w == b"\x1b[b"));
}

#[test]
fn transform_no_change_emits_nothing() {
    let width = 10;
    let mut renderer = renderer(width, 1, Optimizations::none());
    let cur = buffer_with_text(width, 1, 0, "unchanged");
    renderer.cur_buf = Some(cur.clone());

    let out = transform_output(&mut renderer, &cur);
    assert!(out.is_empty(), "expected no bytes, got {out:?}");
}

#[test]
fn transform_el_0_whole_line_clear() {
    let width = 20;
    let mut renderer = renderer(width, 1, opts_with(|o| o.insert(Optimizations::BCE)));
    renderer.cur_buf = Some(buffer_with_text(width, 1, 0, "XXXXXXXXXXXXXXXXXXXX"));

    let new_buf = RenderBuffer::new(width, 1);
    let out = transform_output(&mut renderer, &new_buf);
    assert_eq!(out, b"\x1b[K");
}

#[test]
fn transform_el_0_partial_clear_from_column() {
    let width = 50;
    let mut renderer = renderer(width, 1, opts_with(|o| o.insert(Optimizations::BCE)));
    let mut cur = RenderBuffer::new(width, 1);
    let mut new_buf = RenderBuffer::new(width, 1);
    for x in 0..30u16 {
        cur.set_cell((x, 0), &Cell::narrow("A"));
        new_buf.set_cell((x, 0), &Cell::narrow("A"));
    }
    for x in 30..width {
        cur.set_cell((x, 0), &Cell::narrow("X"));
    }
    cur.clear_touched();
    renderer.cur_buf = Some(cur);

    let out = transform_output(&mut renderer, &new_buf);
    assert_eq!(out, b"\x1b[1;31H\x1b[K");
}

#[test]
fn transform_dch_for_deleted_cell() {
    let width = 12;
    let mut renderer = renderer(width, 1, opts_with(|o| o.insert(Optimizations::DCH)));
    renderer.cur_buf = Some(buffer_with_text(width, 1, 0, "AXBCDEFG"));

    let mut new_buf = RenderBuffer::new(width, 1);
    set_text(&mut new_buf, 0, "ABCDEFG");

    let out = transform_output(&mut renderer, &new_buf);
    assert!(
        out.windows(b"\x1b[P".len()).any(|w| w == b"\x1b[P"),
        "expected DCH 1, got {out:?}"
    );
}

#[test]
fn transform_ich_skipped_when_row_has_skip_cell() {
    // ICH would slide the row's columns right; placeholder cells
    // for an externally-painted region must stay anchored, so the
    // renderer falls back to a plain overwrite.
    let width = 12;
    let mut renderer = renderer(width, 1, opts_with(|o| o.insert(Optimizations::ICH)));
    renderer.cur_buf = Some(buffer_with_text(width, 1, 0, "ABCDEF"));

    let mut new_buf = RenderBuffer::new(width, 1);
    set_text(&mut new_buf, 0, "AXBCDEF");
    new_buf.set_cell((9, 0), &Cell::skip());

    let out = transform_output(&mut renderer, &new_buf);
    assert!(
        !out.windows(b"\x1b[1@".len()).any(|w| w == b"\x1b[1@"),
        "ICH must not be emitted on a row carrying a skip cell, got {out:?}"
    );
}

#[test]
fn transform_dch_skipped_when_row_has_skip_cell() {
    // DCH would slide the row's columns left; placeholder cells
    // for an externally-painted region must stay anchored, so the
    // renderer falls back to a plain overwrite + EL.
    let width = 12;
    let mut renderer = renderer(width, 1, opts_with(|o| o.insert(Optimizations::DCH)));
    let mut cur = buffer_with_text(width, 1, 0, "AXBCDEFG");
    cur.set_cell((10, 0), &Cell::skip());
    cur.clear_touched();
    renderer.cur_buf = Some(cur);

    let mut new_buf = RenderBuffer::new(width, 1);
    set_text(&mut new_buf, 0, "ABCDEFG");
    new_buf.set_cell((10, 0), &Cell::skip());

    let out = transform_output(&mut renderer, &new_buf);
    assert!(
        !out.windows(b"\x1b[1P".len()).any(|w| w == b"\x1b[1P"),
        "DCH must not be emitted on a row carrying a skip cell, got {out:?}"
    );
}

#[test]
fn transform_dch_still_emitted_when_skip_is_left_of_shift() {
    // Placeholder is at column 0; the deletion happens at column
    // 1. The shift only moves cells strictly to the right of the
    // placeholder, so the placeholder's anchor is safe and DCH
    // stays eligible.
    let width = 12;
    let mut renderer = renderer(width, 1, opts_with(|o| o.insert(Optimizations::DCH)));
    let mut cur = RenderBuffer::new(width, 1);
    cur.set_cell((0, 0), &Cell::skip());
    for (i, ch) in "XBCDEFG".chars().enumerate() {
        cur.set_cell(((i + 1) as u16, 0), &Cell::narrow(ch.to_string()));
    }
    cur.clear_touched();
    renderer.cur_buf = Some(cur);

    let mut new_buf = RenderBuffer::new(width, 1);
    new_buf.set_cell((0, 0), &Cell::skip());
    for (i, ch) in "BCDEFG".chars().enumerate() {
        new_buf.set_cell(((i + 1) as u16, 0), &Cell::narrow(ch.to_string()));
    }

    let out = transform_output(&mut renderer, &new_buf);
    assert!(
        out.windows(b"\x1b[P".len()).any(|w| w == b"\x1b[P"),
        "DCH expected past the rightmost skip column, got {out:?}"
    );
}

#[test]
fn transform_step4c_walkback() {
    let width = 10;
    let mut renderer = renderer(width, 1, Optimizations::none());
    renderer.cur_buf = Some(buffer_with_text(width, 1, 0, "Xbcdef"));

    let mut new_buf = RenderBuffer::new(width, 1);
    set_text(&mut new_buf, 0, "Abcdef");

    let out = transform_output(&mut renderer, &new_buf);
    assert_eq!(out, b"Ab");
}

#[test]
fn transform_pen_change_emits_sgr() {
    let width = 10;
    let mut renderer = renderer(width, 1, Optimizations::none());
    renderer.cur_buf = Some(RenderBuffer::new(width, 1));

    let mut new_buf = RenderBuffer::new(width, 1);
    let red = Style::EMPTY.with_fg(Color::Basic(BasicColor::Red));
    new_buf.set_cell((0, 0), &Cell::narrow("R").with_style(red));

    let out = transform_output(&mut renderer, &new_buf);
    assert_eq!(out, b"\x1b[31mR");
}

#[test]
fn transform_osc8_link_open_and_close() {
    let width = 10;
    let mut renderer = renderer(width, 1, Optimizations::none());
    renderer.cur_buf = Some(RenderBuffer::new(width, 1));

    let linked_style = Style::EMPTY.with_link("https://example.test", "");
    let mut linked_buf = RenderBuffer::new(width, 1);
    linked_buf.set_cell((0, 0), &Cell::narrow("L").with_style(linked_style));
    let open = transform_output(&mut renderer, &linked_buf);
    assert!(
        open.windows(b"\x1b]8;;https://example.test\x1b\\L".len())
            .any(|w| w == b"\x1b]8;;https://example.test\x1b\\L"),
        "expected OSC 8 open, got {open:?}"
    );

    let mut plain_buf = RenderBuffer::new(width, 1);
    plain_buf.set_cell((0, 0), &Cell::narrow("P"));
    let close = transform_output(&mut renderer, &plain_buf);
    assert!(
        close
            .windows(b"\x1b]8;;\x1b\\P".len())
            .any(|w| w == b"\x1b]8;;\x1b\\P"),
        "expected OSC 8 close, got {close:?}"
    );
}

#[test]
fn reset_pen_closes_osc8_and_emits_sgr_reset() {
    let mut renderer = renderer(10, 1, Optimizations::none());
    renderer
        .cur
        .set_style(Style::EMPTY.with_link("https://example.test", ""));

    let mut out = Vec::new();
    renderer.reset_pen(&mut out).unwrap();
    assert!(
        out.windows(b"\x1b]8;;\x1b\\".len())
            .any(|w| w == b"\x1b]8;;\x1b\\"),
        "expected OSC 8 close, got {:?}",
        std::str::from_utf8(&out)
    );
    assert!(
        renderer.cur.style().link().is_none(),
        "link tracking should be cleared after reset_pen"
    );
}

#[test]
fn clear_bottom_uses_ed_when_bce() {
    let width = 10;
    let height = 6;
    let mut renderer = renderer(width, height, opts_with(|o| o.insert(Optimizations::BCE)));
    let mut cur = RenderBuffer::new(width, height);
    for y in 1..height {
        cur.set_cell((0, y), &Cell::narrow("X"));
    }
    cur.clear_touched();
    renderer.cur_buf = Some(cur);

    let mut new_buf = RenderBuffer::new(width, height);
    new_buf.set_cell((0, 0), &Cell::narrow("A"));
    let mut out = Vec::new();
    let top = renderer.clear_bottom(&mut out, &new_buf).unwrap();

    assert_eq!(top, 1);
    assert_eq!(out, b"\n\x1b[J");
}

#[test]
fn clear_bottom_without_bce_still_uses_ed_for_default_blank() {
    let width = 10;
    let height = 6;
    let mut renderer = renderer(width, height, Optimizations::none());
    let mut cur = RenderBuffer::new(width, height);
    for y in 1..height {
        cur.set_cell((0, y), &Cell::narrow("X"));
    }
    cur.clear_touched();
    renderer.cur_buf = Some(cur);

    let mut new_buf = RenderBuffer::new(width, height);
    new_buf.set_cell((0, 0), &Cell::narrow("A"));
    let mut out = Vec::new();
    let top = renderer.clear_bottom(&mut out, &new_buf).unwrap();

    assert_eq!(top, 1);
    assert_eq!(out, b"\n\x1b[J");
}

#[test]
fn clear_bottom_with_styled_blank() {
    let width = 8;
    let height = 4;
    let bg = Style::EMPTY.with_bg(Color::Basic(BasicColor::Blue));
    let blank = Cell::BLANK.with_style(bg);
    let mut renderer = renderer(width, height, opts_with(|o| o.insert(Optimizations::BCE)));
    renderer.cur.set_style(blank.style().clone());

    let mut cur = RenderBuffer::new(width, height);
    for y in 2..height {
        cur.set_cell((0, y), &Cell::narrow("X"));
    }
    cur.clear_touched();
    renderer.cur_buf = Some(cur);

    let mut new_buf = RenderBuffer::new(width, height);
    set_text(&mut new_buf, 0, "top");
    for y in 1..height {
        for x in 0..width {
            new_buf.set_cell((x, y), &blank.clone());
        }
    }

    let mut out = Vec::new();
    let top = renderer.clear_bottom(&mut out, &new_buf).unwrap();
    assert_eq!(top, 1);
    assert_eq!(out, b"\n\x1b[J");
}

#[test]
fn clear_bottom_skips_ed_without_bce_for_styled_blank() {
    // Without BCE, ED would paint with the terminal's default
    // background, dropping the styled bg the buffer records. The
    // renderer must skip the ED fast path and let the per-row
    // transform emit styled spaces explicitly.
    let width = 8;
    let height = 4;
    let bg = Style::EMPTY.with_bg(Color::Basic(BasicColor::Blue));
    let blank = Cell::BLANK.with_style(bg);
    let mut renderer = renderer(width, height, Optimizations::none());
    renderer.cur.set_style(blank.style().clone());

    let mut cur = RenderBuffer::new(width, height);
    for y in 2..height {
        cur.set_cell((0, y), &Cell::narrow("X"));
    }
    cur.clear_touched();
    renderer.cur_buf = Some(cur);

    let mut new_buf = RenderBuffer::new(width, height);
    set_text(&mut new_buf, 0, "top");
    for y in 1..height {
        for x in 0..width {
            new_buf.set_cell((x, y), &blank.clone());
        }
    }

    let mut out = Vec::new();
    let top = renderer.clear_bottom(&mut out, &new_buf).unwrap();
    assert_eq!(top, height as usize, "no rows trimmed without BCE");
    assert!(
        out.is_empty(),
        "no bytes should be emitted, got {:?}",
        std::str::from_utf8(&out)
    );
}

#[test]
fn clear_bottom_does_not_trim_rows_with_skip_cells() {
    // Skip placeholders mark cells whose visible content is owned
    // by an external paint layer. clear_bottom must not treat
    // their rows as blank-trimmable — doing so excludes the row
    // from the per-row diff and suppresses the clearing bytes
    // that should fire when the placeholder moves elsewhere.
    let width = 10;
    let height = 6;
    let mut renderer = renderer(width, height, opts_with(|o| o.insert(Optimizations::BCE)));

    // Cur frame: skip at (3, 5) (last row, surrounded by blanks).
    let mut cur = RenderBuffer::new(width, height);
    cur.set_cell((3, 5), &Cell::skip());
    cur.clear_touched();
    renderer.cur_buf = Some(cur);

    // New frame: no skip anywhere — the placeholder was cleared.
    let new_buf = RenderBuffer::new(width, height);

    let mut out = Vec::new();
    let top = renderer.clear_bottom(&mut out, &new_buf).unwrap();
    assert_eq!(
        top, height as usize,
        "rows with a skip placeholder must remain in the diff scope"
    );
}
