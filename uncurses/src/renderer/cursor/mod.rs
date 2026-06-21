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
    use crate::layout::Position;

    fn renderer() -> Renderer {
        let mut r = Renderer::new();
        // Mark both axes as already-known so a fresh planner call
        // doesn't force CUP on the very first move.
        r.cur.x_unknown = false;
        r.cur.y_unknown = false;
        // Width must be set; the planner forces CUP when the surface
        // width is unknown (== 0).
        r.last_width = 80;
        r.last_height = 24;
        r
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
        let mut r = abs();
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 5, x: 10 },
            Position { y: 5, x: 8 },
            None,
        )
        .unwrap();
        assert_eq!(buf, b"\x08\x08");
    }

    #[test]
    fn back_tab_wins_for_aligned_left_move() {
        let mut r = rel();
        r.tabs = TabStops::default_for(120);
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 5, x: 100 },
            Position { y: 5, x: 96 },
            None,
        )
        .unwrap();
        assert_eq!(buf, b"\x1b[Z");
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
        let mut r = abs();
        // Width small enough that not_local short-circuits, so CUP
        // isn't forced and tab gets to compete on byte-count.
        r.last_width = 16;
        r.tabs = TabStops::default_for(16);
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 0, x: 0 },
            Position { y: 0, x: 8 },
            None,
        )
        .unwrap();
        assert_eq!(buf, b"\t");
    }

    #[test]
    fn custom_tab_stops_replace_default_eight() {
        let mut r = abs();
        let mut ts = TabStops::default_for(80);
        ts.clear();
        ts.set(4);
        r.tabs = ts;
        let mut buf = Vec::new();
        r.write_optimal_move(
            &mut buf,
            Position { y: 0, x: 0 },
            Position { y: 0, x: 4 },
            None,
        )
        .unwrap();
        assert_eq!(buf, b"\t");
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
}
