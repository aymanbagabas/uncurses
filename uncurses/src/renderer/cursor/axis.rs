//! Per-axis sequence builders used by [`Renderer::relative_cursor_move`].
//!
//! Each axis runs in two passes: a pure cost pass selects the
//! shortest-shape candidate without touching any byte buffer, then an
//! emit pass writes the chosen shape to `out`. The vertical leg may
//! reset the column (under ONLCR with a `\n` win), so its plan carries
//! the post-vertical column for the horizontal leg to consume.

use std::io::{self, Write};

use crate::ansi::{cost, cursor};
use crate::cell::Cell;
use crate::renderer::Renderer;
use crate::renderer::caps::Optimizations;

/// Shape selected for the vertical leg.
#[derive(Clone, Copy, Debug)]
pub(super) enum VerticalShape {
    /// No vertical movement required.
    None,
    /// Absolute row jump via VPA (`\x1b[{ty+1}d`).
    Vpa { ty: u16 },
    /// Relative cursor-down by `n` rows.
    Cud { n: u16 },
    /// Relative cursor-up by `n` rows.
    Cuu { n: u16 },
    /// Reverse Index (single-step up, only when not at top).
    Ri,
    /// `n` literal line-feed bytes.
    Lf { n: u16 },
}

/// Result of the vertical cost pass: the shape, its byte cost, and
/// the column the horizontal leg should plan from (`Lf` under ONLCR
/// resets the column to `0`; every other shape leaves it untouched).
#[derive(Clone, Copy, Debug)]
pub(super) struct VerticalPlan {
    pub(super) shape: VerticalShape,
    pub(super) cost: usize,
    pub(super) post_fx: u16,
}

/// Forward horizontal sub-shape competing on the cell-bytes axis.
#[derive(Clone, Copy, Debug)]
pub(super) enum ForwardKind {
    Cuf { n: u16 },
    Overwrite { fx: u16, tx: u16 },
    None,
}

/// Shape selected for the horizontal leg.
#[derive(Clone, Copy, Debug)]
pub(super) enum HorizontalShape {
    None,
    /// Absolute column jump (HPA or CHA) — caller-chosen which.
    Hpa {
        tx: u16,
    },
    Cha {
        tx: u16,
    },
    /// Forward leg only (no tab prefix).
    Forward(ForwardKind),
    /// `count` literal `\t` bytes followed by an optional residual
    /// forward leg from the post-tab column to `tx`.
    TabsThen {
        count: u16,
        residual: ForwardKind,
    },
    /// `\x1b[{count}I` (CHT) followed by an optional residual
    /// forward leg.
    ChtThen {
        count: u16,
        residual: ForwardKind,
    },
    /// Relative cursor-left by `n` columns.
    Cub {
        n: u16,
    },
    /// `n` literal backspaces.
    Bs {
        n: u16,
    },
    /// `\x1b[{count}Z` (CBT) followed by an optional residual
    /// backward leg.
    CbtThen {
        count: u16,
        residual_n: u16,
    },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct HorizontalPlan {
    pub(super) shape: HorizontalShape,
    pub(super) cost: usize,
}

impl Renderer {
    // -------- vertical -------------------------------------------------

    /// Cost-pass for the vertical leg. Picks the shortest shape and
    /// reports the column the horizontal leg should start from. In
    /// non-fullscreen mode, downward moves use `\n` unconditionally
    /// (regardless of byte cost) so the host scrolls as needed.
    pub(super) fn plan_vertical_cost(&self, fy: u16, ty: u16, fx: u16) -> VerticalPlan {
        if ty == fy {
            return VerticalPlan {
                shape: VerticalShape::None,
                cost: 0,
                post_fx: fx,
            };
        }

        // Seed with VPA when absolute moves are permitted.
        let mut best_shape: Option<VerticalShape> = None;
        let mut best_cost: usize = usize::MAX;
        if !self.relative_cursor && self.opts.contains(Optimizations::VPA) {
            best_shape = Some(VerticalShape::Vpa { ty });
            best_cost = cost::vpa_cost(ty);
        }

        if ty > fy {
            let n = ty - fy;
            let cud = cost::cud_cost(n);
            if cud < best_cost {
                best_cost = cud;
                best_shape = Some(VerticalShape::Cud { n });
            }

            // LF policy: outside fullscreen, `\n` wins
            // unconditionally so the host actually scrolls when the
            // cursor would land past the last visible row. CUD only
            // moves the cursor within the existing rows — if the
            // target row doesn't exist yet, CUD silently clamps and
            // the inline TUI ends up rendering off-screen. LF at
            // the bottom margin triggers the terminal's scroll, so
            // inline mode pays the extra bytes to guarantee the
            // destination row is visible. In fullscreen the screen
            // is sized to the surface up front, so LF only wins
            // when strictly cheaper than CUD / VPA.
            let lf = cost::lf_cost(n);
            let lf_wins = !self.fullscreen || lf < best_cost;
            if lf_wins {
                best_cost = lf;
                best_shape = Some(VerticalShape::Lf { n });
            }
        } else {
            let n = fy - ty;
            let cuu = cost::cuu_cost(n);
            if cuu < best_cost {
                best_cost = cuu;
                best_shape = Some(VerticalShape::Cuu { n });
            }
            // RI is one byte cheaper than CUU(1) and only safe when
            // we are not at the very top of the screen.
            if n == 1 && fy > 1 && cost::RI_COST < best_cost {
                best_cost = cost::RI_COST;
                best_shape = Some(VerticalShape::Ri);
            }
        }

        let shape = best_shape.expect("vertical plan has at least one candidate when fy != ty");
        let post_fx = match shape {
            VerticalShape::Lf { .. } if self.opts.contains(Optimizations::ONLCR) => 0,
            _ => fx,
        };
        VerticalPlan {
            shape,
            cost: best_cost,
            post_fx,
        }
    }

    /// Emit the chosen vertical shape to `out`.
    pub(super) fn emit_vertical(&self, out: &mut Vec<u8>, shape: VerticalShape) -> io::Result<()> {
        match shape {
            VerticalShape::None => Ok(()),
            VerticalShape::Vpa { ty } => cursor::write_vpa(out, ty),
            VerticalShape::Cud { n } => cursor::write_cud(out, n),
            VerticalShape::Cuu { n } => cursor::write_cuu(out, n),
            VerticalShape::Ri => cursor::write_reverse_index(out),
            VerticalShape::Lf { n } => {
                for _ in 0..n {
                    out.write_all(b"\n")?;
                }
                Ok(())
            }
        }
    }

    // -------- horizontal -----------------------------------------------

    /// Cost-pass for the horizontal leg. Picks the shortest shape,
    /// folding the tabs / backspace capability cross product
    /// internally so the caller can plan with a single combined
    /// horizontal cost instead of looping over capability flags.
    pub(super) fn plan_horizontal_cost(
        &self,
        fx: u16,
        tx: u16,
        overwrite_line: Option<&[Cell]>,
        use_tabs: bool,
        use_backspace: bool,
    ) -> HorizontalPlan {
        if tx == fx {
            return HorizontalPlan {
                shape: HorizontalShape::None,
                cost: 0,
            };
        }

        // Seed with the absolute-column candidate when allowed.
        let mut best_shape: Option<HorizontalShape> = None;
        let mut best_cost: usize = usize::MAX;
        if !self.relative_cursor {
            if self.opts.contains(Optimizations::HPA) {
                let c = cost::hpa_cost(tx);
                if c < best_cost {
                    best_cost = c;
                    best_shape = Some(HorizontalShape::Hpa { tx });
                }
            } else if self.opts.contains(Optimizations::CHA) {
                let c = cost::cha_cost(tx);
                if c < best_cost {
                    best_cost = c;
                    best_shape = Some(HorizontalShape::Cha { tx });
                }
            }
        }

        if tx > fx {
            // Try the tabbed prefix first (mirrors the original
            // emit order). The tab walk counts how many stops fit in
            // [fx, tx], yielding a post-tab column and a residual
            // forward leg from there to tx.
            let tab_run = self.forward_tab_walk(fx, tx);
            if use_tabs && tab_run.count > 0 {
                let tab_cost = cost::tab_cost(tab_run.count);
                let (residual, residual_cost) =
                    self.forward_residual_cost(overwrite_line, tab_run.post_tab_fx, tx);
                let total = tab_cost + residual_cost;
                if total < best_cost {
                    best_cost = total;
                    best_shape = Some(HorizontalShape::TabsThen {
                        count: tab_run.count,
                        residual,
                    });
                }
            }
            if use_tabs && self.opts.contains(Optimizations::CHT) && tab_run.count > 0 {
                let cht = cost::cht_cost(tab_run.count);
                let (residual, residual_cost) =
                    self.forward_residual_cost(overwrite_line, tab_run.post_tab_fx, tx);
                let total = cht + residual_cost;
                // Only consider CHT when its prefix alone beats the
                // raw tab prefix; otherwise the raw-tab variant
                // already covers the same residual at lower cost.
                if cht < cost::tab_cost(tab_run.count) && total < best_cost {
                    best_cost = total;
                    best_shape = Some(HorizontalShape::ChtThen {
                        count: tab_run.count,
                        residual,
                    });
                }
            }

            // No-tab forward leg from fx to tx.
            let (residual, residual_cost) = self.forward_residual_cost(overwrite_line, fx, tx);
            if residual_cost < best_cost {
                best_cost = residual_cost;
                best_shape = Some(HorizontalShape::Forward(residual));
            }
        } else {
            let n = fx - tx;

            // Back-tab prefix.
            if use_tabs && self.opts.contains(Optimizations::CBT) && self.tabs.width() > 0 {
                let ts = &self.tabs;
                let mut col = fx;
                let mut cbt: u16 = 0;
                while ts.prev(col) >= tx {
                    col = ts.prev(col);
                    cbt += 1;
                    if ts.prev(col) == col || col == 0 {
                        break;
                    }
                }
                if cbt > 0 {
                    let residual_n = col - tx;
                    let residual_cost = if residual_n > 0 {
                        cost::cub_cost(residual_n)
                    } else {
                        0
                    };
                    let total = cost::cbt_cost(cbt) + residual_cost;
                    // CBT is unconditional — it is the only path
                    // that crosses tab stops backwards.
                    if total < best_cost {
                        best_cost = total;
                        best_shape = Some(HorizontalShape::CbtThen {
                            count: cbt,
                            residual_n,
                        });
                    }
                }
            }

            if n > 0 {
                let cub = cost::cub_cost(n);
                if cub < best_cost {
                    best_cost = cub;
                    best_shape = Some(HorizontalShape::Cub { n });
                }
                if use_backspace {
                    let bs = cost::bs_cost(n);
                    if bs < best_cost {
                        best_cost = bs;
                        best_shape = Some(HorizontalShape::Bs { n });
                    }
                }
            }
        }

        let shape = best_shape.expect("horizontal plan has at least one candidate when fx != tx");
        HorizontalPlan {
            shape,
            cost: best_cost,
        }
    }

    /// Emit the chosen horizontal shape to `out`.
    pub(super) fn emit_horizontal(
        &self,
        out: &mut Vec<u8>,
        shape: HorizontalShape,
        overwrite_line: Option<&[Cell]>,
    ) -> io::Result<()> {
        match shape {
            HorizontalShape::None => Ok(()),
            HorizontalShape::Hpa { tx } => cursor::write_hpa(out, tx),
            HorizontalShape::Cha { tx } => cursor::write_cha(out, tx),
            HorizontalShape::Forward(kind) => self.emit_forward(out, kind, overwrite_line),
            HorizontalShape::TabsThen { count, residual } => {
                for _ in 0..count {
                    out.write_all(b"\t")?;
                }
                self.emit_forward(out, residual, overwrite_line)
            }
            HorizontalShape::ChtThen { count, residual } => {
                cursor::write_cht(out, count)?;
                self.emit_forward(out, residual, overwrite_line)
            }
            HorizontalShape::Cub { n } => cursor::write_cub(out, n),
            HorizontalShape::Bs { n } => {
                for _ in 0..n {
                    out.write_all(b"\x08")?;
                }
                Ok(())
            }
            HorizontalShape::CbtThen { count, residual_n } => {
                cursor::write_backtab(out, count)?;
                if residual_n > 0 {
                    cursor::write_cub(out, residual_n)?;
                }
                Ok(())
            }
        }
    }

    // -------- internals ------------------------------------------------

    /// Walk the tab stops from `fx` toward `tx`, counting how many
    /// stops fit strictly within `(fx, tx]`. Returns the count and the
    /// resulting column the cursor would sit at after that many tabs.
    fn forward_tab_walk(&self, fx: u16, tx: u16) -> ForwardTabRun {
        if self.tabs.width() == 0 {
            return ForwardTabRun {
                count: 0,
                post_tab_fx: fx,
            };
        }
        let ts = &self.tabs;
        let mut count: u16 = 0;
        let mut col = fx;
        loop {
            if ts.next(col) > tx {
                break;
            }
            count += 1;
            if col == ts.next(col) || col + 1 >= ts.width() {
                break;
            }
            col = ts.next(col);
        }
        ForwardTabRun {
            count,
            post_tab_fx: col,
        }
    }

    /// Cost of the residual forward leg from `fx` to `tx`, picking
    /// between plain CUF and an in-place overwrite re-emit when
    /// `overwrite_line` is supplied and pen-compatible.
    fn forward_residual_cost(
        &self,
        overwrite_line: Option<&[Cell]>,
        fx: u16,
        tx: u16,
    ) -> (ForwardKind, usize) {
        if tx == fx {
            return (ForwardKind::None, 0);
        }
        let n = tx - fx;
        let cuf = cost::cuf_cost(n);
        let mut best_kind = ForwardKind::Cuf { n };
        let mut best_cost = cuf;
        // Overwrite emits at least one byte per cell traversed, so it
        // can only beat CUF when the forward distance is small enough
        // that `n < cuf_cost(n)`. Skip the per-cell pen walk when the
        // cost floor already loses, keeping the planner allocation- and
        // iteration-free on long forward moves.
        if (n as usize) < cuf
            && let Some(line) = overwrite_line
            && let Some(ow) = cost::overwrite_cost(line, self.cur.style(), fx, tx)
            && ow < best_cost
        {
            best_kind = ForwardKind::Overwrite { fx, tx };
            best_cost = ow;
        }
        (best_kind, best_cost)
    }

    fn emit_forward(
        &self,
        out: &mut Vec<u8>,
        kind: ForwardKind,
        overwrite_line: Option<&[Cell]>,
    ) -> io::Result<()> {
        match kind {
            ForwardKind::None => Ok(()),
            ForwardKind::Cuf { n } => cursor::write_cuf(out, n),
            ForwardKind::Overwrite { fx, tx } => {
                let line = overwrite_line
                    .expect("overwrite chosen without overwrite_line — planner contract bug");
                let mut tmp = Vec::with_capacity((tx - fx) as usize);
                let ok = super::overwrite::collect_overwrite_bytes(
                    &mut tmp,
                    line,
                    self.cur.style(),
                    fx,
                    tx,
                );
                debug_assert!(
                    ok,
                    "overwrite cost pass said eligible but emit pass disagreed (fx={fx}, tx={tx})"
                );
                if !ok {
                    // Release-build belt: refuse to silently emit
                    // nothing. Fall back to CUF so the cursor still
                    // lands at the intended column.
                    return cursor::write_cuf(out, tx - fx);
                }
                out.write_all(&tmp)
            }
        }
    }
}

struct ForwardTabRun {
    count: u16,
    post_tab_fx: u16,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::Renderer;
    use crate::renderer::caps::Optimizations;
    use crate::renderer::tabstops::TabStops;

    fn renderer() -> Renderer {
        let mut r = Renderer::new();
        r.cur.x_unknown = false;
        r.cur.y_unknown = false;
        r.last_width = 80;
        r.last_height = 24;
        r.tabs = TabStops::default_for(80);
        r
    }

    /// Every vertical shape's predicted cost matches the actual
    /// byte length the emit pass writes.
    #[test]
    fn vertical_cost_matches_emit_bytes() {
        let r = renderer();
        for fy in 0u16..6 {
            for ty in 0u16..6 {
                let plan = r.plan_vertical_cost(fy, ty, 5);
                let mut bytes = Vec::new();
                r.emit_vertical(&mut bytes, plan.shape).unwrap();
                assert_eq!(
                    plan.cost,
                    bytes.len(),
                    "vertical fy={fy} ty={ty} shape={:?} bytes={bytes:?}",
                    plan.shape
                );
            }
        }
    }

    /// Every horizontal shape's predicted cost matches the actual
    /// byte length the emit pass writes — exhaustive over the
    /// {use_tabs, use_backspace} cross product.
    #[test]
    fn horizontal_cost_matches_emit_bytes_no_overwrite() {
        let r = renderer();
        for &use_tabs in &[false, true] {
            for &use_bs in &[false, true] {
                for fx in 0u16..20 {
                    for tx in 0u16..20 {
                        let plan = r.plan_horizontal_cost(fx, tx, None, use_tabs, use_bs);
                        let mut bytes = Vec::new();
                        r.emit_horizontal(&mut bytes, plan.shape, None).unwrap();
                        assert_eq!(
                            plan.cost,
                            bytes.len(),
                            "horizontal fx={fx} tx={tx} tabs={use_tabs} bs={use_bs} shape={:?} bytes={bytes:?}",
                            plan.shape
                        );
                    }
                }
            }
        }
    }

    /// ASCII overwrite candidate: predicted cost equals emitted bytes
    /// exactly. Wide / multi-byte cells under-predict (documented in
    /// the cost helper); we don't assert byte equality for those.
    #[test]
    fn horizontal_cost_matches_emit_bytes_ascii_overwrite() {
        use crate::cell::Cell;
        let r = renderer();
        // Cells match the active pen so overwrite is eligible.
        let line: Vec<Cell> = (0..20)
            .map(|i| {
                Cell::narrow(((b'a' + (i as u8 % 26)) as char).to_string())
                    .style(r.cur.style().clone())
            })
            .collect();
        for fx in 0u16..20 {
            for tx in fx..20 {
                let plan = r.plan_horizontal_cost(fx, tx, Some(&line), false, false);
                let mut bytes = Vec::new();
                r.emit_horizontal(&mut bytes, plan.shape, Some(&line))
                    .unwrap();
                assert_eq!(
                    plan.cost,
                    bytes.len(),
                    "ASCII overwrite fx={fx} tx={tx} shape={:?} bytes={bytes:?}",
                    plan.shape
                );
            }
        }
    }

    /// Inline / non-fullscreen downward moves use `\n` even when a
    /// shorter CUD or VPA exists, so the host scrolls correctly.
    #[test]
    fn inline_downward_always_uses_lf() {
        let mut r = renderer();
        r.fullscreen = false;
        r.set_relative_cursor(false);
        r.opts.insert(Optimizations::VPA);
        // Pick a row distance where CUD(7) = 4 bytes < LF*7 = 7 bytes.
        let plan = r.plan_vertical_cost(0, 7, 0);
        assert!(matches!(plan.shape, VerticalShape::Lf { n: 7 }));
        assert_eq!(plan.cost, 7);
    }

    /// Fullscreen downward moves let CUD beat LF when shorter.
    #[test]
    fn fullscreen_downward_picks_shorter_of_lf_and_cud() {
        let mut r = renderer();
        r.fullscreen = true;
        let plan = r.plan_vertical_cost(0, 10, 0);
        // CUD(10) = "\x1b[10B" = 5 bytes vs LF*10 = 10 bytes.
        assert!(matches!(plan.shape, VerticalShape::Cud { n: 10 }));
    }
}
