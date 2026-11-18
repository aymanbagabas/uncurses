//! Overwrite-style horizontal move candidate: when the destination row
//! is known, walking the cursor forward by re-emitting the row's own
//! cell bytes can be shorter than CUF/HPA.

use crate::buffer::CellRow;
#[cfg(test)]
use crate::cell::Cell;
use crate::renderer::Renderer;
use crate::style::Style;

/// Whether `content` draws exactly `want` columns under every width policy.
///
/// The cost and emit passes both travel by drawing. A policy-dependent
/// width could leave the cursor in a different column than the planner records.
fn crossable_under_every_policy(content: &str, want: usize) -> bool {
    use crate::text::WidthMode;
    // Printable ASCII avoids four width table lookups on the common path.
    if want == 1 && content.len() == 1 {
        let b = content.as_bytes()[0];
        if b.is_ascii_graphic() || b == b' ' {
            return true;
        }
    }
    for mode in [WidthMode::Grapheme, WidthMode::Wc] {
        for eaw_wide in [false, true] {
            if usize::from(mode.grapheme_width(content, eaw_wide)) != want {
                return false;
            }
        }
    }
    true
}

impl Renderer {
    /// Byte cost of a forward move that draws `line[from..to]` with the active pen.
    ///
    /// Returns `None` for ranges that cut a glyph, contain an unowned continuation,
    /// require a pen change, or draw a policy-dependent width. Each primary must
    /// contain exactly one code point. The emit pass applies the same rules.
    pub(super) fn overwrite_cost(&self, line: CellRow<'_>, from: u16, to: u16) -> Option<usize> {
        let from = from as usize;
        let to = to as usize;
        if to > line.len() || from > to {
            return None;
        }
        let mut line = line.iter().skip(from).peekable();
        if line.peek().is_some_and(|cell| cell.is_continuation()) {
            return None;
        }
        let mut i = from;
        let mut cost = 0usize;
        while i < to {
            let cell = line.next()?;
            if !cell.is_continuation() {
                if !cell.style_eq(self.cur.style()) {
                    return None;
                }
                let content = cell.content();
                if !crossable_under_every_policy(content, usize::from(cell.width().max(1))) {
                    return None;
                }
                let mut code_points = content.chars();
                if code_points.next().is_none() || code_points.next().is_some() {
                    return None;
                }
                cost += content.len();
                let w = cell.width().max(1) as usize;
                i += w;
                if w > 1 {
                    line.nth(w - 2)?;
                }
                continue;
            }
            return None;
        }
        if i != to {
            return None;
        }
        Some(cost)
    }
}

/// Collect the UTF-8 bytes of cells in `line.slice(from_x..to_x)` whose
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
    line: CellRow<'_>,
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
    if line.get(from).is_some_and(|cell| cell.is_continuation()) {
        return false;
    }
    let mut i = from;
    while i < to {
        let cell = line.at(i);
        if !cell.is_continuation() {
            if !cell.style_eq(style) {
                return false;
            }
            // Matches the cost pass, which prices this move in bytes and so
            // offers it only for a cluster of one code point. The two decide
            // eligibility together or the planner picks a move it cannot
            // emit.
            // Matches the cost pass: the cell has to draw the columns the
            // row credits it with, and it has to hold one code point.
            let content = cell.content();
            if !crossable_under_every_policy(content, usize::from(cell.width().max(1))) {
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
        let cell = line.at(i);
        if !cell.is_continuation() {
            out.extend_from_slice(cell.draw_bytes());
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

    #[test]
    fn an_overwrite_declines_a_cell_whose_width_depends_on_the_policy() {
        use crate::text::WidthMode;

        let dot = "\u{00b7}";
        assert_eq!(WidthMode::Grapheme.grapheme_width(dot, false), 1);
        assert_eq!(WidthMode::Grapheme.grapheme_width(dot, true), 2);

        let flag = "\u{1f1ef}";
        assert_eq!(WidthMode::Grapheme.grapheme_width(flag, false), 2);
        assert_eq!(WidthMode::Wc.grapheme_width(flag, false), 1);

        let renderer = Renderer::new();
        assert_eq!(
            renderer.overwrite_cost(
                CellRow::from_cells(&[Cell::new(dot, 1), Cell::new("a", 1)]),
                0,
                2,
            ),
            None,
            "ambiguous width draws two columns in a CJK locale"
        );
        assert_eq!(
            renderer.overwrite_cost(
                CellRow::from_cells(&[Cell::new(flag, 2), Cell::CONTINUATION]),
                0,
                2,
            ),
            None,
            "a lone regional indicator draws one column under wcwidth"
        );
        assert!(
            renderer
                .overwrite_cost(
                    CellRow::from_cells(&[Cell::new("a", 1), Cell::new("b", 1)]),
                    0,
                    2,
                )
                .is_some(),
            "plain ASCII must remain crossable"
        );
    }

    /// OOB ranges must refuse the candidate in release builds (and
    /// hit a debug_assert in debug). Returning `true` with zero
    /// bytes would mis-report the candidate as a free move and beat
    /// every legitimate CUF/HPA alternative.
    #[test]
    #[cfg_attr(debug_assertions, should_panic)]
    fn out_of_bounds_range_refuses_candidate() {
        let line = vec![Cell::new("a", 1); 4];
        let style = Style::default();
        let mut out = Vec::new();
        let accepted = collect_overwrite_bytes(&mut out, CellRow::from_cells(&line), &style, 0, 8);
        assert!(!accepted, "OOB range must return false in release");
        assert!(out.is_empty());
    }

    #[test]
    fn in_bounds_pen_match_writes_bytes() {
        let line = vec![Cell::new("x", 1); 3];
        let style = Style::default();
        let mut out = Vec::new();
        assert!(collect_overwrite_bytes(
            &mut out,
            CellRow::from_cells(&line),
            &style,
            0,
            3
        ));
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
            line.push(Cell::new("\u{4e16}", 2));
            line.push(Cell::CONTINUATION);
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
            CellRow::from_cells(&line),
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
            CellRow::from_cells(&line),
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
            CellRow::from_cells(&line),
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
    use crate::buffer::Buffer;

    /// Cells covering the shapes the planner meets: narrow, wide with its
    /// continuation, and clusters of several code points.
    fn cells() -> Vec<Cell> {
        vec![
            Cell::new("a", 1),
            Cell::new("\u{4e16}", 2),
            Cell::CONTINUATION,
            Cell::new("b", 1),
            Cell::new("\u{1f1ef}\u{1f1f5}", 2),
            Cell::CONTINUATION,
            Cell::new("e\u{301}", 1),
            Cell::new("\u{1f468}\u{200d}\u{1f469}", 2),
            Cell::CONTINUATION,
            Cell::new("c", 1),
            // Cells whose content draws a different number of columns than
            // the row credits them with. Each would let the walk arrive
            // somewhere the planner did not record.
            Cell::new("", 1),
            Cell::new("\u{301}", 1),
            Cell::new("\u{8}", 1),
            Cell::new("\u{4e16}", 1),
            Cell::new("a", 2),
        ]
    }

    /// The planner prices this move in one pass and emits it in another. A
    /// range one accepts and the other refuses makes it choose a move it
    /// cannot produce, which the assertion in `axis` reports at runtime.
    #[test]
    fn the_cost_pass_and_the_emit_pass_accept_the_same_ranges() {
        let line = cells();
        let style = Style::default();
        let renderer = Renderer::new();
        for from in 0..=line.len() {
            for to in from..=line.len() {
                let priced = renderer
                    .overwrite_cost(CellRow::from_cells(&line), from as u16, to as u16)
                    .is_some();
                let mut out = Vec::new();
                let emitted = collect_overwrite_bytes(
                    &mut out,
                    CellRow::from_cells(&line),
                    &style,
                    from as u16,
                    to as u16,
                );
                assert_eq!(
                    priced, emitted,
                    "range {from}..{to}: the cost pass says {priced} and the emit pass says {emitted}"
                );
            }
        }
    }

    #[test]
    fn compact_row_slices_preserve_cost_and_pen_checks() {
        let style = Style::default().bold().link("https://example.test", "");
        let mut buffer = Buffer::new(8, 1);
        buffer.set((1, 0), Cell::new("界", 2).style(style.clone()).as_ref());
        buffer.set((3, 0), Cell::new("x", 1).style(style).as_ref());
        let row = buffer.row(0).unwrap().slice(1..5);
        let mut renderer = Renderer::new();
        for style in [
            Style::default(),
            Style::default().bold().link("https://example.test", ""),
        ] {
            renderer.cur.set_style(style.clone());
            for from in 0..=row.len() as u16 {
                for to in from..=row.len() as u16 {
                    let cost = renderer.overwrite_cost(row, from, to);
                    let mut out = Vec::new();
                    let emitted = collect_overwrite_bytes(&mut out, row, &style, from, to);
                    assert_eq!(cost.is_some(), emitted, "{from}..{to}");
                    if let Some(cost) = cost {
                        assert_eq!(cost, out.len(), "{from}..{to}");
                    }
                }
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
                if !collect_overwrite_bytes(
                    &mut out,
                    CellRow::from_cells(&line),
                    &style,
                    from as u16,
                    to as u16,
                ) {
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

    /// The rules refuse ranges that cut a glyph, not ranges that contain
    /// one. A wide cell is stepped over together with the continuation it
    /// owns, so a range covering whole clusters is still priced and still
    /// emitted.
    #[test]
    fn a_range_over_whole_wide_clusters_is_still_offered() {
        let line = vec![
            Cell::new("\u{4e16}", 2),
            Cell::CONTINUATION,
            Cell::new("\u{754c}", 2),
            Cell::CONTINUATION,
            Cell::new("a", 1),
        ];
        let style = Style::default();
        let renderer = Renderer::new();

        // Both clusters plus the narrow cell.
        assert_eq!(
            renderer.overwrite_cost(CellRow::from_cells(&line), 0, 5),
            Some(7)
        );
        let mut out = Vec::new();
        assert!(collect_overwrite_bytes(
            &mut out,
            CellRow::from_cells(&line),
            &style,
            0,
            5
        ));
        assert_eq!(String::from_utf8_lossy(&out), "\u{4e16}\u{754c}a");

        // And a single cluster on its own.
        assert_eq!(
            renderer.overwrite_cost(CellRow::from_cells(&line), 0, 2),
            Some(3)
        );
    }
}

#[cfg(test)]
mod width_honesty {
    use super::*;

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
        let renderer = Renderer::new();
        for cell in [
            Cell::new("\u{301}", 1),
            Cell::new("\u{8}", 1),
            Cell::new("\u{4e16}", 1),
            Cell::new("a", 2),
            Cell::new("", 1),
        ] {
            let line = vec![cell.clone(), Cell::new("z", 1)];
            let mut out = Vec::new();
            assert_eq!(
                renderer.overwrite_cost(CellRow::from_cells(&line), 0, 1),
                None,
                "priced a move over {:?}",
                cell.content()
            );
            assert!(
                !collect_overwrite_bytes(&mut out, CellRow::from_cells(&line), &style, 0, 1),
                "emitted a move over {:?}",
                cell.content()
            );
        }
    }
}

#[cfg(test)]
mod unowned_continuation {
    use super::*;

    /// A continuation nothing owns is a column nothing draws.
    ///
    /// The walk steps over a continuation together with the cell that owns
    /// it, so reaching one on its own means no cell will draw that column
    /// while the move still counts it as crossed. `Buffer::resize` and the
    /// shifting operations can both leave such a row behind.
    #[test]
    fn a_range_holding_an_unowned_continuation_is_refused() {
        let style = Style::default();
        let line = vec![Cell::new("a", 1), Cell::CONTINUATION];
        let renderer = Renderer::new();
        assert_eq!(
            renderer.overwrite_cost(CellRow::from_cells(&line), 0, 2),
            None
        );
        let mut out = Vec::new();
        assert!(!collect_overwrite_bytes(
            &mut out,
            CellRow::from_cells(&line),
            &style,
            0,
            2
        ));

        // The same columns with an owner are still offered, and draw both.
        let owned = vec![Cell::new("\u{4e16}", 2), Cell::CONTINUATION];
        assert_eq!(
            renderer.overwrite_cost(CellRow::from_cells(&owned), 0, 2),
            Some(3)
        );
        let mut out = Vec::new();
        assert!(collect_overwrite_bytes(
            &mut out,
            CellRow::from_cells(&owned),
            &style,
            0,
            2
        ));
        assert_eq!(String::from_utf8_lossy(&out), "\u{4e16}");
    }
}
