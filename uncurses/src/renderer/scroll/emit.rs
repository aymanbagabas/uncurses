//! Byte-level scroll emitters. Each function tries to issue a single
//! scroll byte sequence covering rows `[top, bot]` within the surface
//! bounds `[min_y, max_y]`, returning `true` on success and `false`
//! when no branch applies so the caller can fall through to the next
//! strategy.

use std::io;

use crate::ansi;
use crate::renderer::caps::Optimizations;
use crate::renderer::{RenderBuffer, Renderer};

/// Try to emit a single scroll-up byte sequence covering rows
/// `[top, bot]`, given that the operation must work within the
/// surface bounds `[min_y, max_y]`. Returns `true` if a sequence was
/// emitted and `false` if no branch applies (caller falls through to
/// the next strategy).
///
/// Branches are tried in priority order:
///   1. `n==1 && top==min_y && bot==max_y`: bare LF at the region
///      bottom is one byte and scrolls the surface.
///   2. `n==1 && bot==max_y`: DL(1) at `top` deletes the top row of
///      the region; rows below shift up; the bottom row blanks.
///   3. `top==min_y && bot==max_y`: SU(n) at the bottom (or repeated
///      LF if SU is unavailable).
///   4. `bot==max_y`: DL(n) at `top`.
///   5. Otherwise: false.
///
/// Branches 2 and 4 require `opts.contains(Optimizations::IL_DL)`; branch 3's SU requires
/// `opts.contains(Optimizations::SU_SD)` (its LF fallback is unconditional).
pub(super) fn scroll_up(
    out: &mut Vec<u8>,
    renderer: &mut Renderer,
    new_buf: &RenderBuffer,
    n: u16,
    top: usize,
    bot: usize,
    min_y: usize,
    max_y: usize,
) -> io::Result<bool> {
    if n == 1 && top == min_y && bot == max_y {
        renderer.move_to(out, new_buf, bot as u16, 0)?;
        out.push(b'\n');
        Ok(true)
    } else if n == 1 && bot == max_y && renderer.opts.contains(Optimizations::IL_DL) {
        renderer.move_to(out, new_buf, top as u16, 0)?;
        ansi::write_delete_lines(out, 1)?;
        Ok(true)
    } else if top == min_y && bot == max_y {
        renderer.move_to(out, new_buf, bot as u16, 0)?;
        if renderer.opts.contains(Optimizations::SU_SD) {
            ansi::write_scroll_up(out, n)?;
        } else {
            for _ in 0..n {
                out.push(b'\n');
            }
        }
        Ok(true)
    } else if bot == max_y && renderer.opts.contains(Optimizations::IL_DL) {
        renderer.move_to(out, new_buf, top as u16, 0)?;
        ansi::write_delete_lines(out, n)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Mirror of [`scroll_up`] for scrolling content downward: RI / IL(1)
/// / SD or repeated RI / IL(n). All branches move to `top` because RI
/// scrolls within the region only when the cursor sits at its top
/// row, and IL inserts blank rows at the cursor position pushing the
/// rest down (with the bottom rows clipped past `max_y`).
pub(super) fn scroll_down(
    out: &mut Vec<u8>,
    renderer: &mut Renderer,
    new_buf: &RenderBuffer,
    n: u16,
    top: usize,
    bot: usize,
    min_y: usize,
    max_y: usize,
) -> io::Result<bool> {
    if n == 1 && top == min_y && bot == max_y {
        renderer.move_to(out, new_buf, top as u16, 0)?;
        ansi::write_reverse_index(out)?;
        Ok(true)
    } else if n == 1 && bot == max_y && renderer.opts.contains(Optimizations::IL_DL) {
        renderer.move_to(out, new_buf, top as u16, 0)?;
        ansi::write_insert_lines(out, 1)?;
        Ok(true)
    } else if top == min_y && bot == max_y {
        renderer.move_to(out, new_buf, top as u16, 0)?;
        if renderer.opts.contains(Optimizations::SU_SD) {
            ansi::write_scroll_down(out, n)?;
        } else {
            for _ in 0..n {
                ansi::write_reverse_index(out)?;
            }
        }
        Ok(true)
    } else if bot == max_y && renderer.opts.contains(Optimizations::IL_DL) {
        renderer.move_to(out, new_buf, top as u16, 0)?;
        ansi::write_insert_lines(out, n)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Final fallback: emit `DL(n)` at `del`, then `IL(n)` at `ins`. The
/// pair simulates a scroll without DECSTBM: the DL pulls rows below
/// `del` upward, then the IL pushes rows back down at `ins` so rows
/// outside the intended region land back where they started.
pub(super) fn scroll_idl(
    out: &mut Vec<u8>,
    renderer: &mut Renderer,
    new_buf: &RenderBuffer,
    n: u16,
    del: usize,
    ins: usize,
) -> io::Result<bool> {
    renderer.move_to(out, new_buf, del as u16, 0)?;
    ansi::write_delete_lines(out, n)?;
    renderer.move_to(out, new_buf, ins as u16, 0)?;
    ansi::write_insert_lines(out, n)?;
    Ok(true)
}
