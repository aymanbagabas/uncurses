//! Bail-out repaint for a row carrying a cluster whose column count the
//! terminal may not agree with.
//!
//! The diff decides what to emit from how many columns it believes each
//! cell takes. Under [`WidthMode::Wc`] a cluster is measured by summing its
//! code points, and a terminal that ligates the cluster draws one glyph and
//! advances over that alone. The belief is then wrong for that cluster and
//! for every column to its right, which is as far as one row reaches.
//!
//! So the row gives up on the diff there and repaints the rest of itself.
//! The tail is written in one pass from a single absolute move, which asks
//! nothing about where the terminal put the cluster, and lets the terminal
//! lay the rest of the row out on its own terms.

use std::io;

use super::emit::cluster_start;
use super::predicates::can_clear_with;
use crate::ansi;
use crate::cell::Cell;
use crate::renderer::caps::Optimizations;
use crate::renderer::{RenderBuffer, Renderer};
use crate::text::WidthMode;

impl Renderer {
    /// The column to repaint a row from, when it holds a cluster the
    /// terminal may measure differently than this does.
    ///
    /// Both rows are scanned, and from the left edge rather than from the
    /// first difference. A cluster decided where the terminal put every
    /// column to its right at the moment it was drawn, so those columns
    /// keep an unknown position for as long as it stands there, whether or
    /// not this frame is what changed them. A cluster the new row no longer
    /// carries counts for the same reason.
    ///
    /// A change confined to the columns left of the cluster is left to the
    /// ordinary diff. Those columns are measured the way the terminal draws
    /// them, so nothing about them is in doubt.
    ///
    /// [`Renderer::width_is_uncertain`] can only hold under
    /// [`WidthMode::Wc`], so any other policy skips the scan rather than
    /// walking the row to a foregone answer.
    pub(super) fn uncertain_bail(
        &self,
        new_line: &[Cell],
        old_line: Option<&[Cell]>,
    ) -> Option<usize> {
        if self.width_mode != WidthMode::Wc {
            return None;
        }
        let at = (0..new_line.len()).find(|&x| {
            self.width_is_uncertain(&new_line[x])
                || old_line
                    .and_then(|l| l.get(x))
                    .is_some_and(|c| self.width_is_uncertain(c))
        })?;

        // A row whose changes all fall left of the cluster has nothing to
        // repaint, and the rest of the row is left standing as it is.
        (at..new_line.len()).find(|&x| {
            old_line
                .and_then(|l| l.get(x))
                .is_none_or(|o| o != &new_line[x])
        })?;

        // A column the old row held a cluster at can be a continuation on
        // the new one, and emission has to start on a column that owns the
        // cell it holds.
        Some(cluster_start(new_line, at))
    }

    /// Repaint `new_line[from..]` on row `y`, in place of diffing it.
    ///
    /// The tail is erased and then written from one absolute move. A
    /// cluster the terminal draws narrower than the sum of its parts pulls
    /// the text after it left, and the erase is what keeps the old row's
    /// last columns from showing through past the end of the new one.
    ///
    /// The cursor is left on the left edge of the row. Every column the
    /// tail covers was placed by the terminal rather than by this, so the
    /// tracked column is a guess from `from` on, and a carriage return is
    /// the one horizontal move that replaces a guess with a fact.
    pub(super) fn repaint_tail(
        &mut self,
        out: &mut Vec<u8>,
        new_buf: &RenderBuffer,
        new_line: &[Cell],
        y: u16,
        from: usize,
    ) -> io::Result<()> {
        let width = new_line.len();
        let blank = &new_line[width - 1];
        let bce = self.opts.contains(Optimizations::BCE);

        self.move_to(out, new_buf, y, from as u16)?;

        // An erase hands the trailing blanks to the terminal, so emission
        // only has to reach the last cell that is not one. A row whose
        // right edge an erase cannot reproduce is written out in full.
        let mut last = width - 1;
        if can_clear_with(blank, bce) {
            self.update_pen(out, Some(blank))?;
            ansi::screen::write_erase_to_eol(out)?;
            while last > from && &new_line[last] == blank {
                last -= 1;
            }
            if &new_line[last] == blank {
                // The erase drew the whole tail. This is reachable because
                // `from` can name a column the old row held a cluster at
                // and the new row leaves blank.
                self.reanchor_to_row_start(out);
                return Ok(());
            }
        }

        // Whether the emission ended inside the interval does not matter
        // here, the way it does to a caller that goes on to position from
        // where it left off. The re-anchor below discards the column
        // either way.
        //
        // A terminal can also advance further than the claim, not only
        // less. A cluster carrying a variation selector draws in emoji
        // presentation, two columns for a code point summed here as one:
        // `❤️‍🔥` claims three columns and takes four. The extra column can
        // cross the right margin on a row this believes fits, and the wrap
        // would spill the row onto the next one, where nothing in the model
        // accounts for it and every later frame inherits it. Clipping at
        // the margin keeps the disagreement inside the row that caused it.
        ansi::mode::Mode::AUTO_WRAP.reset(out)?;
        self.emit_range(out, new_buf, new_line, from, last, true)?;
        ansi::mode::Mode::AUTO_WRAP.set(out)?;
        self.reanchor_to_row_start(out);
        Ok(())
    }
}
