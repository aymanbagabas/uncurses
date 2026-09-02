//! Byte-length predictors for emitted ANSI sequences.
//!
//! ## Category
//!
//! Cost functions mirror writer functions in [`crate::ansi::cursor`] and
//! [`crate::ansi::screen`]. They let render planning compare equivalent cursor,
//! erase, scroll, and overwrite strategies without allocating formatted strings.
//!
//! ## Conventions
//!
//! Costs are byte counts for the exact 7-bit sequences emitted by this crate,
//! including omitted default parameters such as `ESC [ A` for `CUU 1` and
//! `ESC [ H` for home.
//!
//! ## Mode interaction
//!
//! The predictors do not change terminal modes. For mode-sensitive sequences,
//! such as left/right margins requiring [`Mode::LEFT_RIGHT_MARGIN`](crate::ansi::mode::Mode::LEFT_RIGHT_MARGIN), the cost still describes only the bytes emitted.

/// Bytes in a CSI introducer (`ESC [`).
const CSI_LEN: usize = 2;

/// Number of decimal digits in a `u16`. Branchy table — faster than
/// `ilog10` on the small range we feed it (cursor coordinates,
/// repeat counts).
pub const fn digit_count(n: u16) -> usize {
    if n < 10 {
        1
    } else if n < 100 {
        2
    } else if n < 1000 {
        3
    } else if n < 10000 {
        4
    } else {
        5
    }
}

/// Cost of a single-parameter CSI where `n == 1` omits the parameter
/// (the writer emits `ESC [ X` instead of `ESC [ 1 X`). Final byte
/// contributes 1.
///
/// Shape: `\x1b[X` for `n <= 1`, `\x1b[{n}X` otherwise.
const fn csi_optional_param_cost(n: u16) -> usize {
    if n <= 1 {
        CSI_LEN + 1
    } else {
        CSI_LEN + digit_count(n) + 1
    }
}

/// Cost of a single-parameter CSI whose parameter is dropped when
/// the 0-based input value is `0` (the canonical "absolute
/// position" shape: CHA, HPA, VPA all default to row/col 1, which
/// corresponds to a 0-based input of 0).
///
/// Shape: `\x1b[X` for `arg == 0`, `\x1b[{arg+1}X` otherwise.
const fn csi_absolute_position_cost(arg: u16) -> usize {
    if arg == 0 {
        CSI_LEN + 1
    } else {
        CSI_LEN + digit_count(arg + 1) + 1
    }
}

/// Cost of an erase-style CSI where the default parameter is `0`
/// (the writer emits `ESC [ X` for `n == 0` and `ESC [ {n} X`
/// otherwise).
///
/// Shape: `\x1b[X` for `n == 0`, `\x1b[{n}X` otherwise.
const fn csi_erase_cost(n: u16) -> usize {
    if n == 0 {
        CSI_LEN + 1
    } else {
        CSI_LEN + digit_count(n) + 1
    }
}

// ---------- Cursor movement -------------------------------------------------

/// Cost of a CUP sequence for the given 0-based position. The writer
/// drops the column when it is `0`, and drops both parameters when
/// the destination is the origin.
pub const fn cup_cost(row: u16, col: u16) -> usize {
    if row == 0 && col == 0 {
        // \x1b[H
        CSI_LEN + 1
    } else if col == 0 {
        // \x1b[{row+1}H
        CSI_LEN + digit_count(row + 1) + 1
    } else {
        // \x1b[{row+1};{col+1}H
        CSI_LEN + digit_count(row + 1) + 1 + digit_count(col + 1) + 1
    }
}

/// Cost of CUU (`\x1b[{n?}A`).
pub const fn cuu_cost(n: u16) -> usize {
    csi_optional_param_cost(n)
}

/// Cost of CUD (`\x1b[{n?}B`).
pub const fn cud_cost(n: u16) -> usize {
    csi_optional_param_cost(n)
}

/// Cost of CUF (`\x1b[{n?}C`).
pub const fn cuf_cost(n: u16) -> usize {
    csi_optional_param_cost(n)
}

/// Cost of CUB (`\x1b[{n?}D`).
pub const fn cub_cost(n: u16) -> usize {
    csi_optional_param_cost(n)
}

/// Cost of CHA (`\x1b[{col+1?}G`). The writer drops the parameter
/// when `col == 0`.
pub const fn cha_cost(col: u16) -> usize {
    csi_absolute_position_cost(col)
}

/// Cost of HPA (``\x1b[{col+1?}` ``). The writer drops the parameter
/// when `col == 0`.
pub const fn hpa_cost(col: u16) -> usize {
    csi_absolute_position_cost(col)
}

/// Cost of VPA (`\x1b[{row+1?}d`). The writer drops the parameter
/// when `row == 0`.
pub const fn vpa_cost(row: u16) -> usize {
    csi_absolute_position_cost(row)
}

/// Cost of CHT (`\x1b[{n?}I`).
pub const fn cht_cost(n: u16) -> usize {
    csi_optional_param_cost(n)
}

/// Cost of CBT (`\x1b[{n?}Z`).
pub const fn cbt_cost(n: u16) -> usize {
    csi_optional_param_cost(n)
}

/// Cost of Reverse Index (`\x1bM`).
pub const RI_COST: usize = 2;

// ---------- Cell operations -------------------------------------------------

/// Cost of ICH (`\x1b[{n?}@`).
pub const fn ich_cost(n: u16) -> usize {
    csi_optional_param_cost(n)
}

/// Cost of DCH (`\x1b[{n?}P`).
pub const fn dch_cost(n: u16) -> usize {
    csi_optional_param_cost(n)
}

/// Cost of ECH (`\x1b[{n?}X`).
pub const fn ech_cost(n: u16) -> usize {
    csi_optional_param_cost(n)
}

/// Cost of REP (`\x1b[{n?}b`).
pub const fn rep_cost(n: u16) -> usize {
    csi_optional_param_cost(n)
}

// ---------- Line operations -------------------------------------------------

/// Cost of IL (`\x1b[{n?}L`).
pub const fn il_cost(n: u16) -> usize {
    csi_optional_param_cost(n)
}

/// Cost of DL (`\x1b[{n?}M`).
pub const fn dl_cost(n: u16) -> usize {
    csi_optional_param_cost(n)
}

/// Cost of SU (`\x1b[{n?}S`).
pub const fn su_cost(n: u16) -> usize {
    csi_optional_param_cost(n)
}

/// Cost of SD (`\x1b[{n?}T`).
pub const fn sd_cost(n: u16) -> usize {
    csi_optional_param_cost(n)
}

// ---------- Erase -----------------------------------------------------------

/// Cost of EL (`\x1b[{n?}K`). `n == 0` omits the parameter.
pub const fn el_cost(n: u16) -> usize {
    csi_erase_cost(n)
}

/// Cost of ED (`\x1b[{n?}J`). `n == 0` omits the parameter.
pub const fn ed_cost(n: u16) -> usize {
    csi_erase_cost(n)
}

// ---------- Scroll region ---------------------------------------------------

/// Cost of DECSTBM (`\x1b[{top+1};{bottom+1}r`).
pub const fn decstbm_cost(top: u16, bottom: u16) -> usize {
    // CSI + d(top+1) + ';' + d(bottom+1) + 'r'
    CSI_LEN + digit_count(top + 1) + 1 + digit_count(bottom + 1) + 1
}

/// Cost of resetting DECSTBM to the full screen (`\x1b[r`).
pub const DECSTBM_RESET_COST: usize = 3;

// ---------- C0 / fixed prefixes --------------------------------------------

/// Cost of a carriage return (`\r`).
pub const CR_COST: usize = 1;

/// Cost of an absolute home jump (`\x1b[H`).
pub const HOME_COST: usize = 3;

/// Cost of `n` literal line-feed bytes.
pub const fn lf_cost(n: u16) -> usize {
    n as usize
}

/// Cost of `n` literal backspace bytes.
pub const fn bs_cost(n: u16) -> usize {
    n as usize
}

/// Cost of `n` literal tab bytes.
pub const fn tab_cost(n: u16) -> usize {
    n as usize
}

// ---------- Overwrite (re-emit row cells as a forward move) ----------------

/// Whether `content` draws exactly `want` columns however the terminal
/// measures it.
///
/// [`overwrite_cost`] and the emit pass both travel by drawing, and neither
/// knows the width policy the row was painted under. Refusing a cell whose
/// width moves with the policy costs a cursor sequence; accepting one lands
/// the cursor somewhere the planner did not record.
pub(crate) fn crossable_under_every_policy(content: &str, want: usize) -> bool {
    use crate::text::WidthMode;
    // Printable ASCII is one column under every policy, and it is what the
    // planner meets on nearly every candidate, so answer it without four
    // table lookups.
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

/// Byte cost of re-emitting the cells in `line[from..to]` as a
/// forward-move candidate.
///
/// The cost is the summed byte length of the cells' content, which is what
/// the move writes, so it compares directly against the byte cost of a
/// cursor sequence.
///
/// Returns [`None`] when the range is not eligible. This move travels by
/// drawing, so what it draws has to span exactly the columns it claims to
/// cross, and the pen has to survive the trip:
///
/// - the range opens on a continuation, which begins inside a glyph whose
///   bytes have already gone out, so the walk would arrive a column short
/// - the last cluster in the range reaches past `to`, so drawing it would
///   carry the cursor a column too far
/// - an occupied cell has a style or link the active pen does not match,
///   which would need an interleaved pen change
/// - an occupied cell holds anything other than exactly one code point,
///   which keeps the byte cost equal to the columns crossed and keeps the
///   candidate off sequences whose rendered width a terminal may disagree
///   about
///
/// The renderer decides eligibility the same way when it emits the move,
/// and the two have to agree or the planner picks a move it cannot
/// produce.
pub fn overwrite_cost(
    line: &[crate::cell::Cell],
    style: &crate::style::Style,
    from: u16,
    to: u16,
) -> Option<usize> {
    let from = from as usize;
    let to = to as usize;
    if to > line.len() {
        return None;
    }
    // This candidate travels by drawing, so it is only usable when what it
    // draws spans exactly the columns it claims to cross. Opening inside a
    // glyph leaves nothing to draw for that column, and a last cluster
    // reaching past `to` draws a column too many; either way the cursor
    // stops somewhere other than the planner records. The same rule decides
    // eligibility in the emit pass, and the two have to agree.
    if line
        .get(from)
        .is_some_and(crate::cell::Cell::is_continuation)
    {
        return None;
    }
    let mut i = from;
    let mut cost = 0usize;
    while i < to {
        let cell = &line[i];
        if !cell.is_continuation() {
            if &cell.style != style {
                return None;
            }
            let content = cell.content();
            // A cell with nothing to write still owns its column, so drawing
            // it advances the cursor by nothing while the planner records a
            // column crossed. A cluster of several code points has the
            // opposite problem: it costs what it takes to write, which is
            // its bytes, and a joined emoji is closer to thirty of those
            // than to the two columns it occupies. Taking one code point at
            // a time keeps the price and the distance equal, and keeps the
            // move off sequences whose rendered width a terminal may not
            // agree about. Asking for a second code point answers both
            // without walking the rest of a long one.
            // The cell has to draw the columns the row credits it with, or
            // the walk arrives somewhere the planner did not record. A cell
            // can claim a width its content does not have: a combining mark
            // or a control character draws nothing, a narrow cell can hold a
            // wide glyph, and a wide one can hold a narrow glyph. Measuring
            // the content answers that directly, where counting code points
            // only guessed at it.
            // The policy the row was measured under is not known here, so a
            // cell is crossable only when its content draws the same number
            // of columns under every policy a terminal may be using. A cell
            // whose width is policy-dependent -- an East Asian Ambiguous
            // glyph, or a regional indicator that `Wc` and `Grapheme`
            // disagree about -- is refused rather than guessed at, because
            // guessing wrong here accepts the move instead of declining it.
            if !crossable_under_every_policy(content, usize::from(cell.width().max(1))) {
                return None;
            }
            // Beyond that, one code point at a time, which keeps the move
            // off sequences whose rendered width a terminal may not agree
            // about however the table measures them.
            let mut code_points = content.chars();
            if code_points.next().is_none() || code_points.next().is_some() {
                return None;
            }
            cost += content.len();
            let w = cell.width().max(1) as usize;
            i += w;
            continue;
        }
        // Reaching a continuation here means nothing stepped over it, so no
        // cell owns it and nothing will draw its column, while the walk
        // would still count the column as crossed.
        return None;
    }
    if i != to {
        return None;
    }
    Some(cost)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cell whose rendered width moves with the terminal's width policy
    /// cannot be crossed by drawing, because the planner records
    /// `cell.width()` columns either way.
    #[test]
    fn an_overwrite_declines_a_cell_whose_width_depends_on_the_policy() {
        use crate::cell::Cell;
        use crate::style::Style;
        use crate::text::WidthMode;

        let dot = "\u{00b7}";
        assert_eq!(WidthMode::Grapheme.grapheme_width(dot, false), 1);
        assert_eq!(WidthMode::Grapheme.grapheme_width(dot, true), 2);

        let flag = "\u{1f1ef}";
        assert_eq!(WidthMode::Grapheme.grapheme_width(flag, false), 2);
        assert_eq!(WidthMode::Wc.grapheme_width(flag, false), 1);

        let style = Style::default();
        assert_eq!(
            overwrite_cost(&[Cell::narrow(dot), Cell::narrow("a")], &style, 0, 2),
            None,
            "ambiguous width draws two columns in a CJK locale"
        );
        assert_eq!(
            overwrite_cost(&[Cell::wide(flag), Cell::continuation()], &style, 0, 2),
            None,
            "a lone regional indicator draws one column under wcwidth"
        );
        // A cell every policy agrees on is still crossable.
        assert!(
            overwrite_cost(&[Cell::narrow("a"), Cell::narrow("b")], &style, 0, 2).is_some(),
            "plain ASCII must remain crossable"
        );
    }

    use crate::ansi::cursor::{
        write_backtab, write_cha, write_cht, write_cub, write_cud, write_cuf, write_cup, write_cuu,
        write_hpa, write_reverse_index, write_vpa,
    };
    use crate::ansi::screen::{
        write_dch, write_delete_lines, write_ech, write_ed, write_el, write_ich,
        write_insert_lines, write_rep, write_reset_scroll_region, write_scroll_down,
        write_scroll_region, write_scroll_up,
    };

    /// Representative values exercising every branch of `digit_count`:
    /// the `n == 0` / `n == 1` shortcuts, and 1..5 digit widths.
    const NS: &[u16] = &[0, 1, 2, 9, 10, 99, 100, 999, 1000, 9999, 10000, u16::MAX];

    fn write_to_vec(f: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let mut v = Vec::new();
        f(&mut v);
        v
    }

    #[test]
    fn digit_count_matches_decimal_width() {
        for &n in NS {
            assert_eq!(digit_count(n), n.to_string().len(), "digit_count({n})");
        }
    }

    #[test]
    fn cup_cost_round_trips() {
        let coords = [
            (0, 0),
            (0, 5),
            (5, 0),
            (5, 7),
            (99, 99),
            (999, 9),
            (10, 1000),
        ];
        for (r, c) in coords {
            let bytes = write_to_vec(|v| write_cup(v, r, c).unwrap());
            assert_eq!(
                cup_cost(r, c),
                bytes.len(),
                "cup_cost({r},{c}) vs {bytes:?}"
            );
        }
    }

    /// Drive the optional-param shape through every parameterized
    /// writer that consumes it. Each call asserts that the cost
    /// helper agrees with the writer's actual output.
    #[test]
    fn optional_param_costs_round_trip() {
        for &n in NS {
            // `write_cuu` and friends are no-ops for `n == 0`; that
            // is the writer's own contract and we don't predict a
            // cost for the empty emission.
            if n == 0 {
                continue;
            }

            let pairs: &[(&str, usize, Vec<u8>)] = &[
                (
                    "cuu",
                    cuu_cost(n),
                    write_to_vec(|v| write_cuu(v, n).unwrap()),
                ),
                (
                    "cud",
                    cud_cost(n),
                    write_to_vec(|v| write_cud(v, n).unwrap()),
                ),
                (
                    "cuf",
                    cuf_cost(n),
                    write_to_vec(|v| write_cuf(v, n).unwrap()),
                ),
                (
                    "cub",
                    cub_cost(n),
                    write_to_vec(|v| write_cub(v, n).unwrap()),
                ),
                (
                    "cht",
                    cht_cost(n),
                    write_to_vec(|v| write_cht(v, n).unwrap()),
                ),
                (
                    "cbt",
                    cbt_cost(n),
                    write_to_vec(|v| write_backtab(v, n).unwrap()),
                ),
                (
                    "ich",
                    ich_cost(n),
                    write_to_vec(|v| write_ich(v, n).unwrap()),
                ),
                (
                    "dch",
                    dch_cost(n),
                    write_to_vec(|v| write_dch(v, n).unwrap()),
                ),
                (
                    "ech",
                    ech_cost(n),
                    write_to_vec(|v| write_ech(v, n).unwrap()),
                ),
                (
                    "rep",
                    rep_cost(n),
                    write_to_vec(|v| write_rep(v, n).unwrap()),
                ),
                (
                    "il",
                    il_cost(n),
                    write_to_vec(|v| write_insert_lines(v, n).unwrap()),
                ),
                (
                    "dl",
                    dl_cost(n),
                    write_to_vec(|v| write_delete_lines(v, n).unwrap()),
                ),
                (
                    "su",
                    su_cost(n),
                    write_to_vec(|v| write_scroll_up(v, n).unwrap()),
                ),
                (
                    "sd",
                    sd_cost(n),
                    write_to_vec(|v| write_scroll_down(v, n).unwrap()),
                ),
            ];
            for (label, cost, bytes) in pairs {
                assert_eq!(*cost, bytes.len(), "{label}({n}) bytes={bytes:?}");
            }
        }
    }

    #[test]
    fn required_param_costs_round_trip() {
        // u16::MAX overflows the writers' `arg + 1`; cap at 9999 (5
        // digits exercise the wide branch). The `digit_count` test
        // above already covers u16::MAX.
        for &col in NS.iter().filter(|&&n| n <= 9999) {
            let cha = write_to_vec(|v| write_cha(v, col).unwrap());
            assert_eq!(cha_cost(col), cha.len(), "cha_cost({col})");
            let hpa = write_to_vec(|v| write_hpa(v, col).unwrap());
            assert_eq!(hpa_cost(col), hpa.len(), "hpa_cost({col})");
            let vpa = write_to_vec(|v| write_vpa(v, col).unwrap());
            assert_eq!(vpa_cost(col), vpa.len(), "vpa_cost({col})");
        }
    }

    #[test]
    fn erase_costs_round_trip() {
        for n in 0u16..=3 {
            let el = write_to_vec(|v| write_el(v, n).unwrap());
            assert_eq!(el_cost(n), el.len(), "el_cost({n})");
            let ed = write_to_vec(|v| write_ed(v, n).unwrap());
            assert_eq!(ed_cost(n), ed.len(), "ed_cost({n})");
        }
    }

    #[test]
    fn decstbm_cost_round_trips() {
        let cases = [(0u16, 0u16), (0, 23), (4, 19), (99, 199), (999, 9999)];
        for (top, bottom) in cases {
            let bytes = write_to_vec(|v| write_scroll_region(v, top, bottom).unwrap());
            assert_eq!(
                decstbm_cost(top, bottom),
                bytes.len(),
                "decstbm({top},{bottom})"
            );
        }
    }

    #[test]
    fn decstbm_reset_cost_matches_writer() {
        let bytes = write_to_vec(|v| write_reset_scroll_region(v).unwrap());
        assert_eq!(DECSTBM_RESET_COST, bytes.len());
    }

    #[test]
    fn ri_cost_matches_writer() {
        let bytes = write_to_vec(|v| write_reverse_index(v).unwrap());
        assert_eq!(RI_COST, bytes.len());
    }
}
