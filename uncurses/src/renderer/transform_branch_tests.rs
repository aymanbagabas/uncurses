use super::{Optimizations, RenderBuffer, Renderer};
use crate::cell::Cell;
use crate::color::Color;
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
    renderer.cur.x = Some(0);
    renderer.cur.y = Some(0);
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
    let red = Style::default().fg(Color::Red);
    new_buf.set_cell((0, 0), &Cell::narrow("R").style(red));

    let out = transform_output(&mut renderer, &new_buf);
    assert_eq!(out, b"\x1b[31mR");
}

#[test]
fn transform_osc8_link_open_and_close() {
    let width = 10;
    let mut renderer = renderer(width, 1, Optimizations::none());
    renderer.cur_buf = Some(RenderBuffer::new(width, 1));

    let linked_style = Style::default().link("https://example.test", "");
    let mut linked_buf = RenderBuffer::new(width, 1);
    linked_buf.set_cell((0, 0), &Cell::narrow("L").style(linked_style));
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
        .set_style(Style::default().link("https://example.test", ""));

    let mut out = Vec::new();
    renderer.reset_pen(&mut out).unwrap();
    assert!(
        out.windows(b"\x1b]8;;\x1b\\".len())
            .any(|w| w == b"\x1b]8;;\x1b\\"),
        "expected OSC 8 close, got {:?}",
        std::str::from_utf8(&out)
    );
    assert!(
        renderer.cur.style().link.is_none(),
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
    let bg = Style::default().bg(Color::Blue);
    let blank = Cell::BLANK.style(bg);
    let mut renderer = renderer(width, height, opts_with(|o| o.insert(Optimizations::BCE)));
    renderer.cur.set_style(blank.style.clone());

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
    let bg = Style::default().bg(Color::Blue);
    let blank = Cell::BLANK.style(bg);
    let mut renderer = renderer(width, height, Optimizations::none());
    renderer.cur.set_style(blank.style.clone());

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

/// A frame whose first difference is a continuation still reaches the wire.
///
/// The column scan can stop on the second half of a cluster. Emission places
/// the cursor at that column, and a continuation writes no bytes and moves no
/// cursor, so the run was dropped and the terminal kept the frame before it.
/// Closing the run back to the cell that owns the column re-emits the whole
/// cluster instead.
#[test]
fn a_difference_on_a_continuation_re_emits_its_cluster() {
    let mut renderer = Renderer::new();
    renderer.set_optimizations(Optimizations::none());
    let mut buf = RenderBuffer::new(24, 1);
    for x in (0..24).step_by(2) {
        buf.set_cell((x, 0), &Cell::wide("\u{4e16}"));
    }
    let mut out = Vec::new();
    renderer.render(&mut out, &mut buf).unwrap();

    // Disturb a continuation and leave its lead alone. Reaching the row
    // directly is the only way there, because `set` leaves a continuation to
    // the cluster that owns it.
    if let Some(line) = buf.line_mut(0) {
        line[13] = Cell::continuation().style(Style::default().bg(Color::Red));
    }
    buf.touch_line(0, 13, 13);

    let mut out = Vec::new();
    renderer.render(&mut out, &mut buf).unwrap();
    let text = String::from_utf8_lossy(&out);
    assert!(
        text.contains('\u{4e16}'),
        "the cluster owning column 13 never reached the terminal: {text:?}"
    );
}

/// The DCH branch: two frames whose wide cluster sits at different
/// columns, so the walk-back names a lead as the last matching cell.
///
/// Without the wide-cell adjustment, DCH's cursor lands on `n + 1`, which
/// for a lead is the continuation the wide-glyph emission just painted. The
/// terminal starts deleting inside the glyph. The renderer's debug assert
/// on `DCH move_to(...) lands inside a cluster` catches this, and the
/// emitted bytes have to leave the terminal showing the exact cluster new
/// specified.
#[test]
fn dch_across_a_wide_cluster_stays_past_the_glyph() {
    for &has_dch in &[false, true] {
        let mut opts = Optimizations::none();
        if has_dch {
            opts.insert(Optimizations::DCH);
        }
        let mut renderer = Renderer::new();
        renderer.set_optimizations(opts);
        let width = 6u16;
        let mut buf = RenderBuffer::new(width, 1);
        buf.set_cell((0, 0), &Cell::narrow("B"));
        buf.set_cell((1, 0), &Cell::narrow("C"));
        buf.set_cell((2, 0), &Cell::narrow("D"));
        buf.set_cell((3, 0), &Cell::wide("\u{4e16}"));
        let mut out = Vec::new();
        renderer.render(&mut out, &mut buf).unwrap();

        // Frame 2 shifts the cluster left by two columns, so walk-back
        // matches through the lead and o_lc lands ahead of n_lc.
        buf.set_cell((0, 0), &Cell::narrow("A"));
        buf.set_cell((1, 0), &Cell::wide("\u{4e16}"));
        buf.set_cell((3, 0), &Cell::BLANK);
        buf.set_cell((4, 0), &Cell::BLANK);
        let mut out = Vec::new();
        renderer.render(&mut out, &mut buf).unwrap();

        let text = String::from_utf8_lossy(&out);
        // The cluster has to appear whole on the wire, since a mid-cluster
        // DCH would cut its continuation loose or delete its lead.
        assert!(
            text.contains('\u{4e16}'),
            "dch={has_dch}: the cluster never reached the terminal: {text:?}"
        );
    }
}

/// Step 4a's single-column emit: when a lead is the only non-blank
/// column, the emitter has to be told about the whole cluster it fills, not
/// just the lead's own column.
///
/// Naming the range as one column and relying on the wide-glyph write to
/// spill past its stated end leaves the semantic range out of step with the
/// cursor. Extending the end to the cluster's own end keeps the two in step.
#[test]
fn a_lone_lead_covers_its_whole_cluster() {
    let mut renderer = Renderer::new();
    renderer.set_optimizations(Optimizations::none());
    let width = 6u16;
    let mut buf = RenderBuffer::new(width, 1);
    // Frame 1: a row of blanks so `first_cell` will settle at the lead.
    let mut out = Vec::new();
    renderer.render(&mut out, &mut buf).unwrap();

    // Frame 2: a single wide cluster at column 0. Everything else stays
    // blank, so Step 4a runs.
    buf.set_cell((0, 0), &Cell::wide("\u{4e16}"));
    let mut out = Vec::new();
    renderer.render(&mut out, &mut buf).unwrap();

    let text = String::from_utf8_lossy(&out);
    assert!(
        text.contains('\u{4e16}'),
        "Step 4a dropped the cluster: {text:?}"
    );
}

/// The Step 2 fast-path walk-back over matching cells has to close over
/// clusters too. Stopping the range one column before a lead's continuation
/// would emit a run whose cursor rests past its stated end.
///
/// The trailing cell is styled (non-blank) so the trailing-uncolorable
/// branch is chosen. Cur and new differ before a wide cluster, matching
/// through it and the cell after.
#[test]
fn step2_fast_path_closes_over_a_trailing_cluster() {
    let mut renderer = Renderer::new();
    renderer.set_optimizations(Optimizations::none());
    let width = 6u16;
    let mut buf = RenderBuffer::new(width, 1);
    let styled = Style::default().bg(Color::Blue);
    // A styled cell in the last column forces the trailing-uncolorable path.
    buf.set_cell((0, 0), &Cell::narrow("A"));
    buf.set_cell((1, 0), &Cell::wide("\u{4e16}"));
    buf.set_cell((3, 0), &Cell::narrow("D"));
    buf.set_cell((width - 1, 0), &Cell::narrow("Z").style(styled.clone()));
    let mut out = Vec::new();
    renderer.render(&mut out, &mut buf).unwrap();

    // Frame 2 changes A -> B and keeps everything else the same, so the
    // walk-back would name a run ending on the cluster's continuation.
    buf.set_cell((0, 0), &Cell::narrow("B"));
    let mut out = Vec::new();
    renderer.render(&mut out, &mut buf).unwrap();

    let text = String::from_utf8_lossy(&out);
    // The cluster has to survive the frame, since a truncated run would
    // leave the cursor a column early on any downstream write.
    assert!(
        text.contains('B'),
        "Step 2 fast path dropped the changed cell: {text:?}"
    );
}

/// The capability sets the planner chooses between. Each takes its own path
/// through emission, so a boundary fixed on one can still be broken on
/// another.
fn emission_paths() -> Vec<(&'static str, Optimizations)> {
    vec![
        ("none", Optimizations::none()),
        ("ech", Optimizations::none() | Optimizations::ECH),
        ("rep", Optimizations::none() | Optimizations::REP),
        ("ich", Optimizations::none() | Optimizations::ICH),
        ("dch", Optimizations::none() | Optimizations::DCH),
        ("xterm", Optimizations::xterm()),
        ("modern", Optimizations::modern()),
    ]
}

/// A row of two-column clusters, so column `2n` owns one.
fn wide_row(buf: &mut RenderBuffer, width: u16, glyph: &str) {
    for x in (0..width).step_by(2) {
        buf.set_cell((x, 0), &Cell::wide(glyph));
    }
}

/// Every emission path keeps a disturbed cluster whole.
///
/// The frame-drop this guards against depends on which branch the planner
/// takes, and the branch depends on the capabilities, so one set proving the
/// point says nothing about the rest.
#[test]
fn every_emission_path_re_emits_a_disturbed_cluster() {
    for (name, opts) in emission_paths() {
        let mut renderer = Renderer::new();
        renderer.set_optimizations(opts);
        let mut buf = RenderBuffer::new(24, 1);
        wide_row(&mut buf, 24, "\u{4e16}");
        let mut out = Vec::new();
        renderer.render(&mut out, &mut buf).unwrap();

        if let Some(line) = buf.line_mut(0) {
            line[13] = Cell::continuation().style(Style::default().bg(Color::Red));
        }
        buf.touch_line(0, 13, 13);

        let mut out = Vec::new();
        renderer.render(&mut out, &mut buf).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains('\u{4e16}'),
            "{name}: the cluster owning column 13 never reached the terminal: {text:?}"
        );
    }
}

/// Rows whose clusters sit at different columns still emit whole glyphs.
///
/// This is the case where counting clusters and counting columns diverge:
/// the old row and the new one disagree about which columns own a cell, so a
/// boundary derived from one is wrong for the other.
#[test]
fn rows_whose_clusters_sit_at_different_columns_stay_whole() {
    for (name, opts) in emission_paths() {
        let mut renderer = Renderer::new();
        renderer.set_optimizations(opts);
        let mut buf = RenderBuffer::new(24, 1);
        wide_row(&mut buf, 24, "\u{4e16}");
        let mut out = Vec::new();
        renderer.render(&mut out, &mut buf).unwrap();

        // Shift every cluster one column right by opening the row with a
        // narrow cell, so no cluster lines up with where one used to be.
        buf.set_cell((0, 0), &Cell::narrow("a"));
        for x in (1..23).step_by(2) {
            buf.set_cell((x, 0), &Cell::wide("\u{754c}"));
        }
        let mut out = Vec::new();
        renderer.render(&mut out, &mut buf).unwrap();

        // Whatever the planner emitted, the glyphs it drew have to account
        // for whole clusters: a partial one would mean a run cut a glyph.
        let text = String::from_utf8_lossy(&out);
        let drawn = text.matches('\u{754c}').count();
        assert!(
            drawn > 0,
            "{name}: the shifted row never reached the terminal: {text:?}"
        );
    }
}
const SWEEP_COLUMNS: u16 = 40;

fn sweep_renderer(opts: Optimizations) -> Renderer {
    let mut r = Renderer::new();
    r.set_optimizations(opts);
    r
}

// A drag is a style sweeping over rows of mixed width, and the frames it
// produces are what reach for moves that keep the column. The renderer's
// tracked cursor is what those moves trust, so a column of drift draws the
// next row early, over the second half of whatever glyph sits there.

fn simulate(out: &[u8], from: (u16, u16)) -> (u16, u16) {
    let text = String::from_utf8_lossy(out);
    let bytes = text.as_bytes();
    let (mut x, mut y) = from;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                x = 0;
                i += 1;
            }
            b'\n' => {
                y = y.saturating_add(1);
                i += 1;
            }
            b'\t' => {
                x = (x / 8 + 1) * 8;
                i += 1;
            }
            0x08 => {
                x = x.saturating_sub(1);
                i += 1;
            }
            0x1b if i + 1 < bytes.len() && bytes[i + 1] == b'[' => {
                let mut j = i + 2;
                let mut params: Vec<u16> = Vec::new();
                let mut value: Option<u16> = None;
                let mut private = false;
                while j < bytes.len() {
                    match bytes[j] {
                        b'0'..=b'9' => {
                            value = Some(value.unwrap_or(0) * 10 + u16::from(bytes[j] - b'0'));
                            j += 1;
                        }
                        // A colon separates the parts of one parameter, as
                        // in a direct colour. Both belong to the sequence.
                        b';' | b':' => {
                            params.push(value.take().unwrap_or(0));
                            j += 1;
                        }
                        b'?' => {
                            private = true;
                            j += 1;
                        }
                        _ => break,
                    }
                }
                let tail = value;
                params.push(tail.unwrap_or(0));
                if j < bytes.len() && !private {
                    // A relative move reads its count from the first
                    // parameter, not the last.
                    let n = params.first().copied().filter(|v| *v > 0).unwrap_or(1);
                    match bytes[j] {
                        b'H' | b'f' => {
                            y = params.first().copied().unwrap_or(1).max(1) - 1;
                            x = params.get(1).copied().unwrap_or(1).max(1) - 1;
                        }
                        b'G' | b'`' => x = params[0].max(1) - 1,
                        b'd' => y = params[0].max(1) - 1,
                        b'C' => x = x.saturating_add(n),
                        b'D' => x = x.saturating_sub(n),
                        b'A' => y = y.saturating_sub(n),
                        b'B' => y = y.saturating_add(n),
                        _ => {}
                    }
                }
                i = j + 1;
            }
            0x1b if i + 1 < bytes.len() && bytes[i + 1] == b'M' => {
                y = y.saturating_sub(1);
                i += 2;
            }
            // An OSC string runs to a string terminator, not two bytes.
            // Treating it as two would let its text advance the column.
            0x1b if i + 1 < bytes.len() && bytes[i + 1] == b']' => {
                let mut j = i + 2;
                while j < bytes.len() {
                    if bytes[j] == 0x07 {
                        j += 1;
                        break;
                    }
                    if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                        j += 2;
                        break;
                    }
                    j += 1;
                }
                i = j;
            }
            0x1b => i += 2,
            _ => {
                let ch = text[i..].chars().next().unwrap();
                let w = crate::text::WidthMode::Grapheme
                    .grapheme_width(ch.encode_utf8(&mut [0u8; 4]), false);
                x = x.saturating_add(u16::from(w));
                i += ch.len_utf8();
            }
        }
    }
    (x, y)
}

/// The same invariant across a drag, which is where it was reported.
///
/// A selection is a style sweeping over rows of mixed width, and the frames
/// it produces are what reach for column-preserving moves. Each frame is
/// checked, because the drift shows up on one and is corrected on the next,
/// so a check only at the end would miss it.
#[test]
fn a_sweeping_highlight_keeps_the_tracked_cursor_honest() {
    for (name, opts) in emission_paths() {
        let mut renderer = sweep_renderer(opts);
        let mut buf = RenderBuffer::new(SWEEP_COLUMNS, 8);

        let wide: Vec<char> = "あのイーハトーヴォのすきとおった風、やまね"
            .chars()
            .collect();
        let ascii = "Grapheme clusters";
        let highlight = Style::default().fg(Color::Black).bg(Color::White);

        let paint = |buf: &mut RenderBuffer, upto: u16| {
            for (i, ch) in wide.iter().enumerate() {
                let x = (i as u16) * 2;
                let style = if x < upto {
                    highlight.clone()
                } else {
                    Style::default()
                };
                if x < SWEEP_COLUMNS {
                    buf.set_cell((x, 3), &Cell::wide(ch.to_string()).style(style));
                }
            }
            for (i, ch) in ascii.chars().enumerate() {
                let x = i as u16;
                let style = if x < upto {
                    highlight.clone()
                } else {
                    Style::default()
                };
                buf.set_cell((x, 5), &Cell::narrow(ch.to_string()).style(style));
            }
        };

        paint(&mut buf, 0);
        let mut out = Vec::new();
        renderer.render(&mut out, &mut buf).unwrap();
        let mut at = {
            let p = renderer.cursor_position();
            (p.x, p.y)
        };

        // Grow the highlight to the end, then pull it back, so both edges
        // sweep over the wide row in turn.
        let sweep = (0..=SWEEP_COLUMNS).chain((0..SWEEP_COLUMNS).rev());
        for upto in sweep {
            paint(&mut buf, upto);
            let mut out = Vec::new();
            renderer.render(&mut out, &mut buf).unwrap();
            let (sx, sy) = simulate(&out, at);
            let tracked = renderer.cursor_position();
            assert_eq!(
                (sx, sy),
                (tracked.x, tracked.y),
                "{name}: at highlight {upto} the bytes leave the cursor at ({sx}, {sy}) \
                 but the renderer tracked ({}, {})\nbytes: {:?}",
                tracked.x,
                tracked.y,
                String::from_utf8_lossy(&out)
            );
            at = (tracked.x, tracked.y);
        }
    }
}

/// An insert begins past the cluster that owns the column it starts from.
///
/// The walk that finds where two rows diverge steps one column at a time and
/// stops at column zero, so it can come to rest on the second half of a
/// glyph. Inserting there starts inside that glyph and destroys it.
#[test]
fn an_insert_starts_past_the_cluster_that_owns_its_column() {
    let mut renderer = Renderer::new();
    renderer.set_optimizations(Optimizations::none());
    let mut buf = RenderBuffer::new(8, 1);
    buf.set_cell((0, 0), &Cell::wide("\u{4e16}"));
    buf.set_cell((2, 0), &Cell::narrow("a"));
    let mut out = Vec::new();
    renderer.render(&mut out, &mut buf).unwrap();

    // A second cluster appears, pushing the narrow cell right. The walk back
    // from the divergence at column 2 runs to column 0, which is a lead.
    buf.set_cell((2, 0), &Cell::wide("\u{4e16}"));
    buf.set_cell((4, 0), &Cell::narrow("a"));
    let mut out = Vec::new();
    renderer.render(&mut out, &mut buf).unwrap();

    // The row the terminal ends up with has to hold both clusters, so two
    // glyphs have to go out. One means the insert landed inside the first.
    let text = String::from_utf8_lossy(&out);
    assert_eq!(
        text.matches('\u{4e16}').count(),
        2,
        "expected both clusters on the wire: {text:?}"
    );
}
