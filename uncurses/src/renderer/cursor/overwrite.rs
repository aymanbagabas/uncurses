//! Overwrite-style horizontal move candidate: when the destination row
//! is known, walking the cursor forward by re-emitting the row's own
//! cell bytes can be shorter than CUF/HPA.

use crate::cell::Cell;
use crate::style::Style;

/// Collect the UTF-8 bytes of cells in `line[from_x..to_x]` whose
/// style matches the active pen into `out`. Returns `true` when the
/// run is compatible with the pen and the bytes have been written;
/// returns `false` (and leaves `out` unchanged) when a width>0 cell
/// would require a pen change, or when the requested column range
/// extends past the row. Continuation cells (`width == 0`) are
/// silently skipped.
pub(in crate::renderer) fn collect_overwrite_bytes(
    arena: &dyn crate::renderer::packed::arena::Arena,
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
    let mut i = from;
    while i < to {
        let cell = &line[i];
        if !cell.is_continuation() {
            if &cell.style.style != style {
                return false;
            }
            i += cell.width() as usize;
            continue;
        }
        i += 1;
    }
    out.clear();
    let mut i = from;
    while i < to {
        let cell = &line[i];
        if !cell.is_continuation() {
            let _ = std::fmt::Write::write_fmt(out_str, format_args!("{cell}"));
            i += cell.width() as usize;
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
        let line = vec![Cell::narrow('a'); 4];
        let style = Style::default();
        let mut out = Vec::new();
        let accepted = collect_overwrite_bytes(
            crate::renderer::packed::arena::global_ref(),
            &mut out,
            &line,
            &style,
            0,
            8,
        );
        assert!(!accepted, "OOB range must return false in release");
        assert!(out.is_empty());
    }

    #[test]
    fn in_bounds_pen_match_writes_bytes() {
        let line = vec![Cell::narrow('x'); 3];
        let style = Style::default();
        let mut out = Vec::new();
        assert!(collect_overwrite_bytes(
            crate::renderer::packed::arena::global_ref(),
            &mut out,
            &line,
            &style,
            0,
            3
        ));
        assert_eq!(out, b"xxx");
    }
}
