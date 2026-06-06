//! Byte-length predictors for the ANSI escape sequences the renderer
//! emits. Used by the cursor planner and the per-line transform to
//! pick the shortest of several equivalent emissions without actually
//! formatting them.
//!
//! Every helper is paired with a `write_*` builder in
//! [`crate::ansi::cursor`] or [`crate::ansi::screen`]; the test
//! module asserts that `cost(args) == write(args).len()` for every
//! representative input so the two cannot silently drift.

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

/// Approximate byte cost of re-emitting the cells in `line[from..to]`
/// as a forward-move candidate.
///
/// Returns [`None`] when any `width > 0` cell in the range has a
/// style or link that does not match the active pen — in that case
/// the row cannot be re-emitted without an interleaved pen change
/// and the candidate is not eligible.
///
/// The cost approximation is the sum of `cell.width()` over occupied
/// cells in the range. For plain ASCII this equals the emitted byte
/// length exactly. For wide CJK glyphs (`width == 2`, multi-byte
/// content) and other multi-byte single-width glyphs (combining
/// sequences, emoji selectors) the prediction underestimates the
/// emitted length, so an overwrite candidate may be picked here that
/// a strict byte minimisation would have rejected. Accepting this
/// approximation keeps the predictor allocation-free.
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
    let mut i = from;
    let mut cost = 0usize;
    while i < to {
        let cell = &line[i];
        if cell.is_rect() {
            // Rect cells carry an opaque payload that may only be
            // emitted at the anchor position. Replaying it through
            // an overwrite walk would print a DCS sequence at the
            // wrong cursor location and corrupt the screen.
            return None;
        }
        if cell.width() > 0 {
            if cell.style() != style {
                return None;
            }
            cost += cell.width() as usize;
            i += cell.width() as usize;
            continue;
        }
        i += 1;
    }
    Some(cost)
}

#[cfg(test)]
mod tests {
    use super::*;
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
