//! Relative cursor move: a vertical leg followed by a horizontal leg.
//!
//! Both legs run cost-first via [`super::axis`]: each leg's shortest
//! shape is selected without touching any byte buffer, then the
//! chosen shapes are emitted in order. The vertical leg's post-fx
//! (the column after the vertical move, which may differ from `from.x`
//! when `\n` resets the column under ONLCR) feeds the horizontal
//! cost pass.

use std::io;

use crate::layout::Position;
use crate::renderer::Renderer;
use crate::cell::Cell;

impl Renderer {
    /// Plan the relative cursor move (no emit). Returns the combined
    /// cost and the two leg plans so the outer planner can pick a
    /// winner across multiple prefix candidates and emit only the
    /// chosen one.
    pub(super) fn relative_cursor_plan(
        &self,
        from: Position,
        to: Position,
        overwrite_line: Option<&[Cell]>,
        use_tabs: bool,
        use_backspace: bool,
    ) -> RelativePlan {
        let v = self.plan_vertical_cost(from.y, to.y, from.x);
        let h = self.plan_horizontal_cost(v.post_fx, to.x, overwrite_line, use_tabs, use_backspace);
        RelativePlan {
            cost: v.cost + h.cost,
            vertical: v,
            horizontal: h,
        }
    }

    /// Emit a previously-planned relative cursor move.
    pub(super) fn emit_relative_plan(
        &self,
        out: &mut Vec<u8>,
        plan: &RelativePlan,
        overwrite_line: Option<&[Cell]>,
    ) -> io::Result<()> {
        self.emit_vertical(out, plan.vertical.shape)?;
        self.emit_horizontal(out, plan.horizontal.shape, overwrite_line)
    }

    /// Plan-and-emit a relative cursor move. Test-only convenience
    /// wrapper for callers that don't need to compose the relative
    /// plan with a prefix candidate.
    #[cfg(test)]
    pub(crate) fn relative_cursor_move(
        &self,
        out: &mut Vec<u8>,
        from: Position,
        to: Position,
        overwrite_line: Option<&[Cell]>,
        use_tabs: bool,
        use_backspace: bool,
    ) -> io::Result<()> {
        let plan = self.relative_cursor_plan(from, to, overwrite_line, use_tabs, use_backspace);
        self.emit_relative_plan(out, &plan, overwrite_line)
    }
}

/// Combined two-leg plan returned by [`Renderer::relative_cursor_plan`].
pub(super) struct RelativePlan {
    pub(super) cost: usize,
    pub(super) vertical: super::axis::VerticalPlan,
    pub(super) horizontal: super::axis::HorizontalPlan,
}
