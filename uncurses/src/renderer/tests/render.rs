//! Behavioural tests for [`Renderer::render`] and the cursor /
//! glyph emission helpers in the frame submodule.

use crate::Position;
use crate::cell::Cell;
use crate::renderer::RenderBuffer;
use crate::renderer::Renderer;

#[test]
fn test_post_scroll_splitter_chain_emits_correct_horizontal_back() {
    // Reproduce the splitter chain at the bottom of a scrolled-up
    // region: a left panel ends at col 55 with `│`, and the right
    // panel preview row holds `│    }` followed by trailing blanks.
    // The next row also starts at col 55 with `│`. The renderer
    // must move the cursor from the end of row 47's emitted content
    // (col 61) back to col 55 on row 48, not back by 1.
    use crate::cell::Cell;
    use crate::renderer::Renderer;
    use crate::renderer::buffer::RenderBuffer;
    use crate::style::Style;

    let mut r = Renderer::new();
    r.set_fullscreen(true);
    r.set_relative_cursor(false);
    let mut rb = RenderBuffer::new(120, 50);

    let splitter_col: u16 = 55;
    let dim_splitter = Cell::new("│", 1).with_style(Style::EMPTY.faint());

    // Frame 1: priming render so cur_buf is initialised. Just put a
    // splitter on row 0 so render() has at least one change.
    rb.set_cell((splitter_col, 0), &dim_splitter);
    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb).unwrap();
    sink.clear();

    // Simulate post-scroll state: blank cur_buf rows 46-48 (mirroring
    // BCE fill after a `\e[?S` scroll-up) and seed new_buf rows 46-48
    // with the right-panel splitter chain.
    if let Some(cb) = r.cur_buf.as_mut() {
        for y in 46u16..=48 {
            if let Some(row) = cb.line_mut(y) {
                for c in row.iter_mut() {
                    *c = Cell::BLANK;
                }
            }
        }
    }
    // New buffer rows: row 46 has `│    }`, row 47 has `│` then
    // some preview content, row 48 has `│` then blanks.
    for y in 46u16..=48 {
        for x in 0..120u16 {
            rb.set_cell((x, y), &Cell::BLANK);
        }
        rb.set_cell((splitter_col, y), &dim_splitter);
    }
    // Row 46: right-panel preview `    }` after the splitter.
    let row46_tail: &[(u16, &str)] = &[(56, " "), (57, " "), (58, " "), (59, " "), (60, "}")];
    for &(x, ch) in row46_tail {
        rb.set_cell((x, 46), &Cell::new(ch, 1));
    }
    // Force the transform to visit every column on those rows.
    rb.touch_full_line(46);
    rb.touch_full_line(47);
    rb.touch_full_line(48);
    // Pretend the cursor was left somewhere off-row so the planner
    // emits a CUP to row 46 before the diff starts.
    r.invalidate_cursor();

    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb).unwrap();
    let out = String::from_utf8_lossy(&sink).to_string();

    // The chain `│    }\n\b│` would place the row 47 splitter at
    // col 60 instead of col 55. Bytes between the `}` and the next
    // `│` must contain a CUB(6) (or HPA/CHA/CUP) — not a single BS.
    if let Some(brace) = out.find('}') {
        let tail = &out[brace + 1..];
        // Next move bytes up to the second splitter glyph (UTF-8 │).
        let next_split = tail.find('│').expect("expected next splitter glyph");
        let between = &tail[..next_split];
        let has_back_6 = between.contains("\x1b[6D")
            || between.contains("\x1b[56G")
            || between.contains("\x1b[56`")
            || between.contains("\x1b[48;56H")
            || between.contains("\x1b[\x08\x08\x08\x08\x08\x08") // unlikely but exhaustive
            || between.contains("\r");
        let single_bs = between.matches('\x08').count();
        assert!(
            has_back_6 || single_bs >= 6,
            "expected CUB(6)/HPA/CHA/CUP/CR between `}}` and next splitter, got {:?}",
            between
        );
    }
}

#[test]
fn test_renderer_new() {
    let r = Renderer::new();
    assert!(!r.fullscreen);
}

#[test]
fn test_render_empty() {
    let mut r = Renderer::new();
    let mut rb = RenderBuffer::new(10, 5);
    let mut out = Vec::new();
    r.render(&mut out, &mut rb).unwrap();
    // No changes → no output
    assert!(out.is_empty());
}

#[test]
fn test_render_with_changes() {
    let mut r = Renderer::new();
    let mut rb = RenderBuffer::new(10, 5);
    rb.set_cell((0, 0), &Cell::new("X", 1));
    let mut out = Vec::new();
    r.render(&mut out, &mut rb).unwrap();
    assert!(!out.is_empty());
    let output = String::from_utf8_lossy(&out);
    assert!(output.contains('X'));
}

#[test]
fn test_render_drains_internal_buffer() {
    let mut r = Renderer::new();
    let mut rb = RenderBuffer::new(10, 5);
    rb.set_cell((0, 0), &Cell::new("A", 1));
    let mut out = Vec::new();
    r.render(&mut out, &mut rb).unwrap();
    assert!(!out.is_empty());
}

#[test]
fn test_long_to_short_line_clears_trailing_chars() {
    // Frame N: row 0 contains long text "icu_segmenter = \"2\"".
    // Frame N+1: row 0 is empty (all blanks). The renderer must
    // emit a clear-to-EOL (or equivalent) so the old characters
    // do not remain on the terminal.
    let mut r = Renderer::new();
    let mut rb = RenderBuffer::new(40, 2);
    for (i, ch) in "icu_segmenter = \"2\"".chars().enumerate() {
        rb.set_cell((i as u16, 0), &Cell::new(ch.to_string(), 1));
    }
    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb).unwrap();
    sink.clear();

    // Frame N+1 — same buffer but cleared.
    for x in 0..40u16 {
        rb.set_cell((x, 0), &Cell::BLANK);
    }
    r.render(&mut sink, &mut rb).unwrap();
    let out = String::from_utf8_lossy(&sink).to_string();
    // The renderer must clear the row somehow — either ECH, EL, or
    // by writing spaces. If neither EL nor ECH appears, look for at
    // least one literal space we wrote.
    let has_el = out.contains("\x1b[K") || out.contains("\x1b[0K");
    let has_ech = out.contains('X') && out.contains("\x1b[");
    let has_spaces = out.contains(' ');
    assert!(
        has_el || has_ech || has_spaces,
        "expected clearing sequence in output {out:?}"
    );
}

#[test]
fn test_blank_row_after_content_clears_to_eol() {
    // Targeted: row had real content, next frame has none. The
    // emitted bytes for that row must include either a clear-to-EOL
    // or enough spaces to overwrite every previously-written column.
    let mut r = Renderer::new();
    let mut rb = RenderBuffer::new(20, 2);
    let s = "libc = \"0.2\"";
    for (i, ch) in s.chars().enumerate() {
        rb.set_cell((i as u16, 0), &Cell::new(ch.to_string(), 1));
    }
    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb).unwrap();
    sink.clear();

    for x in 0..20u16 {
        rb.set_cell((x, 0), &Cell::BLANK);
    }
    r.render(&mut sink, &mut rb).unwrap();
    let out_bytes = sink.clone();
    let out = String::from_utf8_lossy(&out_bytes).to_string();
    // Must clear via EL, ECH, or by writing literal spaces over the row.
    let has_el = out.contains("\x1b[K") || out.contains("\x1b[0K");
    let has_ech = out.contains('X')
        && out
            .as_bytes()
            .windows(3)
            .any(|w| w[0] == 0x1b && w[1] == b'[');
    let space_run_len = out
        .as_bytes()
        .windows(s.len())
        .filter(|w| w.iter().all(|b| *b == b' '))
        .count();
    assert!(
        has_el || has_ech || space_run_len > 0,
        "row was not cleared after content vanished: {out:?}"
    );
}

#[test]
fn test_wide_cell_at_last_column_skips_lr_dance() {
    // A width-2 cell that occupies the last two columns of the
    // bottom row does NOT sit "in" the lower-right corner; the
    // cursor when emitting it is at width-2, not width-1. The LR
    // DECAWM dance must not fire — it would needlessly emit
    // DECAWM toggles around an ordinary write.
    let mut r = Renderer::new();
    r.set_fullscreen(true);
    let mut rb = RenderBuffer::new(10, 3);
    // Fill row 2 with single-width cells in columns 0..8, then a
    // width-2 cell at column 8 (occupying 8 and 9).
    for x in 0..8u16 {
        rb.set_cell((x, 2), &Cell::new("a", 1));
    }
    rb.set_cell((8, 2), &Cell::new("漢", 2));
    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb).unwrap();
    let out = String::from_utf8_lossy(&sink).to_string();
    // The DECAWM dance markers must NOT appear: the wide cell at
    // col 8 isn't the LR corner cell.
    assert!(
        !out.contains("\x1b[?7l"),
        "wide cell reaching the last column should not trigger DECAWM-off: {out:?}"
    );
    assert!(
        !out.contains("\x1b[?7h"),
        "wide cell reaching the last column should not trigger DECAWM-on: {out:?}"
    );
}

#[test]
fn test_lower_right_corner_disables_autowrap() {
    // In fullscreen mode, writing the bottom-right cell must wrap the
    // glyph between DECAWM-off / DECAWM-on so the terminal does not
    // auto-wrap and scroll the alt-screen up by one row.
    let mut r = Renderer::new();
    r.set_fullscreen(true);
    let mut rb = RenderBuffer::new(10, 3);
    for x in 0..10u16 {
        rb.set_cell((x, 2), &Cell::new("Z", 1));
    }
    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb).unwrap();
    let out = String::from_utf8_lossy(&sink).to_string();
    let last_z = out.rfind('Z').expect("expected Z in output");
    let before = &out[..last_z];
    let after = &out[last_z + 1..];
    assert!(
        before.ends_with("\x1b[?7l"),
        "expected DECAWM-off immediately before bottom-right write, got tail {:?}",
        &before[before.len().saturating_sub(20)..]
    );
    assert!(
        after.starts_with("\x1b[?7h"),
        "expected DECAWM-on immediately after bottom-right write, got {:?}",
        &after[..after.len().min(20)]
    );
}

#[test]
fn test_phantom_cleared_by_move_cursor() {
    let mut r = Renderer::new();
    r.last_width = 10;
    r.last_height = 5;
    r.cur.pos = Position { y: 0, x: 10 };
    r.cur.at_phantom = true;
    let mut sink = Vec::new();
    let rb = RenderBuffer::new(10, 5);
    r.move_to(&mut sink, &rb, 2, 3).unwrap();
    assert!(!r.cur.at_phantom);
    assert_eq!(r.cur.pos.y, 2);
    assert_eq!(r.cur.pos.x, 3);
}

#[test]
fn test_phantom_after_line_filling_write() {
    let mut r = Renderer::new();
    r.last_width = 130;
    r.last_height = 30;
    r.cur.pos = Position { y: 5, x: 44 };
    let mut sink = Vec::new();
    for i in 0..86u16 {
        let b = b'a' + (i as u8 % 26);
        r.put_glyph_bytes(&mut sink, &[b], 1, 130, 30).unwrap();
    }
    assert!(
        r.cur.at_phantom,
        "phantom flag must be set after filling last column"
    );
    assert_eq!(
        r.cur.pos.x, 130,
        "tracked cursor parks one past last column"
    );
}

#[test]
fn test_move_cursor_emits_cr_when_phantom() {
    let mut r = Renderer::new();
    r.last_width = 130;
    r.last_height = 30;
    r.cur.pos = Position { y: 5, x: 130 };
    r.cur.at_phantom = true;
    let mut sink = Vec::new();
    let rb = RenderBuffer::new(130, 30);
    r.move_to(&mut sink, &rb, 6, 44).unwrap();
    assert!(!r.cur.at_phantom);
    assert!(
        sink.starts_with(b"\r"),
        "expected CR reset before optimal move, got {:?}",
        sink
    );
}

#[test]
fn test_overwrite_advance_uses_cell_content() {
    let mut r = Renderer::new();
    r.last_width = 20;
    r.last_height = 5;
    r.cur.x_unknown = false;
    r.cur.y_unknown = false;
    let mut rb = RenderBuffer::new(20, 5);
    for (i, ch) in ['a', 'b', 'c', 'd', 'e', 'f'].iter().enumerate() {
        rb.set_cell((i as u16, 0), &Cell::new(ch.to_string(), 1));
    }
    let mut sink = Vec::new();
    r.move_cursor(&mut sink, &rb, 0, 6).unwrap();
    // CUF cheaper here; output is not the raw text.
    assert_ne!(sink, b"abcdef");
    assert_eq!(r.cur.pos.x, 6);

    // Larger gap: CUF(10) = 5 bytes, overwriting 10 ASCII cells
    // = 10 bytes. CUF still wins.
    sink.clear();
    r.cur.pos = Position { y: 0, x: 0 };
    for i in 0..10u16 {
        rb.set_cell((i, 0), &Cell::new("x", 1));
    }
    r.move_cursor(&mut sink, &rb, 0, 10).unwrap();
    assert!(sink.starts_with(b"\x1b["));

    // Tiny gap where CUF is largest and overwrite is short: from
    // col 0 moving to col 3 with three ASCII cells — overwrite =
    // 3 bytes, CUF(3) = 4 bytes (\x1b[3C). Overwrite wins.
    sink.clear();
    r.cur.pos = Position { y: 0, x: 0 };
    for i in 0..3u16 {
        rb.set_cell((i, 0), &Cell::new("y", 1));
    }
    r.move_cursor(&mut sink, &rb, 0, 3).unwrap();
    assert_eq!(sink, b"yyy");
    assert_eq!(r.cur.pos.x, 3);
}

/// G-7: inline shrink at the same width emits a partial clear at
/// the new bottom row and does NOT carpet-bomb the surface with a
/// full ED. Orphan rows below the new height get wiped, content
/// above is preserved in cur_buf so the next transform pass can
/// diff against it.
#[test]
fn test_inline_shrink_partial_clear() {
    let mut r = Renderer::new();
    r.set_fullscreen(false);
    // Frame 1: 10x5 with content on every row.
    let mut rb = RenderBuffer::new(10, 5);
    for y in 0..5u16 {
        for x in 0..10u16 {
            rb.set_cell((x, y), &Cell::new("A", 1));
        }
    }
    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb).unwrap();
    sink.clear();

    // Frame 2: shrink to 10x3, same content on remaining rows.
    let mut rb2 = RenderBuffer::new(10, 3);
    for y in 0..3u16 {
        for x in 0..10u16 {
            rb2.set_cell((x, y), &Cell::new("A", 1));
        }
    }
    r.render(&mut sink, &mut rb2).unwrap();
    let out = String::from_utf8_lossy(&sink).to_string();
    // Must emit ED-below (\x1b[J or \x1b[0J) at the new bottom row.
    assert!(
        out.contains("\x1b[J") || out.contains("\x1b[0J"),
        "expected ED-below for partial clear, got {out:?}"
    );
    // Must NOT clear from the top — the rows above the new bottom
    // were unchanged and should not be repainted from scratch.
    assert!(
        !out.starts_with("\x1b[2J") && !out.contains("\x1b[H\x1b[J"),
        "unexpected full clear in {out:?}"
    );
}

/// G-4: after an inline resize the cursor lands at (0, new_height-1)
/// regardless of where the transform pass left it.
#[test]
fn test_inline_resize_moves_cursor_to_bottom_left() {
    let mut r = Renderer::new();
    r.set_fullscreen(false);
    // Frame 1: 10x5 with content.
    let mut rb = RenderBuffer::new(10, 5);
    rb.set_cell((0, 0), &Cell::new("A", 1));
    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb).unwrap();
    sink.clear();

    // Frame 2: grow to 10x7 with content on row 0 only. After the
    // transform pass the cursor would naturally sit somewhere near
    // row 0; the bottom-left snap must reposition it.
    let mut rb2 = RenderBuffer::new(10, 7);
    rb2.set_cell((0, 0), &Cell::new("B", 1));
    r.render(&mut sink, &mut rb2).unwrap();
    assert_eq!(r.cur.pos, Position { y: 6, x: 0 });
}

/// Fullscreen resize does NOT trigger the inline bottom-left move.
#[test]
fn test_fullscreen_resize_does_not_force_bottom_left() {
    let mut r = Renderer::new();
    r.set_fullscreen(true);
    let mut rb = RenderBuffer::new(10, 5);
    rb.set_cell((3, 2), &Cell::new("X", 1));
    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb).unwrap();
    sink.clear();

    let mut rb2 = RenderBuffer::new(10, 7);
    rb2.set_cell((3, 2), &Cell::new("X", 1));
    r.render(&mut sink, &mut rb2).unwrap();
    // Fullscreen path must not force the cursor to the bottom-left.
    assert_ne!(r.cur.pos, Position { y: 6, x: 0 });
}

/// Resize without force_clear preserves cur_buf content: a row
/// whose content survives the resize unchanged emits NO bytes for
/// that row in the next frame. (Previously the unconditional
/// force_clear repainted everything.)
#[test]
fn test_resize_preserves_cur_buf_for_unchanged_rows() {
    let mut r = Renderer::new();
    r.set_fullscreen(false);
    // Frame 1: 10x5, row 0 = "HELLO".
    let mut rb = RenderBuffer::new(10, 5);
    for (i, ch) in "HELLO".chars().enumerate() {
        rb.set_cell((i as u16, 0), &Cell::new(ch.to_string(), 1));
    }
    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb).unwrap();
    sink.clear();

    // Frame 2: grow to 10x7, row 0 still "HELLO", new rows blank.
    let mut rb2 = RenderBuffer::new(10, 7);
    for (i, ch) in "HELLO".chars().enumerate() {
        rb2.set_cell((i as u16, 0), &Cell::new(ch.to_string(), 1));
    }
    r.render(&mut sink, &mut rb2).unwrap();
    let out = String::from_utf8_lossy(&sink).to_string();
    // The "HELLO" glyphs must not be re-emitted — cur_buf row 0
    // still matches new_buf row 0 after the resize.
    assert!(
        !out.contains("HELLO"),
        "unchanged row should not be repainted, got {out:?}"
    );
}

/// save_cursor / restore_cursor must round-trip the right-margin
/// phantom flag and the per-axis "unknown" bits, not just the
/// position+pen carried by Cursor. Without this, alt-screen exit
/// (DECRST 1049) leaves the renderer with stale phantom/unknown
/// state from the alt screen's last write.
#[test]
fn test_save_restore_round_trips_phantom_and_unknown_flags() {
    let mut r = Renderer::new();
    r.last_width = 80;
    r.last_height = 24;
    r.cur.pos = Position { y: 3, x: 80 };
    r.cur.at_phantom = true;
    r.cur.x_unknown = false;
    r.cur.y_unknown = false;

    r.save_cursor();

    // Mutate every saved bit so a no-op restore would be detected.
    r.cur.pos = Position { y: 0, x: 0 };
    r.cur.at_phantom = false;
    r.cur.x_unknown = true;
    r.cur.y_unknown = true;

    r.restore_cursor();
    assert_eq!(r.cur.pos, Position { y: 3, x: 80 });
    assert!(r.cur.at_phantom);
    assert!(!r.cur.x_unknown);
    assert!(!r.cur.y_unknown);
}

/// set_cursor_position must clear the per-axis "unknown" bits — the
/// caller has just authoritatively repositioned the terminal cursor
/// and the next move planner shouldn't fall through to emitting a
/// redundant absolute CUP.
#[test]
fn test_set_cursor_position_clears_unknown_flags() {
    let mut r = Renderer::new();
    assert!(r.cur.x_unknown && r.cur.y_unknown);
    r.set_cursor_position(Position { y: 5, x: 7 });
    assert_eq!(r.cur.pos, Position { y: 5, x: 7 });
    assert!(!r.cur.x_unknown);
    assert!(!r.cur.y_unknown);
    assert!(!r.cur.at_phantom);
}
