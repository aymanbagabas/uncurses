//! Behavioural tests for [`Renderer::render`] and the cursor /
//! glyph emission helpers in the frame submodule.

use crate::cell::Cell;
use crate::layout::Position;
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
    let dim_splitter = Cell::narrow("│").style(Style::default().faint());

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
        rb.set_cell((x, 46), &Cell::narrow(ch));
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
    rb.set_cell((0, 0), &Cell::narrow("X"));
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
    rb.set_cell((0, 0), &Cell::narrow("A"));
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
        rb.set_cell((i as u16, 0), &Cell::narrow(ch.to_string()));
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
        rb.set_cell((i as u16, 0), &Cell::narrow(ch.to_string()));
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
        rb.set_cell((x, 2), &Cell::narrow("a"));
    }
    rb.set_cell((8, 2), &Cell::wide("漢"));
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
        rb.set_cell((x, 2), &Cell::narrow("Z"));
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
    r.cur.set_pos(Position { y: 0, x: 10 });
    r.cur.at_phantom = true;
    let mut sink = Vec::new();
    let rb = RenderBuffer::new(10, 5);
    r.move_to(&mut sink, &rb, 2, 3).unwrap();
    assert!(!r.cur.at_phantom);
    assert_eq!(r.cur.pos().y, 2);
    assert_eq!(r.cur.pos().x, 3);
}

#[test]
fn test_phantom_after_line_filling_write() {
    let mut r = Renderer::new();
    r.last_width = 130;
    r.last_height = 30;
    r.cur.set_pos(Position { y: 5, x: 44 });
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
        r.cur.pos().x,
        130,
        "tracked cursor parks one past last column"
    );
}

#[test]
fn test_move_cursor_emits_cr_when_phantom() {
    let mut r = Renderer::new();
    r.last_width = 130;
    r.last_height = 30;
    r.cur.set_pos(Position { y: 5, x: 130 });
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
    r.cur.x = Some(0);
    r.cur.y = Some(0);
    let mut rb = RenderBuffer::new(20, 5);
    for (i, ch) in ['a', 'b', 'c', 'd', 'e', 'f'].iter().enumerate() {
        rb.set_cell((i as u16, 0), &Cell::narrow(ch.to_string()));
    }
    let mut sink = Vec::new();
    r.move_cursor(&mut sink, &rb, 0, 6).unwrap();
    // CUF cheaper here; output is not the raw text.
    assert_ne!(sink, b"abcdef");
    assert_eq!(r.cur.pos().x, 6);

    // Larger gap: CUF(10) = 5 bytes, overwriting 10 ASCII cells
    // = 10 bytes. CUF still wins.
    sink.clear();
    r.cur.set_pos(Position { y: 0, x: 0 });
    for i in 0..10u16 {
        rb.set_cell((i, 0), &Cell::narrow("x"));
    }
    r.move_cursor(&mut sink, &rb, 0, 10).unwrap();
    assert!(sink.starts_with(b"\x1b["));

    // Tiny gap where CUF is largest and overwrite is short: from
    // col 0 moving to col 3 with three ASCII cells — overwrite =
    // 3 bytes, CUF(3) = 4 bytes (\x1b[3C). Overwrite wins.
    sink.clear();
    r.cur.set_pos(Position { y: 0, x: 0 });
    for i in 0..3u16 {
        rb.set_cell((i, 0), &Cell::narrow("y"));
    }
    r.move_cursor(&mut sink, &rb, 0, 3).unwrap();
    assert_eq!(sink, b"yyy");
    assert_eq!(r.cur.pos().x, 3);
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
            rb.set_cell((x, y), &Cell::narrow("A"));
        }
    }
    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb).unwrap();
    sink.clear();

    // Frame 2: shrink to 10x3, same content on remaining rows.
    let mut rb2 = RenderBuffer::new(10, 3);
    for y in 0..3u16 {
        for x in 0..10u16 {
            rb2.set_cell((x, y), &Cell::narrow("A"));
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

/// Inline shrink with a force-clear: the partial clear plus
/// force-clear pass must move the cursor up by the full distance
/// from the previous bottom-left to the new top, not silently
/// teleport the tracked source past the new bounds. Regression for
/// a clamp that dropped `cur.pos.y` to the new max before the
/// relative-move planner could emit CUU, which left the physical
/// cursor parked below the new top and the rows above the new
/// surface untouched on screen.
#[test]
fn test_inline_shrink_with_force_clear_moves_cursor_to_new_top() {
    let mut r = Renderer::new();
    r.set_fullscreen(false);
    let mut rb = RenderBuffer::new(20, 11);
    for y in 0..11u16 {
        for x in 0..20u16 {
            rb.set_cell((x, y), &Cell::narrow("X"));
        }
    }
    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb).unwrap();
    sink.clear();

    r.request_clear();
    let mut rb2 = RenderBuffer::new(20, 3);
    rb2.set_cell((2, 1), &Cell::narrow("B"));
    r.render(&mut sink, &mut rb2).unwrap();
    let out = String::from_utf8_lossy(&sink).to_string();
    // Cursor was at (0, 10) after frame 1's bottom-left snap; the
    // force-clear preface must walk all the way back to (0, 0) of
    // the new 3-row surface — that is CUU 10, not CUU 2.
    assert!(
        out.contains("\x1b[10A"),
        "expected CUU 10 to reach the new top from the old bottom, got {out:?}"
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
    rb.set_cell((0, 0), &Cell::narrow("A"));
    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb).unwrap();
    sink.clear();

    // Frame 2: grow to 10x7 with content on row 0 only. After the
    // transform pass the cursor would naturally sit somewhere near
    // row 0; the bottom-left snap must reposition it.
    let mut rb2 = RenderBuffer::new(10, 7);
    rb2.set_cell((0, 0), &Cell::narrow("B"));
    r.render(&mut sink, &mut rb2).unwrap();
    assert_eq!(r.cur.pos(), Position { y: 6, x: 0 });
}

/// Fullscreen resize does NOT trigger the inline bottom-left move.
#[test]
fn test_fullscreen_resize_does_not_force_bottom_left() {
    let mut r = Renderer::new();
    r.set_fullscreen(true);
    let mut rb = RenderBuffer::new(10, 5);
    rb.set_cell((3, 2), &Cell::narrow("X"));
    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb).unwrap();
    sink.clear();

    let mut rb2 = RenderBuffer::new(10, 7);
    rb2.set_cell((3, 2), &Cell::narrow("X"));
    r.render(&mut sink, &mut rb2).unwrap();
    // Fullscreen path must not force the cursor to the bottom-left.
    assert_ne!(r.cur.pos(), Position { y: 6, x: 0 });
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
        rb.set_cell((i as u16, 0), &Cell::narrow(ch.to_string()));
    }
    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb).unwrap();
    sink.clear();

    // Frame 2: grow to 10x7, row 0 still "HELLO", new rows blank.
    let mut rb2 = RenderBuffer::new(10, 7);
    for (i, ch) in "HELLO".chars().enumerate() {
        rb2.set_cell((i as u16, 0), &Cell::narrow(ch.to_string()));
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
    r.cur.set_pos(Position { y: 3, x: 80 });
    r.cur.at_phantom = true;

    r.save_cursor();

    // Mutate every saved bit so a no-op restore would be detected.
    r.cur.x = None;
    r.cur.y = None;
    r.cur.at_phantom = false;

    r.restore_cursor();
    assert_eq!(r.cur.pos(), Position { y: 3, x: 80 });
    assert!(r.cur.at_phantom);
    assert!(r.cur.x.is_some());
    assert!(r.cur.y.is_some());
}

/// set_cursor_position must clear the per-axis "unknown" bits — the
/// caller has just authoritatively repositioned the terminal cursor
/// and the next move planner shouldn't fall through to emitting a
/// redundant absolute CUP.
#[test]
fn test_set_cursor_position_clears_unknown_flags() {
    let mut r = Renderer::new();
    assert!(r.cur.x.is_none() && r.cur.y.is_none());
    r.set_cursor_position(Position { y: 5, x: 7 });
    assert_eq!(r.cur.pos(), Position { y: 5, x: 7 });
    assert!(r.cur.x.is_some());
    assert!(r.cur.y.is_some());
}

/// Inline-mode round trip across a sequence of resizes that mixes
/// shrinks, grows, force-clears, and width changes. Each frame
/// fills its surface with one distinct glyph per row so we can
/// assert that no phantom rows from a previous (taller) frame
/// remain visible and that the cursor walks correctly between the
/// previous bottom-left snap and the next frame's content.
#[test]
fn test_inline_resize_sequence_round_trip() {
    fn fill(rb: &mut RenderBuffer, width: u16, height: u16, base: u8) {
        for y in 0..height {
            let ch = ((base + y as u8) as char).to_string();
            for x in 0..width {
                rb.set_cell((x, y), &Cell::narrow(&ch));
            }
        }
    }

    let mut r = Renderer::new();
    r.set_fullscreen(false);

    // ---- Frame 1: 20x5, row k = 'A'+k. Priming render. -------------
    let mut rb1 = RenderBuffer::new(20, 5);
    fill(&mut rb1, 20, 5, b'A');
    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb1).unwrap();
    let out1 = String::from_utf8_lossy(&sink).to_string();
    println!("FRAME1 out = {out1:?}");
    for k in 0..5u8 {
        let ch = (b'A' + k) as char;
        assert!(
            out1.contains(&ch.to_string().repeat(20)),
            "frame 1: missing run of 20×{ch:?} on row {k}: {out1:?}"
        );
    }
    // After frame 1 the inline-resize bottom-left snap parks the
    // cursor at (0, 4).
    assert_eq!(r.cur.pos(), Position { y: 4, x: 0 });
    sink.clear();

    // ---- Frame 2: shrink to 20x3, force-clear. row k = X/Y/Z. ------
    r.request_clear();
    let mut rb2 = RenderBuffer::new(20, 3);
    for (k, ch) in ['X', 'Y', 'Z'].iter().enumerate() {
        for x in 0..20u16 {
            rb2.set_cell((x, k as u16), &Cell::narrow(ch.to_string()));
        }
    }
    r.render(&mut sink, &mut rb2).unwrap();
    let out2 = String::from_utf8_lossy(&sink).to_string();
    println!("FRAME2 out = {out2:?}");
    // Force-clear: must walk back from (0,4) to the new top via
    // CUU 4, then erase (ED) so the lower 'D'/'E' rows from frame 1
    // can't survive on the physical screen.
    assert!(
        out2.contains("\x1b[4A"),
        "frame 2: expected CUU 4 from old bottom (y=4) to new top, got {out2:?}"
    );
    assert!(
        out2.contains("\x1b[J") || out2.contains("\x1b[0J") || out2.contains("\x1b[2J"),
        "frame 2: expected ED on force-clear, got {out2:?}"
    );
    for ch in ['X', 'Y', 'Z'] {
        assert!(
            out2.contains(&ch.to_string().repeat(20)),
            "frame 2: missing run of 20×{ch:?}: {out2:?}"
        );
    }
    // No leftover 'D' / 'E' glyphs from rows 3,4 of frame 1.
    assert!(
        !out2.contains("DDDDDDDDDDDDDDDDDDDD"),
        "frame 2: phantom row 3 ('D' run) leaked: {out2:?}"
    );
    assert!(
        !out2.contains("EEEEEEEEEEEEEEEEEEEE"),
        "frame 2: phantom row 4 ('E' run) leaked: {out2:?}"
    );
    assert_eq!(r.cur.pos(), Position { y: 2, x: 0 });
    sink.clear();

    // ---- Frame 3: grow to 20x7, row k = 'P'+k. No force-clear. -----
    let mut rb3 = RenderBuffer::new(20, 7);
    fill(&mut rb3, 20, 7, b'P');
    r.render(&mut sink, &mut rb3).unwrap();
    let out3 = String::from_utf8_lossy(&sink).to_string();
    println!("FRAME3 out = {out3:?}");
    for k in 0..7u8 {
        let ch = (b'P' + k) as char;
        assert!(
            out3.contains(&ch.to_string().repeat(20)),
            "frame 3: missing run of 20×{ch:?} on row {k}: {out3:?}"
        );
    }
    // None of the previous frame's X/Y/Z runs may show up — those
    // rows were rewritten with P/Q/R.
    for ch in ['X', 'Y', 'Z'] {
        assert!(
            !out3.contains(&ch.to_string().repeat(20)),
            "frame 3: phantom run of 20×{ch:?} leaked: {out3:?}"
        );
    }
    assert_eq!(r.cur.pos(), Position { y: 6, x: 0 });
    sink.clear();

    // ---- Frame 4: shrink to 15x4, force-clear. row k = 'Q'+k. ------
    r.request_clear();
    let mut rb4 = RenderBuffer::new(15, 4);
    fill(&mut rb4, 15, 4, b'Q');
    r.render(&mut sink, &mut rb4).unwrap();
    let out4 = String::from_utf8_lossy(&sink).to_string();
    println!("FRAME4 out = {out4:?}");
    // Old bottom was y=6; force-clear must walk back via CUU 6.
    assert!(
        out4.contains("\x1b[6A"),
        "frame 4: expected CUU 6 from old bottom (y=6) to new top, got {out4:?}"
    );
    assert!(
        out4.contains("\x1b[J") || out4.contains("\x1b[0J") || out4.contains("\x1b[2J"),
        "frame 4: expected ED on force-clear, got {out4:?}"
    );
    for k in 0..4u8 {
        let ch = (b'Q' + k) as char;
        assert!(
            out4.contains(&ch.to_string().repeat(15)),
            "frame 4: missing run of 15×{ch:?} on row {k}: {out4:?}"
        );
    }
    // No 20-wide phantom runs from frame 3 (rows are now 15 wide,
    // a 20-run of 'P'/'V' would imply the old surface is leaking).
    for ch in ['P', 'Q', 'R', 'S', 'T', 'U', 'V'] {
        assert!(
            !out4.contains(&ch.to_string().repeat(20)),
            "frame 4: 20-wide phantom run of {ch:?} from frame 3 leaked: {out4:?}"
        );
    }
    assert_eq!(r.cur.pos(), Position { y: 3, x: 0 });
    sink.clear();

    // ---- Frame 5: grow to 25x6, row k = 'R'+k. No force-clear. -----
    let mut rb5 = RenderBuffer::new(25, 6);
    fill(&mut rb5, 25, 6, b'R');
    r.render(&mut sink, &mut rb5).unwrap();
    let out5 = String::from_utf8_lossy(&sink).to_string();
    println!("FRAME5 out = {out5:?}");
    for k in 0..6u8 {
        let ch = (b'R' + k) as char;
        assert!(
            out5.contains(&ch.to_string().repeat(25)),
            "frame 5: missing run of 25×{ch:?} on row {k}: {out5:?}"
        );
    }
    // No 15-wide Q/R/S/T leftovers (those would mean the planner
    // diffed against the old 15-col surface and left tail columns
    // unwritten).
    assert!(
        !out5.contains(&"Q".repeat(15)) || out5.contains(&"Q".repeat(25)),
        "frame 5: 15-wide phantom Q run from frame 4 leaked: {out5:?}"
    );
    assert_eq!(r.cur.pos(), Position { y: 5, x: 0 });
}

/// `clear_bottom` emits ED to wipe trailing rows on the wire when
/// `new_buf` has more trailing-blank rows than `cur_buf`. After that
/// erase, the next frame must repaint those rows when their content
/// returns — the previously-buggy path skipped them because
/// `transform_line` saw `cur_buf` still holding the old (now wiped
/// off-screen) content and decided "unchanged".
///
/// Scenario: 10×8 surface. Frame 1 paints scrim across all 8 rows.
/// Frame 2 clears and repaints only rows 0-3, so rows 4-7 collapse
/// to trailing blanks (wiped by ED). Frame 3 paints scrim across all
/// 8 rows again — rows 4-7 must reappear.
#[test]
fn test_clear_bottom_syncs_cur_buf_so_next_frame_repaints() {
    use crate::color::{BasicColor, Color};
    use crate::style::Style;

    let mut r = Renderer::new();
    r.set_fullscreen(false);

    let scrim = Cell::narrow(" ").style(Style::default().bg(Color::Basic(BasicColor::Blue)));

    // Frame 1: scrim across all 8 rows.
    let mut rb1 = RenderBuffer::new(10, 8);
    for y in 0..8 {
        for x in 0..10 {
            rb1.set_cell((x, y), &scrim);
        }
    }
    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb1).unwrap();
    sink.clear();

    // Frame 2: only rows 0-3 have content, rows 4-7 are blank.
    // `clear_bottom` should emit ED to wipe rows 4-7 from the wire.
    let mut rb2 = RenderBuffer::new(10, 8);
    for y in 0..4 {
        for x in 0..10 {
            rb2.set_cell((x, y), &scrim);
        }
    }
    r.render(&mut sink, &mut rb2).unwrap();
    sink.clear();

    // Frame 3: scrim across all 8 rows again. Rows 4-7 were wiped
    // off-screen by frame 2's ED — they must be repainted now.
    let mut rb3 = RenderBuffer::new(10, 8);
    for y in 0..8 {
        for x in 0..10 {
            rb3.set_cell((x, y), &scrim);
        }
    }
    r.render(&mut sink, &mut rb3).unwrap();
    let out = String::from_utf8_lossy(&sink).to_string();

    // Rows 4-7 must be re-emitted: each produces an EL or explicit
    // cell content. `\e[K` (EL) appears for every row painted from
    // scratch in this scenario; require at least 4 occurrences so we
    // know all four wiped rows came back.
    let el_count = out.matches("\x1b[K").count();
    assert!(
        el_count >= 4,
        "rows 4-7 must be repainted after prior ED wiped them: out={out:?}"
    );
}

/// Regression: `clear_bottom` must NOT emit a redundant ED when
/// `cur_buf` is already blank wherever `new_buf` has trailing blanks
/// (e.g., the very first frame after init, or any frame following a
/// force-clear that wiped `cur_buf`). The optimiser should only fire
/// when there's stale content on screen to wipe; firing whenever
/// `new_buf` has trailing blanks regardless of `cur_buf` state wastes
/// bytes on a no-op ED.
#[test]
fn test_clear_bottom_skips_ed_when_cur_buf_already_blank() {
    let mut r = Renderer::new();
    r.set_fullscreen(false);

    // First frame: cur_buf is freshly initialised (all blank). New
    // frame has content rows 0-2 and trailing blanks rows 3-7.
    let mut rb = RenderBuffer::new(10, 8);
    let glyph = Cell::narrow("x");
    for y in 0..3 {
        for x in 0..10 {
            rb.set_cell((x, y), &glyph);
        }
    }

    let mut sink = Vec::new();
    r.render(&mut sink, &mut rb).unwrap();
    let out = String::from_utf8_lossy(&sink).to_string();

    // No ED-below should fire — cur_buf was already blank in rows 3-7.
    assert!(
        !out.contains("\x1b[J") && !out.contains("\x1b[0J"),
        "redundant ED-below emitted for cur_buf that was already blank: out={out:?}"
    );
}

/// Regression: in inline (relative) mode, after the cursor is invalidated
/// (e.g. a suspend/resume shell handoff, possibly with a resize reflow), the
/// next move must RE-ANCHOR at the current physical row — emit a bare `\r` and
/// step only downward — instead of stepping UP from the stale tracked row and
/// overwriting content above the surface. Mirrors how Bubble Tea / ultraviolet
/// treat an unknown `(-1,-1)` cursor as `(0,0)`.
#[test]
fn inline_move_after_invalidate_reanchors_without_cursor_up() {
    let mut r = Renderer::new(); // inline / relative by default
    r.last_width = 20;
    r.last_height = 5;
    // A previous frame left the cursor parked at the bottom row.
    r.cur.set_pos(Position { y: 4, x: 0 });

    // Shell handoff voids our position model.
    r.invalidate_cursor();
    assert!(r.cur.x.is_none() && r.cur.y.is_none());

    // The resume repaint moves to the top of the surface.
    let buf = RenderBuffer::new(20, 5);
    let mut out = Vec::new();
    r.move_to(&mut out, &buf, 0, 0).unwrap();
    let s = String::from_utf8(out).unwrap();

    // It must NOT emit any cursor-up (`CUU`, `ESC [ <n> A`) — that is the
    // overshoot that "ate" the previous line. It re-anchors with `\r` and
    // treats the current row as the new top.
    assert!(
        !s.contains('A'),
        "must not emit CUU on inline re-anchor: {s:?}"
    );
    assert!(s.contains('\r'), "should re-home the column with CR: {s:?}");
    assert_eq!(r.cur.pos(), Position { y: 0, x: 0 });
}

/// Contrast: with a KNOWN cursor at the bottom, moving to the top in inline
/// mode does step up (`CUU`/`LF`-equivalent) — confirming the re-anchor above
/// is specifically driven by the invalidated (unknown) state.
#[test]
fn inline_move_with_known_cursor_steps_up() {
    let mut r = Renderer::new();
    r.last_width = 20;
    r.last_height = 5;
    r.cur.set_pos(Position { y: 4, x: 0 });

    let buf = RenderBuffer::new(20, 5);
    let mut out = Vec::new();
    r.move_to(&mut out, &buf, 0, 0).unwrap();
    let s = String::from_utf8(out).unwrap();
    // A real upward move is emitted (CUU) since the cursor was known.
    assert!(
        s.contains('A'),
        "expected an upward move from a known cursor: {s:?}"
    );
    assert_eq!(r.cur.pos(), Position { y: 0, x: 0 });
}
