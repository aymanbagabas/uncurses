//! Cursor movement cost optimization helpers and planner.
//!
//! This module is intentionally thin: it holds the constants and
//! helpers (`LONG_DIST`, [`not_local`]) used by the planner itself,
//! which lives on the renderer as `write_optimal_move`; the test-only
//! relative wrapper is compiled only for tests.

pub(super) mod axis;
pub(super) mod overwrite;
pub(super) mod planner;
pub(super) mod relative;

use crate::layout::Position;

/// Local-move threshold in cells.
///
/// When the target is away from the edges and the Manhattan distance is
/// greater than this value, absolute mode skips the relative cost search
/// and emits CUP directly.
pub(super) const LONG_DIST: u16 = 8 - 1;

/// Return whether a move is non-local for CUP planning.
///
/// Mirrors the typical terminfo heuristic: the target must be at least
/// [`LONG_DIST`] cells from both horizontal edges, and the Manhattan
/// distance from `from` to `to` must exceed [`LONG_DIST`].
pub(super) fn not_local(width: u16, from: Position, to: Position) -> bool {
    if width <= 2 * LONG_DIST + 2 {
        return false;
    }
    let right_edge = width.saturating_sub(1).saturating_sub(LONG_DIST);
    let manhattan = to.y.abs_diff(from.y) + to.x.abs_diff(from.x);
    to.x > LONG_DIST && to.x < right_edge && manhattan > LONG_DIST
}

#[cfg(test)]
mod tests {
    use super::super::Renderer;
    use super::super::caps::Optimizations;
    use super::super::tabstops::TabStops;
    use crate::cell::Cell;
    use crate::color::Color;
    use crate::layout::Position;
    use crate::renderer::frame::emit::PenPolicy;
    use crate::style::Style;

    fn renderer() -> Renderer {
        let mut r = Renderer::new();
        // Mark both axes as already-known so a fresh planner call
        // doesn't force CUP on the very first move.
        r.cur.x = Some(0);
        r.cur.y = Some(0);
        // Width must be set; the planner forces CUP when the surface
        // width is unknown (== 0).
        r.last_width = 80;
        r.last_height = 24;
        r
    }

    /// `TABS` and `BS` are granted by `Screen::init` from the live
    /// terminal state, never by a `$TERM` baseline, so every test that
    /// exercises them has to ask for them and pair the result with the
    /// fallback the planner picks when the host withholds them.
    fn with_line_discipline(mut r: Renderer) -> Renderer {
        r.opts = r.opts.with_tabs(true).with_bs(true);
        r
    }

    fn moved(r: &mut Renderer, from: Position, to: Position) -> Vec<u8> {
        let mut buf = Vec::new();
        r.write_optimal_move(&mut buf, from, to, None).unwrap();
        buf
    }

    fn abs() -> Renderer {
        let mut r = renderer();
        r.relative_cursor = false;
        r
    }

    fn rel() -> Renderer {
        let mut r = renderer();
        r.relative_cursor = true;
        r
    }

    fn fs_abs() -> Renderer {
        let mut r = abs();
        r.fullscreen = true;
        r
    }

    #[test]
    fn move_diag_down_left_emits_cub_not_bs() {
        let mut r = abs();
        r.fullscreen = true;
        r.last_width = 120;
        r.last_height = 50;
        r.tabs = crate::renderer::tabstops::TabStops::default_for(120);
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 46, x: 61 },
            Position { y: 47, x: 55 },
            None,
        )
        .unwrap();
        assert_eq!(
            buf,
            b"\n\x1b[6D",
            "expected LF + CUB(6); got {:?}",
            std::str::from_utf8(&buf)
        );
    }

    #[test]
    fn same_position_is_noop() {
        let mut r = abs();
        let mut buf = Vec::new();
        let pos = Position { y: 5, x: 10 };
        r.write_optimal_move(&mut buf, pos, pos, None).unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn move_down_one_uses_newline() {
        let mut r = abs();
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 5, x: 0 },
            Position { y: 6, x: 0 },
            None,
        )
        .unwrap();
        assert_eq!(buf, b"\n");
    }

    #[test]
    fn move_down_multiple_uses_newlines() {
        let mut r = rel();
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 0, x: 0 },
            Position { y: 3, x: 0 },
            None,
        )
        .unwrap();
        assert_eq!(buf, b"\n\n\n");
    }

    #[test]
    fn move_to_origin_uses_cup() {
        let mut r = abs();
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 10, x: 10 },
            Position { y: 0, x: 0 },
            None,
        )
        .unwrap();
        assert_eq!(buf, b"\x1b[H");
    }

    #[test]
    fn move_same_row_left_small_uses_backspace() {
        let from = Position { y: 5, x: 10 };
        let to = Position { y: 5, x: 8 };
        assert_eq!(
            moved(&mut with_line_discipline(abs()), from, to),
            b"\x08\x08"
        );
        // Without BS the 2-column step ties with CHA on byte count and
        // absolute positioning takes the tiebreak.
        assert_eq!(moved(&mut abs(), from, to), b"\x1b[9G");
    }

    #[test]
    fn back_tab_wins_for_aligned_left_move() {
        let from = Position { y: 5, x: 100 };
        let to = Position { y: 5, x: 96 };
        let tabbed = |r: Renderer| {
            let mut r = r;
            r.tabs = TabStops::default_for(120);
            r
        };
        assert_eq!(
            moved(&mut tabbed(with_line_discipline(rel())), from, to),
            b"\x1b[Z"
        );
        // CBT counts columns in tab stops, so the planner only trusts it
        // when the host also lets a literal `\t` through: see `axis.rs`.
        assert_eq!(moved(&mut tabbed(rel()), from, to), b"\x1b[4D");
    }

    #[test]
    fn back_tab_disabled_falls_back() {
        let mut r = rel();
        r.tabs = TabStops::default_for(120);
        r.opts.remove(Optimizations::CBT);
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 5, x: 100 },
            Position { y: 5, x: 96 },
            None,
        )
        .unwrap();
        assert!(!buf.windows(3).any(|w| w == b"\x1b[Z"));
    }

    #[test]
    fn tab_move_wins_over_cuf() {
        // Width small enough that not_local short-circuits, so CUP
        // isn't forced and tab gets to compete on byte-count.
        let narrow = |r: Renderer| {
            let mut r = r;
            r.last_width = 16;
            r.tabs = TabStops::default_for(16);
            r
        };
        let from = Position { y: 0, x: 0 };
        let to = Position { y: 0, x: 8 };
        assert_eq!(
            moved(&mut narrow(with_line_discipline(abs())), from, to),
            b"\t"
        );
        assert_eq!(moved(&mut narrow(abs()), from, to), b"\x1b[9G");
    }

    #[test]
    fn custom_tab_stops_replace_default_eight() {
        let stop_at_4 = |r: Renderer| {
            let mut r = r;
            let mut ts = TabStops::default_for(80);
            ts.clear();
            ts.set(4);
            r.tabs = ts;
            r
        };
        let from = Position { y: 0, x: 0 };
        let to = Position { y: 0, x: 4 };
        assert_eq!(
            moved(&mut stop_at_4(with_line_discipline(abs())), from, to),
            b"\t"
        );
        assert_eq!(moved(&mut stop_at_4(abs()), from, to), b"\x1b[5G");
    }

    #[test]
    fn cleared_tab_stop_falls_back_to_cuf() {
        let mut r = abs();
        let mut ts = TabStops::default_for(80);
        ts.reset(8);
        r.tabs = ts;
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 0, x: 0 },
            Position { y: 0, x: 8 },
            None,
        )
        .unwrap();
        assert_ne!(buf, b"\t");
    }

    #[test]
    fn inline_mode_forces_newlines_for_downward() {
        let mut r = rel();
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 0, x: 5 },
            Position { y: 1, x: 0 },
            None,
        )
        .unwrap();
        // Outer CR-prefix wins: `\r` then `\n` lands at (1, 0).
        assert_eq!(buf, b"\r\n");
    }

    #[test]
    fn inline_mode_preserves_column_with_plain_newline() {
        let mut r = rel();
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 0, x: 3 },
            Position { y: 2, x: 3 },
            None,
        )
        .unwrap();
        // \n keeps col=3 in raw mode, so no horizontal correction needed.
        assert_eq!(buf, b"\n\n");
    }

    #[test]
    fn onlcr_resets_column() {
        let mut r = rel();
        r.opts.insert(Optimizations::ONLCR);
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 0, x: 5 },
            Position { y: 1, x: 0 },
            None,
        )
        .unwrap();
        assert_eq!(buf, b"\n");
    }

    #[test]
    fn ri_used_for_single_up_step() {
        let mut r = rel();
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 5, x: 0 },
            Position { y: 4, x: 0 },
            None,
        )
        .unwrap();
        assert_eq!(buf, b"\x1bM");
    }

    #[test]
    fn fullscreen_does_not_force_newlines() {
        let mut r = fs_abs();
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 0, x: 0 },
            Position { y: 10, x: 0 },
            None,
        )
        .unwrap();
        assert_ne!(buf, b"\n\n\n\n\n\n\n\n\n\n");
        assert!(buf.len() <= 5);
    }

    #[test]
    fn cr_helper_used_for_long_left_to_low_col() {
        // CUB(48)=5 vs \r + CUF(1)=4.
        let mut r = rel();
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 5, x: 49 },
            Position { y: 5, x: 1 },
            None,
        )
        .unwrap();
        assert_eq!(buf, b"\r\x1b[C");
    }

    #[test]
    fn not_local_long_jump_uses_cup() {
        let mut r = abs();
        r.last_width = 120;
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 0, x: 0 },
            Position { y: 20, x: 40 },
            None,
        )
        .unwrap();
        assert_eq!(buf, b"\x1b[21;41H");
    }

    #[test]
    fn not_local_skipped_when_target_near_edge() {
        let mut r = abs();
        r.last_width = 120;
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 0, x: 0 },
            Position { y: 5, x: 115 },
            None,
        )
        .unwrap();
        assert!(!buf.is_empty());
    }

    // --- BCE: pen reset before a scrolling `\n` ----------------------
    //
    // Inline downward moves are emitted as literal line feeds no matter
    // the byte cost, so the host scrolls when the destination row does
    // not exist yet. On a terminal with back-color erase that scroll
    // paints the freshly exposed row with the active pen's background,
    // and nothing records it in `cur_buf` — so the stray colour is
    // never diffed away. The planner resets the pen first.

    /// The reset lands before the line feeds, and leaves the tracked
    /// pen at the default so the next glyph run re-asserts what it
    /// needs.
    #[test]
    fn inline_scrolling_lf_resets_pen_first() {
        let mut r = rel();
        r.cur.set_style(Style::default().bg(Color::Blue));
        let out = moved(&mut r, Position { y: 0, x: 0 }, Position { y: 2, x: 0 });
        assert_eq!(out, b"\x1b[m\n\n");
        assert!(
            r.cur.style().is_empty(),
            "pen must be tracked as reset, got {:?}",
            r.cur.style()
        );
    }

    /// A fullscreen surface is sized to the screen and `move_to` clamps
    /// the target row, so `\n` never reaches the bottom margin and the
    /// pen can ride along untouched.
    #[test]
    fn fullscreen_lf_keeps_pen() {
        let mut r = rel();
        r.fullscreen = true;
        r.cur.set_style(Style::default().bg(Color::Blue));
        let out = moved(&mut r, Position { y: 0, x: 0 }, Position { y: 1, x: 0 });
        assert_eq!(out, b"\n");
    }

    /// Without BCE the terminal erases with its own default background,
    /// so the pen cannot bleed and the reset would be dead bytes.
    #[test]
    fn inline_lf_without_bce_keeps_pen() {
        let mut r = rel();
        r.opts = r.opts.with_bce(false);
        r.cur.set_style(Style::default().bg(Color::Blue));
        let out = moved(&mut r, Position { y: 0, x: 0 }, Position { y: 2, x: 0 });
        assert_eq!(out, b"\n\n");
    }

    /// Back-color erase paints the background and nothing else, so a
    /// pen carrying only a foreground leaves no trace on the exposed
    /// row. Matches what `Cursor::bce_blank` records for a deliberate
    /// scroll.
    #[test]
    fn inline_lf_with_foreground_only_pen_keeps_pen() {
        let mut r = rel();
        r.cur.set_style(Style::default().fg(Color::Blue));
        let out = moved(&mut r, Position { y: 0, x: 0 }, Position { y: 2, x: 0 });
        assert_eq!(out, b"\n\n");
    }

    /// Upward moves use CUU / RI, which never scroll a row into
    /// existence.
    #[test]
    fn inline_upward_move_keeps_pen() {
        let mut r = rel();
        r.cur.set_style(Style::default().bg(Color::Blue));
        let out = moved(&mut r, Position { y: 3, x: 0 }, Position { y: 1, x: 0 });
        assert!(
            !out.starts_with(b"\x1b[m"),
            "upward move must not reset the pen, got {out:?}"
        );
    }

    /// `PenPolicy::Keep` is how the scroll emitters and `clear_below`
    /// opt out: they erase with the active pen deliberately and record
    /// that fill themselves.
    #[test]
    fn keep_policy_skips_the_reset() {
        let mut r = rel();
        r.cur.set_style(Style::default().bg(Color::Blue));
        let mut buf = Vec::new();
        r.write_optimal_move_with_pen(
            &mut buf,
            Position { y: 0, x: 0 },
            Position { y: 2, x: 0 },
            None,
            PenPolicy::Keep,
        )
        .unwrap();
        assert_eq!(buf, b"\n\n");
    }

    /// The horizontal leg can move forward by re-printing destination
    /// cells, but only cells matching the active pen qualify. Resetting
    /// for the scroll changes which cells those are, so the plan is
    /// recomputed against the pen that will actually be in effect —
    /// otherwise the move would re-print blue-backed text with a pen
    /// that is no longer blue.
    ///
    /// The distance matters: overwrite only competes while
    /// `n < cuf_cost(n)`, so `n = 3` (against `\x1b[3C`, 4 bytes) keeps
    /// the pen walk reachable. At `n = 5` the cost floor short-circuits
    /// before the pen is ever consulted and the test would pass without
    /// the re-plan.
    #[test]
    fn reset_before_lf_disqualifies_styled_overwrite_cells() {
        let mut r = rel();
        let styled = Style::default().bg(Color::Blue);
        r.cur.set_style(styled.clone());
        let line: Vec<Cell> = "abcde"
            .chars()
            .map(|c| Cell::narrow(c.to_string()).style(styled.clone()))
            .collect();

        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 0, x: 0 },
            Position { y: 2, x: 3 },
            Some(&line),
        )
        .unwrap();

        assert_eq!(
            buf, b"\x1b[m\n\n\x1b[3C",
            "expected a reset then a plain forward move, got {buf:?}"
        );
        assert!(
            !buf.windows(3).any(|w| w == b"abc"),
            "must not re-print styled cells under a reset pen, got {buf:?}"
        );
    }

    /// The same move without the scrolling `\n` keeps the pen, so the
    /// styled cells stay eligible and overwrite still wins. Pins the
    /// other side of the re-plan: the reset is what disqualifies them,
    /// not the distance.
    #[test]
    fn styled_overwrite_survives_when_no_reset_is_needed() {
        let mut r = rel();
        let styled = Style::default().bg(Color::Blue);
        r.cur.set_style(styled.clone());
        let line: Vec<Cell> = "abcde"
            .chars()
            .map(|c| Cell::narrow(c.to_string()).style(styled.clone()))
            .collect();

        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 0, x: 0 },
            Position { y: 0, x: 3 },
            Some(&line),
        )
        .unwrap();

        assert_eq!(buf, b"abc", "expected an overwrite move, got {buf:?}");
    }
}
