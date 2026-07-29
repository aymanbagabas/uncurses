//! Overwrite-style horizontal move candidate: when the destination row
//! is known, walking the cursor forward by re-emitting the row's own
//! cell bytes can be shorter than CUF/HPA.

use crate::cell::Cell;
use crate::style::Style;

/// Collect the UTF-8 bytes of cells in `line[from_x..to_x]` whose
/// style matches the active pen into `out`. Returns `true` when the
/// run is compatible with the pen and the bytes have been written;
/// returns `false` (and leaves `out` unchanged) otherwise.
///
/// This is a cursor *move*, not a content write: the bytes must
/// reproduce the very cells they traverse (the pen check guarantees
/// that), so the only additional requirement is that they advance the
/// cursor by exactly `to_x - from_x` columns. Anything that cannot
/// satisfy that — a range starting on a continuation, a wide cell
/// straddling `to_x`, a cell with no glyph bytes to re-emit, or a pen
/// change — refuses the candidate and lets the caller fall back to CUF.
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
    if !crate::ansi::cost::overwrite_eligible(line, style, from, to) {
        return false;
    }
    out.clear();
    let mut i = from;
    while i < to {
        let cell = &line[i];
        out.extend_from_slice(cell.content().as_bytes());
        // `max(1)` per the row-walker convention: eligibility already
        // rejected width-0 cells, and this keeps a future caller that
        // reaches here with one from hanging the renderer outright.
        i += cell.width().max(1) as usize;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::cost;

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
        let accepted = collect_overwrite_bytes(&mut out, &line, &style, 0, 8);
        assert!(!accepted, "OOB range must return false in release");
        assert!(out.is_empty());
    }

    #[test]
    fn in_bounds_pen_match_writes_bytes() {
        let line = vec![Cell::new("x", 1); 3];
        let style = Style::default();
        let mut out = Vec::new();
        assert!(collect_overwrite_bytes(&mut out, &line, &style, 0, 3));
        assert_eq!(out, b"xxx");
    }

    /// Every accepted candidate must emit exactly `to - from` columns
    /// worth of cursor advance, otherwise the renderer's model column
    /// drifts from the terminal's and the row keeps a stale cell.
    #[test]
    fn accepted_candidates_advance_exactly_the_requested_columns() {
        let style = Style::default();
        // `line` = [ 'a', '漢', <cont>, 'b' ]
        let line = vec![
            Cell::new("a", 1),
            Cell::new("漢", 2),
            Cell::new("", 0),
            Cell::new("b", 1),
        ];

        // A wide cell is overwritable like any other: one glyph, two
        // columns, landing exactly on `to`. Cost is its 3 UTF-8 bytes,
        // not its 2 columns — the emit pass writes bytes.
        let mut out = Vec::new();
        assert!(collect_overwrite_bytes(&mut out, &line, &style, 1, 3));
        assert_eq!(out, "漢".as_bytes());
        assert_eq!(cost::overwrite_cost(&line, &style, 1, 3), Some(3));

        // ...and it still composes with its narrow neighbours.
        assert!(collect_overwrite_bytes(&mut out, &line, &style, 0, 4));
        assert_eq!(out, "a漢b".as_bytes());
        assert_eq!(cost::overwrite_cost(&line, &style, 0, 4), Some(5));

        // Starting on a continuation emitted zero bytes while claiming
        // one column — and `overwrite_cost` scored it as a free move,
        // so it beat every real CUF/HPA candidate.
        assert!(!collect_overwrite_bytes(&mut out, &line, &style, 2, 3));
        assert_eq!(cost::overwrite_cost(&line, &style, 2, 3), None);

        // A wide cell straddling `to` would overshoot by one column.
        assert!(!collect_overwrite_bytes(&mut out, &line, &style, 1, 2));
        assert_eq!(cost::overwrite_cost(&line, &style, 1, 2), None);

        // Blanks carry a `" "`, so an empty narrow cell is malformed —
        // it would emit nothing yet occupy a column. Refuse rather than
        // substitute a space, which would rewrite content this path has
        // no business touching.
        let malformed = vec![Cell::new("", 1), Cell::new("x", 1)];
        assert!(!collect_overwrite_bytes(&mut out, &malformed, &style, 0, 2));
        assert_eq!(cost::overwrite_cost(&malformed, &style, 0, 2), None);

        // Real blanks are ordinary overwrite material.
        let blanks = vec![Cell::new(" ", 1); 2];
        assert!(collect_overwrite_bytes(&mut out, &blanks, &style, 0, 2));
        assert_eq!(out, b"  ");
        assert_eq!(cost::overwrite_cost(&blanks, &style, 0, 2), Some(2));
    }
}
