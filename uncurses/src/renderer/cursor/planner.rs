//! Cursor planner: byte-cost-optimal absolute move emission.
//!
//! Each candidate's byte cost is computed analytically from the
//! [`crate::ansi::cost`] helpers; only the winning candidate is
//! materialised to bytes. Candidate enumeration order matches the
//! historical emit order so ties resolve identically (strict `<`
//! comparison keeps the earlier candidate).
//!
//! One emission sits outside the cost model: the pen reset that has to
//! precede a `\n` which can scroll (see
//! [`Renderer::lf_would_bleed_background`]). Pricing it would not change
//! any decision — an inline downward move picks `\n` regardless of cost
//! — and its length depends on the pen rather than the geometry, so the
//! per-shape "predicted cost equals emitted bytes" invariant is kept on
//! the shapes themselves.

use std::io::{self, Write};

use crate::ansi::{cost, cursor};
use crate::cell::Cell;
use crate::layout::Position;
use crate::renderer::Renderer;
use crate::renderer::caps::Optimizations;

use super::axis::VerticalShape;
use super::relative::RelativePlan;
use crate::renderer::frame::emit::PenPolicy;

/// Prefix prepended to a relative-move candidate.
#[derive(Clone, Copy, Debug)]
enum PrefixShape {
    None,
    Cr,
    Home,
}

impl PrefixShape {
    fn cost(self) -> usize {
        match self {
            PrefixShape::None => 0,
            PrefixShape::Cr => cost::CR_COST,
            PrefixShape::Home => cost::HOME_COST,
        }
    }

    fn apply_to(self, from: Position) -> Position {
        match self {
            PrefixShape::None => from,
            PrefixShape::Cr => Position { y: from.y, x: 0 },
            PrefixShape::Home => Position { y: 0, x: 0 },
        }
    }

    fn emit(self, out: &mut Vec<u8>) -> io::Result<()> {
        match self {
            PrefixShape::None => Ok(()),
            PrefixShape::Cr => out.write_all(b"\r"),
            PrefixShape::Home => out.write_all(b"\x1b[H"),
        }
    }
}

impl Renderer {
    /// Choose and emit the byte-cost-optimal cursor move.
    ///
    /// # Parameters
    ///
    /// - `out`: destination byte buffer.
    /// - `from`: renderer-tracked starting position.
    /// - `to`: target position.
    /// - `overwrite_line`: optional destination row cells. When present,
    ///   re-emitting matching cells can compete as a forward horizontal
    ///   move.
    /// - `pen`: whether the move may reset the pen before a `\n` that
    ///   can scroll. See [`PenPolicy`].
    ///
    /// # Behavior
    ///
    /// Same-position moves are no-ops when both cursor axes are known.
    /// In absolute mode, long-distance moves or unknown source axes force
    /// CUP immediately. Otherwise the planner enumerates eligible
    /// relative shapes by byte cost and emits only the winner.
    ///
    /// # Errors
    ///
    /// Propagates writes to `out`; with `Vec<u8>` output this is
    /// effectively infallible.
    ///
    /// In absolute mode, CUP wins outright for non-local moves; for
    /// local moves CUP seeds the candidate list (so it wins ties
    /// against relative shapes). In relative mode, the planner
    /// enumerates the {no-prefix, `\r`-prefix} × {tabs, backspace} cross
    /// product by cost and emits only the winner. (The `\x1b[H` home prefix
    /// is an absolute-mode-only candidate.)
    pub(crate) fn write_optimal_move(
        &mut self,
        out: &mut Vec<u8>,
        from: Position,
        to: Position,
        overwrite_line: Option<&[Cell]>,
    ) -> io::Result<()> {
        self.write_optimal_move_with_pen(
            out,
            from,
            to,
            overwrite_line,
            PenPolicy::ResetBeforeScroll,
        )
    }

    /// [`Renderer::write_optimal_move`] with an explicit pen policy.
    pub(crate) fn write_optimal_move_with_pen(
        &mut self,
        out: &mut Vec<u8>,
        from: Position,
        to: Position,
        overwrite_line: Option<&[Cell]>,
        pen: PenPolicy,
    ) -> io::Result<()> {
        if from.y == to.y && from.x == to.x && self.cur.known() {
            return Ok(());
        }

        // CUP fast path in absolute mode: long-distance jump or stale
        // tracked position dominates outright; emit it without
        // running the cost search.
        let force_cup = !self.relative_cursor
            && (!self.cur.known()
                || self.last_width == 0
                || super::not_local(self.last_width, from, to));
        if force_cup {
            return cursor::write_cup(out, to.y, to.x);
        }

        let mut best = self.plan_move(from, to, overwrite_line);

        // A `\n` that scrolls the host carries the active pen into the
        // row it exposes (see [`Renderer::lf_would_bleed_background`]).
        // Reset the pen first, then re-plan: the horizontal leg's
        // overwrite candidate is only eligible for cells matching the
        // active pen, so it has to be judged against the pen that will
        // actually be in effect when the bytes land.
        if pen == PenPolicy::ResetBeforeScroll && self.lf_would_bleed_background(&best) {
            self.reset_pen(out)?;
            best = self.plan_move(from, to, overwrite_line);
        }

        match best {
            Winner::Cup { .. } => cursor::write_cup(out, to.y, to.x),
            Winner::Relative { prefix, plan, .. } => {
                prefix.emit(out)?;
                self.emit_relative_plan(out, &plan, overwrite_line)
            }
        }
    }

    /// Whether emitting `best` would drag a non-default background into
    /// a row exposed by scrolling.
    ///
    /// Inline downward moves are emitted as literal `\n` regardless of
    /// byte cost (see [`Renderer::plan_vertical_cost`]) so the host
    /// scrolls when the target row does not exist yet. On a terminal
    /// with back-color erase that scroll paints the freshly exposed row
    /// with the pen's background, and — unlike the deliberate scrolls in
    /// [`crate::renderer::scroll`], which record the painted blank in
    /// `cur_buf` — nothing here tells the frame model that the row
    /// changed. The next diff therefore sees no work to do and the
    /// stray background never gets repaired.
    ///
    /// Three conditions narrow it to the rows that can actually bleed:
    ///
    /// - **Inline only.** A fullscreen surface is sized to the screen and
    ///   [`Renderer::move_to`] clamps the target row, so `\n` never
    ///   reaches the bottom margin there.
    /// - **BCE only.** Without it the terminal erases with its own
    ///   default background and the pen is irrelevant.
    /// - **Non-default background only.** Back-color erase paints the
    ///   background and nothing else — the same rule the deliberate
    ///   scroll path applies through `Cursor::bce_blank` — so a pen that
    ///   only carries `fg` or attributes leaves no trace.
    fn lf_would_bleed_background(&self, best: &Winner) -> bool {
        let Winner::Relative { plan, .. } = best else {
            return false;
        };
        !self.fullscreen
            && matches!(plan.vertical.shape, VerticalShape::Lf { .. })
            && self.opts.contains(Optimizations::BCE)
            && self.cur.style().bg.is_some()
    }

    /// Enumerate the move candidates and return the cheapest.
    ///
    /// Pure: no bytes are materialised, so the caller can plan, change
    /// the pen, and plan again to keep the two in agreement.
    fn plan_move(&self, from: Position, to: Position, overwrite_line: Option<&[Cell]>) -> Winner {
        // Seed the candidate list with CUP in absolute mode so it
        // wins ties against any relative shape.
        let mut best: Option<Winner> = None;
        if !self.relative_cursor {
            let cup_cost = cost::cup_cost(to.y, to.x);
            best = Some(Winner::Cup { cost: cup_cost });
        }

        // Capability trial bits enumerated in the original order:
        // (none, BS, TABS, BOTH). Each trial competes against all
        // three prefixes. The cross product is small (4 × 3 = 12 in
        // the worst case) and entirely cost-only — no bytes
        // materialised until the winner is selected.
        let trials_mask: u8 = ((self.opts.contains(Optimizations::TABS) as u8) << 1)
            | (self.opts.contains(Optimizations::BS) as u8);

        let try_trial = |use_tabs: bool, use_backspace: bool, best: &mut Option<Winner>| {
            for prefix in [PrefixShape::None, PrefixShape::Cr, PrefixShape::Home] {
                // Skip Home in relative mode (we never seed it
                // there; the original planner only tried it for
                // absolute moves).
                if matches!(prefix, PrefixShape::Home) && self.relative_cursor {
                    continue;
                }
                let cand_from = prefix.apply_to(from);
                let plan = self.relative_cursor_plan(
                    cand_from,
                    to,
                    overwrite_line,
                    use_tabs,
                    use_backspace,
                );
                let total = prefix.cost() + plan.cost;
                let improves = match best {
                    None => true,
                    Some(w) => total < w.cost(),
                };
                if improves {
                    *best = Some(Winner::Relative {
                        prefix,
                        plan,
                        cost: total,
                    });
                }
            }
        };

        // Iterate (none, BS, TABS, BOTH) in that order to preserve
        // historical tie-break order.
        for i in 0u8..=trials_mask {
            if i & !trials_mask != 0 {
                continue;
            }
            let use_tabs = i & 0b10 != 0;
            let use_backspace = i & 0b01 != 0;
            try_trial(use_tabs, use_backspace, &mut best);
        }

        best.expect("planner enumerates at least one candidate")
    }
}

enum Winner {
    Cup {
        cost: usize,
    },
    Relative {
        prefix: PrefixShape,
        plan: RelativePlan,
        cost: usize,
    },
}

impl Winner {
    fn cost(&self) -> usize {
        match self {
            Winner::Cup { cost } | Winner::Relative { cost, .. } => *cost,
        }
    }
}
