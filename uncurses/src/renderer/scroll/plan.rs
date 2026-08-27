//! Per-hunk scroll strategy dispatcher.
//!
//! Walks the direct → DECSTBM-bracketed → DL+IL fallback chain for a
//! single contiguous run of rows that need to move by the same shift.
//! Gated by `Optimizations::CSR` and `Optimizations::IL_DL`. On
//! success, applies the cur_buf shift, touches the affected rows so
//! the per-row transform pass revisits them, and shifts/recomputes
//! `old_hashes` for the next frame's scroll detection.

use std::io;

use crate::ansi;
use crate::cell::Cell;
use crate::renderer::caps::Optimizations;
use crate::renderer::{RenderBuffer, Renderer};

use super::emit::{scroll_down, scroll_idl, scroll_up};
use super::verify::{recompute_blank_hashes, shift_old_hashes_down, shift_old_hashes_up};

pub(super) fn scrolln(
    out: &mut Vec<u8>,
    renderer: &mut Renderer,
    new_buf: &mut RenderBuffer,
    n: i32,
    top: usize,
    bot: usize,
    max_y: usize,
) -> io::Result<()> {
    if n == 0 || top > bot || bot > max_y {
        return Ok(());
    }

    // `scroll_idl` simulates a scroll with DL+IL and no scrolling region,
    // so every row below `bot` moves up and back within the frame. It
    // lands correct and emits no corrective bytes, which means no
    // row-level check can see it -- but unsynchronized the bounce is on
    // screen. Refuse it there unless the region reaches the last row, in
    // which case nothing sits below it to bounce.
    let idl_ok = renderer.sync_output || bot == max_y;

    let mut v;
    if n > 0 {
        let amt = n as u16;
        v = scroll_up(out, renderer, new_buf, amt, top, bot, 0, max_y)?;
        if !v && renderer.opts.contains(Optimizations::CSR) {
            ansi::screen::write_scroll_region(out, top as u16, bot as u16)?;
            renderer.invalidate_cursor();
            v = scroll_up(out, renderer, new_buf, amt, top, bot, top, bot)?;
            ansi::screen::write_reset_scroll_region(out)?;
            renderer.invalidate_cursor();
        }
        if !v && idl_ok && renderer.opts.contains(Optimizations::IL_DL) {
            v = scroll_idl(out, renderer, new_buf, amt, top, bot + 1 - amt as usize)?;
        }
    } else {
        let amt = (-n) as u16;
        v = scroll_down(out, renderer, new_buf, amt, top, bot, 0, max_y)?;
        if !v && renderer.opts.contains(Optimizations::CSR) {
            ansi::screen::write_scroll_region(out, top as u16, bot as u16)?;
            renderer.invalidate_cursor();
            v = scroll_down(out, renderer, new_buf, amt, top, bot, top, bot)?;
            ansi::screen::write_reset_scroll_region(out)?;
            renderer.invalidate_cursor();
        }
        if !v && idl_ok && renderer.opts.contains(Optimizations::IL_DL) {
            v = scroll_idl(out, renderer, new_buf, amt, bot + 1 - amt as usize, top)?;
        }
    }

    if !v {
        return Ok(());
    }

    // The buffer / hash bookkeeping uses an exclusive bottom; convert
    // once here. delete_lines / insert_lines fill the freed rows with
    // the bg-only blank that BCE actually paints on the wire (or a
    // default blank when BCE is off), so cur_buf matches the screen
    // exactly and the per-row diff sees no work to do for those rows.
    let bottom_excl = bot + 1;
    // Split-borrow: `renderer.cur` is disjoint from `renderer.cur_buf`,
    // so the bce_blank ref lives across the cur_buf mutation below.
    let bce = renderer.opts.contains(Optimizations::BCE);
    let bce_fill: &Cell = renderer.cur.bce_blank(bce);
    if let Some(cb) = renderer.cur_buf.as_mut() {
        if n > 0 {
            cb.delete_lines(top as u16, n as u16, bottom_excl as u16, bce_fill);
        } else {
            cb.insert_lines(top as u16, (-n) as u16, bottom_excl as u16, bce_fill);
        }
    }

    // Force `transform_line` to revisit every row in the scrolled
    // region so the cell-by-cell diff against the post-scroll cur_buf
    // re-emits whatever actually differs (notably the rows that were
    // blanked at the region edges).
    for y in top..=bot {
        new_buf.touch_full_line(y as u16);
    }

    // Keep `old_hashes` aligned with cur_buf: shift the stable hash
    // segment, then recompute hashes for the rows that were blanked
    // directly from cur_buf so detection on the next frame sees the
    // actual blank content rather than a stale or zero hash.
    if n > 0 {
        let amt = n as usize;
        shift_old_hashes_up(&mut renderer.old_hashes, top, bottom_excl, amt);
        recompute_blank_hashes(renderer, bottom_excl.saturating_sub(amt), bottom_excl);
    } else {
        let amt = (-n) as usize;
        shift_old_hashes_down(&mut renderer.old_hashes, top, bottom_excl, amt);
        recompute_blank_hashes(renderer, top, top + amt);
    }

    Ok(())
}
