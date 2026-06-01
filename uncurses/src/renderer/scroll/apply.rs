//! Top-level scroll application entry point.
//!
//! Walks the planning input twice — top-down for upward shifts,
//! bottom-up for downward — collecting each contiguous run of rows
//! that move by the same shift into a hunk and handing it off to the
//! per-hunk dispatcher in [`super::plan::scrolln`].
//!
//! The hunk dispatcher walks a three-strategy fallback chain:
//!
//! 1. **Direct** — try a single SU/SD/DL/IL/LF/RI without DECSTBM.
//!    Only succeeds when the region's bottom row is the screen bottom
//!    (or the region spans the whole screen) so the byte stream
//!    affects only rows the planner intends to move.
//! 2. **DECSTBM-bracketed** — set the scrolling region with `\x1b[t;br`,
//!    retry the same direct emitter with `min_y/max_y` collapsed onto
//!    the region edges so any of its branches becomes valid, reset
//!    the region with `\x1b[r`. The cursor is invalidated around both
//!    DECSTBM bytes so the next move reasserts position via CUP — VT
//!    spec puts DECOM-driven cursor homing in DECSTBM and we cannot
//!    tell which DECOM the terminal is in.
//! 3. **DL+IL pair** — `scroll_idl` issues a DL at one row and an IL
//!    at another to simulate the scroll without DECSTBM.
//!
//! The branch emitters in [`super::emit`] each implement a
//! five-branch priority tree (`n==1 && full` → LF/RI, `n==1 &&
//! bot==max_y` → DL(1)/IL(1), `full` → SU/SD or repeated LF/RI,
//! `bot==max_y` → DL(n)/IL(n), else return false) so the dispatcher's
//! two retries reuse the same code with different `(min_y, max_y)`
//! bounds. Post-scroll hash bookkeeping lives in [`super::verify`].

use std::io;

use crate::renderer::{RenderBuffer, Renderer};

use super::plan::scrolln;

/// Apply detected scroll operations to minimize redraws. Walks
/// `renderer.oldnum` twice — top-down for upward shifts, bottom-up
/// for downward — and dispatches each contiguous hunk to
/// [`super::plan::scrolln`].
pub(crate) fn apply_scrolls(
    out: &mut Vec<u8>,
    renderer: &mut Renderer,
    new_buf: &mut RenderBuffer,
) -> io::Result<()> {
    let height = renderer.last_height as usize;
    if height == 0 || renderer.oldnum.len() < height {
        return Ok(());
    }

    // Take oldnum out of the renderer so the loop can mutate the
    // renderer freely (move_cursor / update_pen are &mut self) while
    // we keep an indexable view of the planning input. mem::take
    // leaves an empty Vec behind; we restore the original at the end.
    let mut oldnum = std::mem::take(&mut renderer.oldnum);
    let h = height as i32;
    let max_y = height - 1;

    let result = (|| -> io::Result<()> {
        // Pass 1: top→bottom, scroll up (shift > 0). Includes the source
        // rows below the destination run by extending `bot` to
        // `i - 1 + shift` so the scroll byte covers the cells SU has to
        // pull up into the destination span.
        let mut i: i32 = 0;
        loop {
            while i < h && (oldnum[i as usize] < 0 || oldnum[i as usize] <= i) {
                i += 1;
            }
            if i >= h {
                break;
            }
            let shift = oldnum[i as usize] - i;
            let start = i as usize;
            i += 1;
            while i < h && oldnum[i as usize] >= 0 && oldnum[i as usize] - i == shift {
                i += 1;
            }
            let bot = ((i - 1 + shift) as usize).min(max_y);
            scrolln(out, renderer, new_buf, shift, start, bot, max_y)?;
        }

        // Pass 2: bottom→top, scroll down (shift < 0). Symmetric: extend
        // `top` upward by `-shift` to include the source rows above.
        let mut i: i32 = h - 1;
        loop {
            while i >= 0 && (oldnum[i as usize] < 0 || oldnum[i as usize] >= i) {
                i -= 1;
            }
            if i < 0 {
                break;
            }
            let shift = oldnum[i as usize] - i;
            let end = i as usize;
            i -= 1;
            while i >= 0 && oldnum[i as usize] >= 0 && oldnum[i as usize] - i == shift {
                i -= 1;
            }
            let start = (i + 1 - (-shift)).max(0) as usize;
            scrolln(out, renderer, new_buf, shift, start, end, max_y)?;
        }

        Ok(())
    })();

    // Restore oldnum regardless of whether the planning loop errored
    // so the renderer can reuse the allocation next frame. scrolln
    // doesn't touch oldnum (it operates through cur_buf / new_buf
    // helpers), so the snapshot we took is still authoritative.
    std::mem::swap(&mut renderer.oldnum, &mut oldnum);
    result
}

#[cfg(test)]
mod tests {
    use super::super::plan::scrolln;
    use super::super::verify::{shift_old_hashes_down, shift_old_hashes_up};
    use super::*;
    use crate::Position;
    use crate::renderer::Optimizations;
    use crate::renderer::frame::prepare::hash_line;

    fn make_renderer(width: u16, height: u16, opts: Optimizations) -> Renderer {
        let mut r = Renderer::new();
        r.set_optimizations(opts);
        r.last_width = width;
        r.last_height = height;
        r.cur_buf = Some(RenderBuffer::new(width, height));
        r.old_hashes = vec![0u64; height as usize];
        r.cur.x_unknown = false;
        r.cur.y_unknown = false;
        r
    }

    #[test]
    fn shift_up_blanks_bottom() {
        let mut h = vec![10, 11, 12, 13, 14, 15];
        shift_old_hashes_up(&mut h, 1, 5, 2);
        assert_eq!(h, vec![10, 13, 14, 0, 0, 15]);
    }

    #[test]
    fn shift_down_blanks_top() {
        let mut h = vec![10, 11, 12, 13, 14, 15];
        shift_old_hashes_down(&mut h, 1, 5, 2);
        assert_eq!(h, vec![10, 0, 0, 11, 12, 15]);
    }

    #[test]
    fn shift_zero_or_empty_is_noop() {
        let mut h = vec![1, 2, 3];
        shift_old_hashes_up(&mut h, 0, 3, 0);
        assert_eq!(h, vec![1, 2, 3]);
        shift_old_hashes_down(&mut h, 0, 3, 0);
        assert_eq!(h, vec![1, 2, 3]);
        shift_old_hashes_up(&mut h, 2, 2, 1);
        assert_eq!(h, vec![1, 2, 3]);
    }

    #[test]
    fn shift_larger_than_region_blanks_all() {
        let mut h = vec![10, 11, 12, 13, 14];
        shift_old_hashes_up(&mut h, 1, 4, 10);
        assert_eq!(h, vec![10, 0, 0, 0, 14]);
    }

    #[test]
    fn scrolln_up_il_dl_pairs_dl_and_il_in_partial_region() {
        // No csr, no su_sd, only il_dl: a partial-region
        // scroll up falls through both the direct and DECSTBM paths
        // and lands on scroll_idl, which emits DL at top and IL at
        // bot+1-n.
        let opts = Optimizations::default()
            .union(Optimizations::IL_DL)
            .difference(Optimizations::CSR | Optimizations::SU_SD);
        let mut renderer = make_renderer(10, 8, opts);
        let mut new_buf = RenderBuffer::new(10, 8);
        new_buf.clear_touched();
        let mut out = Vec::new();

        // region rows 2..=5 (inclusive), shift up by 2
        scrolln(&mut out, &mut renderer, &mut new_buf, 2, 2, 5, 7).unwrap();

        let s = String::from_utf8(out).expect("ascii");
        assert!(s.contains("\x1b[2M"), "expected DL(2) somewhere: {s:?}");
        assert!(s.contains("\x1b[2L"), "expected IL(2) somewhere: {s:?}");
        let dl_at = s.find("\x1b[2M").unwrap();
        let il_at = s.find("\x1b[2L").unwrap();
        assert!(dl_at < il_at, "DL must precede IL in the paired fallback");
        // ins = bot+1-n = 5+1-2 = 4
        assert_eq!(renderer.cur.pos, Position { y: 4, x: 0 });
    }

    #[test]
    fn scrolln_down_il_dl_pairs_dl_and_il_in_partial_region() {
        let opts = Optimizations::default()
            .union(Optimizations::IL_DL)
            .difference(Optimizations::CSR | Optimizations::SU_SD);
        let mut renderer = make_renderer(10, 8, opts);
        let mut new_buf = RenderBuffer::new(10, 8);
        new_buf.clear_touched();
        let mut out = Vec::new();

        // region rows 2..=5, shift down by 2
        scrolln(&mut out, &mut renderer, &mut new_buf, -2, 2, 5, 7).unwrap();

        let s = String::from_utf8(out).expect("ascii");
        assert!(s.contains("\x1b[2M"), "expected DL(2) somewhere: {s:?}");
        assert!(s.contains("\x1b[2L"), "expected IL(2) somewhere: {s:?}");
        let dl_at = s.find("\x1b[2M").unwrap();
        let il_at = s.find("\x1b[2L").unwrap();
        assert!(
            dl_at < il_at,
            "DL must precede IL in the paired fallback (so the bottom rows are dropped before the region shifts down)",
        );
        // ins = top = 2
        assert_eq!(renderer.cur.pos, Position { y: 2, x: 0 });
    }

    #[test]
    fn scrolln_up_marks_new_buf_rows_touched() {
        let opts = Optimizations::default()
            .union(Optimizations::CSR | Optimizations::IL_DL | Optimizations::SU_SD);
        let mut renderer = make_renderer(10, 5, opts);
        let mut new_buf = RenderBuffer::new(10, 5);
        new_buf.clear_touched();
        let mut out = Vec::new();

        // region rows 1..=3 inclusive, shift up by 1
        scrolln(&mut out, &mut renderer, &mut new_buf, 1, 1, 3, 4).unwrap();

        assert!(new_buf.touched(0).is_none(), "row 0 outside region");
        for y in 1..4u16 {
            assert!(
                new_buf.touched(y).is_some(),
                "row {y} inside region must be touched",
            );
        }
        assert!(new_buf.touched(4).is_none(), "row 4 outside region");
    }

    #[test]
    fn scrolln_down_marks_new_buf_rows_touched() {
        let opts = Optimizations::default()
            .union(Optimizations::CSR | Optimizations::IL_DL | Optimizations::SU_SD);
        let mut renderer = make_renderer(10, 5, opts);
        let mut new_buf = RenderBuffer::new(10, 5);
        new_buf.clear_touched();
        let mut out = Vec::new();

        scrolln(&mut out, &mut renderer, &mut new_buf, -1, 1, 3, 4).unwrap();

        assert!(new_buf.touched(0).is_none(), "row 0 outside region");
        for y in 1..4u16 {
            assert!(
                new_buf.touched(y).is_some(),
                "row {y} inside region must be touched",
            );
        }
        assert!(new_buf.touched(4).is_none(), "row 4 outside region");
    }

    #[test]
    fn scrolln_up_il_dl_skips_preemptive_pen_reset() {
        // With a styled pen, scrolln must NOT emit a full SGR reset
        // before DL. The exposed rows get BCE-painted with the
        // current bg, and delete_lines fills cur_buf with the same
        // blank, so model and wire stay in sync without a reset.
        let opts = Optimizations::default()
            .union(Optimizations::IL_DL)
            .difference(Optimizations::CSR | Optimizations::SU_SD);
        let mut renderer = make_renderer(10, 8, opts);
        renderer.cur.set_style(
            crate::style::Style::default()
                .with_bg(crate::color::Color::Basic(crate::color::BasicColor::Red)),
        );
        let mut new_buf = RenderBuffer::new(10, 8);
        new_buf.clear_touched();
        let mut out = Vec::new();

        scrolln(&mut out, &mut renderer, &mut new_buf, 2, 2, 5, 7).unwrap();

        let s = String::from_utf8(out).expect("ascii");
        let dl_at = s.find("\x1b[2M").expect("expected DL in output");
        let before_dl = &s[..dl_at];
        assert!(
            !before_dl.contains("\x1b[0m") && !before_dl.contains("\x1b[m"),
            "no full SGR reset should precede DL: {s:?}"
        );
    }

    #[test]
    fn scrolln_up_recomputes_blanked_row_hashes() {
        let opts = Optimizations::default()
            .union(Optimizations::CSR)
            .difference(Optimizations::IL_DL | Optimizations::SU_SD);
        let mut renderer = make_renderer(10, 6, opts);
        renderer.old_hashes = vec![100, 200, 300, 400, 500, 600];
        let mut new_buf = RenderBuffer::new(10, 6);
        new_buf.clear_touched();
        let mut out = Vec::new();

        // Full-screen scroll up by 2 — rows 4 and 5 become blank in
        // cur_buf. Their old_hashes entries must be recomputed from
        // the actual (now-blank) cur_buf content, not left at zero.
        scrolln(&mut out, &mut renderer, &mut new_buf, 2, 0, 5, 5).unwrap();

        let cb = renderer.cur_buf.as_ref().unwrap();
        let blank_hash = hash_line(cb.line(4).unwrap());
        assert_eq!(renderer.old_hashes[4], blank_hash);
        assert_eq!(renderer.old_hashes[5], blank_hash);
        assert_eq!(renderer.old_hashes[0], 300);
        assert_eq!(renderer.old_hashes[1], 400);
    }

    #[test]
    fn scrolln_down_recomputes_blanked_row_hashes() {
        let opts = Optimizations::default()
            .union(Optimizations::CSR)
            .difference(Optimizations::IL_DL | Optimizations::SU_SD);
        let mut renderer = make_renderer(10, 6, opts);
        renderer.old_hashes = vec![100, 200, 300, 400, 500, 600];
        let mut new_buf = RenderBuffer::new(10, 6);
        new_buf.clear_touched();
        let mut out = Vec::new();

        scrolln(&mut out, &mut renderer, &mut new_buf, -2, 0, 5, 5).unwrap();

        let cb = renderer.cur_buf.as_ref().unwrap();
        let blank_hash = hash_line(cb.line(0).unwrap());
        assert_eq!(renderer.old_hashes[0], blank_hash);
        assert_eq!(renderer.old_hashes[1], blank_hash);
        assert_eq!(renderer.old_hashes[2], 100);
        assert_eq!(renderer.old_hashes[3], 200);
    }

    #[test]
    fn apply_scrolls_expands_region_to_cover_source_rows_up() {
        // Plan: oldnum = [2, 3, 4, -1, -1, -1] (height=6).
        // Destination rows 0..=2 want content at rows 2..=4. The
        // scroll byte must cover rows 0..=4 so SU(2) actually pulls
        // the source content into the destination span.
        let opts = Optimizations::default()
            .union(Optimizations::CSR | Optimizations::SU_SD)
            .difference(Optimizations::IL_DL);
        let mut renderer = make_renderer(10, 6, opts);
        *renderer.oldnum = vec![2, 3, 4, -1, -1, -1];
        let mut new_buf = RenderBuffer::new(10, 6);
        new_buf.clear_touched();
        let mut out = Vec::new();

        apply_scrolls(&mut out, &mut renderer, &mut new_buf).unwrap();
        let s = String::from_utf8(out).expect("ascii");
        // Direct path fails (bot=4 != max_y=5), DECSTBM-bracketed
        // SU(2) wins. DECSTBM is 1-based: rows 0..=4 → "\x1b[1;5r".
        assert!(
            s.contains("\x1b[1;5r"),
            "expected expanded scroll region [1;5], got {s:?}",
        );
        assert!(s.contains("\x1b[2S"), "expected SU(2), got {s:?}");
    }

    #[test]
    fn apply_scrolls_expands_region_to_cover_source_rows_down() {
        // Symmetric: destination 3..=5 wants source 1..=3, scroll
        // region must span 1..=5. il_dl=false forces the DECSTBM+SD
        // path even though bot==max_y (the direct IL path is gated
        // on il_dl).
        let opts = Optimizations::default()
            .union(Optimizations::CSR | Optimizations::SU_SD)
            .difference(Optimizations::IL_DL);
        let mut renderer = make_renderer(10, 6, opts);
        *renderer.oldnum = vec![-1, -1, -1, 1, 2, 3];
        let mut new_buf = RenderBuffer::new(10, 6);
        new_buf.clear_touched();
        let mut out = Vec::new();

        apply_scrolls(&mut out, &mut renderer, &mut new_buf).unwrap();
        let s = String::from_utf8(out).expect("ascii");
        assert!(
            s.contains("\x1b[2;6r"),
            "expected expanded scroll region [2;6], got {s:?}",
        );
        assert!(s.contains("\x1b[2T"), "expected SD(2), got {s:?}");
    }

    #[test]
    fn apply_scrolls_uses_direct_il_when_bot_at_max_y() {
        // When bot==max_y AND il_dl is available, the direct path
        // wins for downward scrolls — IL(n) at top without DECSTBM
        // saves the bracket bytes.
        let opts = Optimizations::default()
            .union(Optimizations::CSR | Optimizations::IL_DL | Optimizations::SU_SD);
        let mut renderer = make_renderer(10, 6, opts);
        *renderer.oldnum = vec![-1, -1, -1, 1, 2, 3];
        let mut new_buf = RenderBuffer::new(10, 6);
        new_buf.clear_touched();
        let mut out = Vec::new();

        apply_scrolls(&mut out, &mut renderer, &mut new_buf).unwrap();
        let s = String::from_utf8(out).expect("ascii");
        assert!(s.contains("\x1b[2L"), "expected IL(2), got {s:?}");
        assert!(
            !s.contains("\x1b[2;6r"),
            "must not bracket DECSTBM when direct IL works: {s:?}",
        );
        assert!(
            !s.contains("\x1b[2T"),
            "must not emit SD when direct IL works: {s:?}",
        );
    }

    #[test]
    fn scrolln_up_region_falls_back_to_lf_without_su_sd() {
        // csr=true, su_sd=false: DECSTBM-bracketed path,
        // SU branch unavailable, falls through to repeated LF.
        let opts = Optimizations::default()
            .union(Optimizations::CSR)
            .difference(Optimizations::IL_DL | Optimizations::SU_SD);
        let mut renderer = make_renderer(10, 8, opts);
        // Position cursor at the bottom of the target region. After
        // DECSTBM emission cursor_unknown is set; the next move
        // emits an absolute CUP regardless of where we were.
        renderer.cur.pos = Position { y: 5, x: 0 };
        let mut new_buf = RenderBuffer::new(10, 8);
        new_buf.clear_touched();
        let mut out = Vec::new();

        // region rows 2..=5, shift up by 2
        scrolln(&mut out, &mut renderer, &mut new_buf, 2, 2, 5, 7).unwrap();

        let s = String::from_utf8(out).expect("ascii");
        assert!(s.contains("\x1b[3;6r"), "expected DECSTBM [3;6], got {s:?}");
        assert!(!s.contains("\x1b[2S"), "must not emit SU(2): {s:?}");
        let lf_count = s.bytes().filter(|&b| b == b'\n').count();
        assert_eq!(lf_count, 2, "expected 2 LFs, got {lf_count} in {s:?}");
        assert!(s.contains("\x1b[r"), "expected DECSTBM reset: {s:?}");
    }

    #[test]
    fn scrolln_down_region_falls_back_to_ri_without_su_sd() {
        let opts = Optimizations::default()
            .union(Optimizations::CSR)
            .difference(Optimizations::IL_DL | Optimizations::SU_SD);
        let mut renderer = make_renderer(10, 8, opts);
        renderer.cur.pos = Position { y: 2, x: 0 };
        let mut new_buf = RenderBuffer::new(10, 8);
        new_buf.clear_touched();
        let mut out = Vec::new();

        scrolln(&mut out, &mut renderer, &mut new_buf, -2, 2, 5, 7).unwrap();

        let s = String::from_utf8(out).expect("ascii");
        assert!(s.contains("\x1b[3;6r"), "expected DECSTBM [3;6], got {s:?}");
        assert!(!s.contains("\x1b[2T"), "must not emit SD(2): {s:?}");
        let ri_count = s.matches("\x1bM").count();
        assert_eq!(ri_count, 2, "expected 2 RIs, got {ri_count} in {s:?}");
    }

    #[test]
    fn scrolln_up_paints_freed_rows_with_styled_clear_blank() {
        // When the working pen carries a non-default background, the
        // scroll byte must be emitted with that pen so BCE pre-fills
        // the exposed row, and cur_buf must record the styled blank
        // so the subsequent diff sees the row as already-correct.
        use crate::cell::Cell;
        use crate::color::{BasicColor, Color};
        use crate::style::Style;

        let opts = Optimizations::default()
            .difference(Optimizations::CSR | Optimizations::IL_DL | Optimizations::SU_SD);
        let mut renderer = make_renderer(10, 4, opts);
        let bg_style = Style::EMPTY.with_bg(Color::Basic(BasicColor::Blue));
        renderer.cur.set_style(bg_style.clone());
        renderer.cur.pos = Position { y: 3, x: 0 };
        let mut new_buf = RenderBuffer::new(10, 4);
        new_buf.clear_touched();
        let mut out = Vec::new();

        // Full-screen scroll up by 1 — direct path's n==1 LF branch
        // wins at top==min_y && bot==max_y.
        scrolln(&mut out, &mut renderer, &mut new_buf, 1, 0, 3, 3).unwrap();

        let cb = renderer.cur_buf.as_ref().unwrap();
        let bottom_row = cb.line(3).unwrap();
        let expected = Cell::BLANK.with_style(bg_style);
        assert!(
            bottom_row.iter().all(|c| *c == expected),
            "cur_buf bottom row must be styled-blank, got: {bottom_row:?}",
        );
        assert_ne!(
            bottom_row[0],
            Cell::BLANK,
            "must not record default-blank when clear_blank is styled",
        );
    }

    #[test]
    fn scrolln_strips_non_bg_attrs_from_freed_rows() {
        // BCE on the wire only paints the bg into freed cells — bold,
        // fg, underline, etc. are dropped. cur_buf's fill must match
        // that or the next-frame diff will think those rows already
        // carry attrs the screen never received.
        use crate::cell::Cell;
        use crate::color::{BasicColor, Color};
        use crate::style::Style;

        let opts = Optimizations::default()
            .difference(Optimizations::CSR | Optimizations::IL_DL | Optimizations::SU_SD);
        let mut renderer = make_renderer(10, 4, opts);
        let pen = Style::EMPTY
            .with_bg(Color::Basic(BasicColor::Red))
            .with_fg(Color::Basic(BasicColor::White))
            .with_bold();
        renderer.cur.set_style(pen);
        renderer.cur.mark_pen_changed();
        renderer.cur.pos = Position { y: 3, x: 0 };
        let mut new_buf = RenderBuffer::new(10, 4);
        new_buf.clear_touched();
        let mut out = Vec::new();

        scrolln(&mut out, &mut renderer, &mut new_buf, 1, 0, 3, 3).unwrap();

        let cb = renderer.cur_buf.as_ref().unwrap();
        let bottom_row = cb.line(3).unwrap();
        let bg_only = Style::EMPTY.with_bg(Color::Basic(BasicColor::Red));
        let expected = Cell::BLANK.with_style(bg_only);
        assert!(
            bottom_row.iter().all(|c| *c == expected),
            "cur_buf bottom row must be bg-only, got: {bottom_row:?}",
        );
        assert!(
            bottom_row.iter().all(|c| c.style().fg.is_none()
                && c.style().attrs.is_empty()
                && c.style().underline_color.is_none()),
            "freed cells must drop fg / attrs / underline_color"
        );
    }

    #[test]
    fn scrolln_without_bce_freed_rows_are_default_blank() {
        // Without BCE the scroll byte paints freed cells with the
        // terminal's default bg, so cur_buf's fill must be the
        // default Cell::BLANK regardless of the active pen.
        use crate::cell::Cell;
        use crate::color::{BasicColor, Color};
        use crate::style::Style;

        let opts = Optimizations::default()
            .difference(Optimizations::BCE | Optimizations::CSR | Optimizations::SU_SD);
        let mut renderer = make_renderer(10, 4, opts);
        renderer
            .cur
            .set_style(Style::EMPTY.with_bg(Color::Basic(BasicColor::Red)));
        renderer.cur.mark_pen_changed();
        renderer.cur.pos = Position { y: 3, x: 0 };
        let mut new_buf = RenderBuffer::new(10, 4);
        new_buf.clear_touched();
        let mut out = Vec::new();

        scrolln(&mut out, &mut renderer, &mut new_buf, 1, 0, 3, 3).unwrap();

        let cb = renderer.cur_buf.as_ref().unwrap();
        let bottom_row = cb.line(3).unwrap();
        assert!(
            bottom_row.iter().all(|c| *c == Cell::BLANK),
            "without BCE, freed cells must be default-blank, got: {bottom_row:?}",
        );
    }

    #[test]
    fn scrolln_invalidates_cursor_after_decstbm_brackets() {
        // After both DECSTBM bytes (set and reset) the next move
        // must emit absolute CUP, not a relative move from a tracked
        // position the terminal may not actually be at (DECOM-set
        // terminals home the cursor on DECSTBM, DECOM-reset don't).
        let opts = Optimizations::default()
            .union(Optimizations::CSR | Optimizations::SU_SD)
            .difference(Optimizations::IL_DL);
        let mut renderer = make_renderer(10, 6, opts);
        // Force absolute mode so the invalidation path emits CUP.
        renderer.set_relative_cursor(false);
        renderer.cur.pos = Position { y: 0, x: 0 };
        let mut new_buf = RenderBuffer::new(10, 6);
        new_buf.clear_touched();
        let mut out = Vec::new();

        // Partial-region scroll up: forces direct path to fail and
        // DECSTBM-bracketed path to run.
        scrolln(&mut out, &mut renderer, &mut new_buf, 2, 1, 4, 5).unwrap();

        // After scrolln returns, both axes must be marked unknown so
        // the very next move_cursor reasserts position via CUP /
        // \r-snap.
        assert!(
            renderer.cur.x_unknown && renderer.cur.y_unknown,
            "scrolln must invalidate cursor after DECSTBM reset",
        );
    }
}
