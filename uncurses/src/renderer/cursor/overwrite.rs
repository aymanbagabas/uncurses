//! Overwrite-style horizontal move candidate: when the destination row
//! is known, walking the cursor forward by re-emitting the row's own
//! cell bytes can be shorter than CUF/HPA.

use crate::cell::Cell;
use crate::style::Style;

/// Collect the UTF-8 bytes of cells in `line[from_x..to_x]` whose
/// style matches the active pen into `out`. Returns `true` when the
/// run is compatible with the pen and the bytes have been written;
/// returns `false` (and leaves `out` unchanged) when a width>0 cell
/// would require a pen change, when the requested column range
/// extends past the row, or when either end of the range falls inside
/// a cluster.
///
/// This candidate moves the cursor by drawing, so the columns it draws
/// have to add up to the distance it claims to travel. A range starting
/// on a continuation begins inside a glyph whose bytes have already gone
/// out, so it draws one column fewer than it owes; a range whose last
/// cell reaches past `to_x` draws one more. Either leaves the terminal's
/// cursor somewhere other than where the planner recorded it, and every
/// later move inherits the error.
pub(in crate::renderer) fn collect_overwrite_bytes(
    out: &mut Vec<u8>,
    line: &[Cell],
    style: &Style,
    from_x: u16,
    to_x: u16,
) -> bool {
    let from = from_x as usize;
    let to = to_x as usize;
    debug_assert!(
        to <= line.len(),
        "collect_overwrite_bytes: range {from}..{to} extends past line of length {}",
        line.len()
    );
    if to > line.len() {
        // Release builds: refuse the candidate rather than silently
        // returning a zero-byte "free" overwrite the planner will
        // happily choose over a real move.
        return false;
    }
    // A continuation here means the range opens inside a glyph the
    // terminal has already drawn, so no bytes are owed for the column and
    // the walk would arrive short.
    if line.get(from).is_some_and(Cell::is_continuation) {
        return false;
    }
    let mut i = from;
    while i < to {
        let cell = &line[i];
        if !cell.is_continuation() {
            if &cell.style != style {
                return false;
            }
            // Matches the cost pass, which prices this move in bytes and so
            // offers it only for a cluster of one code point. The two decide
            // eligibility together or the planner picks a move it cannot
            // emit.
            // Matches the cost pass: the cell has to draw the columns the
            // row credits it with, and it has to hold one code point.
            let content = cell.content();
            if usize::from(crate::text::WidthMode::Grapheme.grapheme_width(content, false))
                != usize::from(cell.width().max(1))
            {
                return false;
            }
            let mut code_points = content.chars();
            if code_points.next().is_none() || code_points.next().is_some() {
                return false;
            }
            i += cell.width().max(1) as usize;
            continue;
        }
        // No cell stepped over this continuation, so none owns it and
        // nothing draws its column. Matches the cost pass.
        return false;
    }
    // Landing past `to` means the last cluster reaches beyond the range, so
    // drawing it would carry the cursor further than the move promised.
    if i != to {
        return false;
    }
    out.clear();
    let mut i = from;
    while i < to {
        let cell = &line[i];
        if !cell.is_continuation() {
            out.extend_from_slice(cell.content().as_bytes());
            i += cell.width().max(1) as usize;
            continue;
        }
        i += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OOB ranges must refuse the candidate in release builds (and
    /// hit a debug_assert in debug). Returning `true` with zero
    /// bytes would mis-report the candidate as a free move and beat
    /// every legitimate CUF/HPA alternative.
    #[test]
    #[cfg_attr(debug_assertions, should_panic)]
    fn out_of_bounds_range_refuses_candidate() {
        let line = vec![Cell::narrow("a"); 4];
        let style = Style::default();
        let mut out = Vec::new();
        let accepted = collect_overwrite_bytes(&mut out, &line, &style, 0, 8);
        assert!(!accepted, "OOB range must return false in release");
        assert!(out.is_empty());
    }

    #[test]
    fn in_bounds_pen_match_writes_bytes() {
        let line = vec![Cell::narrow("x"); 3];
        let style = Style::default();
        let mut out = Vec::new();
        assert!(collect_overwrite_bytes(&mut out, &line, &style, 0, 3));
        assert_eq!(out, b"xxx");
    }
}

#[cfg(test)]
mod cluster_bounds_tests {
    use super::*;

    /// A row of two-column clusters, so column `2n` owns one and `2n + 1`
    /// is its continuation.
    fn wide_line() -> Vec<Cell> {
        let mut line = Vec::new();
        for _ in 0..5 {
            line.push(Cell::wide("\u{4e16}"));
            line.push(Cell::continuation());
        }
        line
    }

    /// The candidate moves the cursor by drawing, so it is only usable when
    /// what it draws spans exactly the columns it claims to cross.
    #[test]
    fn a_move_opening_inside_a_glyph_is_refused() {
        let line = wide_line();
        let mut out = Vec::new();
        // Column 1 is the second half of the cluster at 0. Its bytes went
        // out with that cluster, so there is nothing left to draw for it and
        // the walk would arrive a column short.
        assert!(!collect_overwrite_bytes(
            &mut out,
            &line,
            &Style::default(),
            1,
            6
        ));
    }

    /// The mirror: a range whose last cluster reaches past the end.
    #[test]
    fn a_move_ending_inside_a_glyph_is_refused() {
        let line = wide_line();
        let mut out = Vec::new();
        // The cluster at column 4 occupies 4 and 5, so stopping at 5 would
        // draw a glyph that carries the cursor one column past the target.
        assert!(!collect_overwrite_bytes(
            &mut out,
            &line,
            &Style::default(),
            0,
            5
        ));
    }

    /// A range covering whole clusters draws exactly the columns it crosses.
    #[test]
    fn a_move_over_whole_clusters_draws_what_it_crosses() {
        let line = wide_line();
        let mut out = Vec::new();
        assert!(collect_overwrite_bytes(
            &mut out,
            &line,
            &Style::default(),
            0,
            6
        ));
        // Three clusters of two columns each, and nothing for the
        // continuations, which the clusters already account for.
        assert_eq!(String::from_utf8_lossy(&out), "\u{4e16}\u{4e16}\u{4e16}");
    }
}

#[cfg(test)]
mod passes_agree {
    use super::*;
    use crate::ansi::cost::overwrite_cost;

    /// Cells covering the shapes the planner meets: narrow, wide with its
    /// continuation, and clusters of several code points.
    fn cells() -> Vec<Cell> {
        vec![
            Cell::narrow("a"),
            Cell::wide("\u{4e16}"),
            Cell::continuation(),
            Cell::narrow("b"),
            Cell::wide("\u{1f1ef}\u{1f1f5}"),
            Cell::continuation(),
            Cell::narrow("e\u{301}"),
            Cell::wide("\u{1f468}\u{200d}\u{1f469}"),
            Cell::continuation(),
            Cell::narrow("c"),
            // Cells whose content draws a different number of columns than
            // the row credits them with. Each would let the walk arrive
            // somewhere the planner did not record.
            Cell::narrow(""),
            Cell::narrow("\u{301}"),
            Cell::narrow("\u{8}"),
            Cell::narrow("\u{4e16}"),
            Cell::wide("a"),
        ]
    }

    /// The planner prices this move in one pass and emits it in another. A
    /// range one accepts and the other refuses makes it choose a move it
    /// cannot produce, which the assertion in `axis` reports at runtime.
    #[test]
    fn the_cost_pass_and_the_emit_pass_accept_the_same_ranges() {
        let line = cells();
        let style = Style::default();
        for from in 0..=line.len() {
            for to in from..=line.len() {
                let priced = overwrite_cost(&line, &style, from as u16, to as u16).is_some();
                let mut out = Vec::new();
                let emitted =
                    collect_overwrite_bytes(&mut out, &line, &style, from as u16, to as u16);
                assert_eq!(
                    priced, emitted,
                    "range {from}..{to}: the cost pass says {priced} and the emit pass says {emitted}"
                );
            }
        }
    }

    /// What the move draws has to span exactly the columns it crosses, or
    /// the cursor stops somewhere the planner did not record.
    #[test]
    fn an_accepted_range_draws_the_columns_it_crosses() {
        let line = cells();
        let style = Style::default();
        for from in 0..=line.len() {
            for to in from..=line.len() {
                let mut out = Vec::new();
                if !collect_overwrite_bytes(&mut out, &line, &style, from as u16, to as u16) {
                    continue;
                }
                let drawn: usize = String::from_utf8_lossy(&out)
                    .chars()
                    .map(|c| {
                        usize::from(
                            crate::text::WidthMode::Grapheme
                                .grapheme_width(c.encode_utf8(&mut [0u8; 4]), false),
                        )
                    })
                    .sum();
                assert_eq!(drawn, to - from, "range {from}..{to} drew {drawn} columns");
            }
        }
    }
}

#[cfg(test)]
mod still_useful {
    use super::*;
    use crate::ansi::cost::overwrite_cost;

    /// The rules refuse ranges that cut a glyph, not ranges that contain
    /// one. A wide cell is stepped over together with the continuation it
    /// owns, so a range covering whole clusters is still priced and still
    /// emitted.
    #[test]
    fn a_range_over_whole_wide_clusters_is_still_offered() {
        let line = vec![
            Cell::wide("\u{4e16}"),
            Cell::continuation(),
            Cell::wide("\u{754c}"),
            Cell::continuation(),
            Cell::narrow("a"),
        ];
        let style = Style::default();

        // Both clusters plus the narrow cell.
        assert_eq!(overwrite_cost(&line, &style, 0, 5), Some(7));
        let mut out = Vec::new();
        assert!(collect_overwrite_bytes(&mut out, &line, &style, 0, 5));
        assert_eq!(String::from_utf8_lossy(&out), "\u{4e16}\u{754c}a");

        // And a single cluster on its own.
        assert_eq!(overwrite_cost(&line, &style, 0, 2), Some(3));
    }
}

#[cfg(test)]
mod empty_content {
    use super::*;
    use crate::ansi::cost::overwrite_cost;

    /// A cell that owns a column and has nothing to write cannot carry the
    /// cursor across it.
    ///
    /// Its content is empty while its width is one, so pricing the move by
    /// bytes would call it free and drawing it would cross no columns, while
    /// the planner recorded one crossed. Refusing it keeps what the move
    /// draws equal to the distance it claims.
    #[test]
    fn a_cell_with_nothing_to_write_cannot_carry_the_cursor() {
        let line = vec![Cell::narrow(""), Cell::narrow("a")];
        let style = Style::default();
        assert_eq!(overwrite_cost(&line, &style, 0, 1), None);
        let mut out = Vec::new();
        assert!(!collect_overwrite_bytes(&mut out, &line, &style, 0, 1));
    }
}

#[cfg(test)]
mod width_honesty {
    use super::*;
    use crate::ansi::cost::overwrite_cost;

    /// A cell has to draw the columns the row credits it with.
    ///
    /// Nothing stops a cell from claiming a width its content does not have,
    /// and this move travels by drawing, so such a cell carries the cursor
    /// somewhere other than where the planner recorded it. A combining mark
    /// and a control character draw nothing, a narrow cell can hold a wide
    /// glyph, and a wide one can hold a narrow glyph.
    #[test]
    fn a_cell_that_draws_a_width_it_does_not_claim_is_refused() {
        let style = Style::default();
        for cell in [
            Cell::narrow("\u{301}"),
            Cell::narrow("\u{8}"),
            Cell::narrow("\u{4e16}"),
            Cell::wide("a"),
            Cell::narrow(""),
        ] {
            let line = vec![cell.clone(), Cell::narrow("z")];
            let mut out = Vec::new();
            assert_eq!(
                overwrite_cost(&line, &style, 0, 1),
                None,
                "priced a move over {:?}",
                cell.content()
            );
            assert!(
                !collect_overwrite_bytes(&mut out, &line, &style, 0, 1),
                "emitted a move over {:?}",
                cell.content()
            );
        }
    }
}

#[cfg(test)]
mod unowned_continuation {
    use super::*;
    use crate::ansi::cost::overwrite_cost;

    /// A continuation nothing owns is a column nothing draws.
    ///
    /// The walk steps over a continuation together with the cell that owns
    /// it, so reaching one on its own means no cell will draw that column
    /// while the move still counts it as crossed. `Buffer::resize` and the
    /// shifting operations can both leave such a row behind.
    #[test]
    fn a_range_holding_an_unowned_continuation_is_refused() {
        let style = Style::default();
        let line = vec![Cell::narrow("a"), Cell::continuation()];
        assert_eq!(overwrite_cost(&line, &style, 0, 2), None);
        let mut out = Vec::new();
        assert!(!collect_overwrite_bytes(&mut out, &line, &style, 0, 2));

        // The same columns with an owner are still offered, and draw both.
        let owned = vec![Cell::wide("\u{4e16}"), Cell::continuation()];
        assert_eq!(overwrite_cost(&owned, &style, 0, 2), Some(3));
        let mut out = Vec::new();
        assert!(collect_overwrite_bytes(&mut out, &owned, &style, 0, 2));
        assert_eq!(String::from_utf8_lossy(&out), "\u{4e16}");
    }
}
