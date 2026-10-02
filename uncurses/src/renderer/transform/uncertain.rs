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
    /// Both rows count, including their equal prefix. A cluster decided
    /// where the terminal put every
    /// column to its right at the moment it was drawn, so those columns
    /// keep an unknown position for as long as it stands there, whether or
    /// not this frame is what changed them. A cluster the new row no longer
    /// carries counts for the same reason.
    ///
    /// A change confined to the columns left of the cluster is left to the
    /// ordinary diff. Those columns are measured the way the terminal draws
    /// them, so nothing about them is in doubt.
    ///
    /// `first_diff` names the first differing column. The caller compares
    /// the full rows and skips identical rows before this check.
    pub(super) fn uncertain_bail(
        &self,
        new_line: &[Cell],
        old_line: Option<&[Cell]>,
        first_diff: usize,
    ) -> Option<usize> {
        let new_at = self.uncertain_from(new_line);
        // The equal prefix has the same uncertainty on both rows. Only an
        // earlier cluster in the old row's changed tail can move the boundary.
        let old_end = new_at.unwrap_or(new_line.len());
        let old_at = old_line
            .and_then(|l| l.get(first_diff..l.len().min(old_end)))
            .and_then(|l| self.uncertain_from(l))
            .map(|at| first_diff + at);
        let at = old_at.or(new_at)?;

        // A row whose changes all fall left of the cluster has nothing to
        // repaint, and the rest of the row is left standing as it is.
        if at > first_diff {
            (at..new_line.len()).find(|&x| {
                old_line
                    .and_then(|l| l.get(x))
                    .is_none_or(|o| o != &new_line[x])
            })?;
        }

        // A column the old row held a cluster at can be a continuation on
        // the new one, and emission has to start on a column that owns the
        // cell it holds.
        Some(cluster_start(new_line, at))
    }

    /// The column of the first cluster on `line` the terminal may measure
    /// differently than this does, if the row carries one.
    ///
    /// Every column from here to the row's right edge keeps an unknown
    /// position for as long as that cluster stands, whichever frame drew
    /// it.
    ///
    /// [`Renderer::width_is_uncertain`] can only hold under
    /// [`WidthMode::Wc`], so any other policy skips the scan rather than
    /// walking the row to a foregone answer.
    pub(super) fn uncertain_from(&self, line: &[Cell]) -> Option<usize> {
        if self.width_mode != WidthMode::Wc {
            return None;
        }
        (0..line.len()).find(|&x| self.width_is_uncertain(&line[x]))
    }

    /// Move to row `y`, column `x`, to leave the cursor resting there
    /// once the frame's cells are drawn.
    ///
    /// A row carrying a cluster the terminal may measure differently has
    /// no column past that cluster this can name: the terminal counts
    /// from where it drew the glyph, and this counts from what the
    /// cluster's parts sum to. Naming one anyway misplaces the cursor,
    /// and the planner is free to pay for a short forward move by
    /// re-emitting the cells it passes over, which would paint those
    /// glyphs into columns the row never meant them for.
    ///
    /// So the cursor walks there instead. It is placed on the cluster,
    /// which is a column every terminal agrees on, and then the cells
    /// between the cluster and `x` are written. Writing is the one move
    /// that needs no column: each cell leaves the cursor wherever the
    /// terminal itself decided to put the next one. The cells come from
    /// the frame just drawn, so the row reads exactly as it did before.
    ///
    /// A target at or left of the cluster is reached the ordinary way.
    /// Those columns are measured the way the terminal draws them.
    pub(crate) fn move_to_resting(
        &mut self,
        out: &mut Vec<u8>,
        buf: &RenderBuffer,
        y: u16,
        x: u16,
    ) -> io::Result<()> {
        if self.walk_to_uncertain(out, buf, y, x)? {
            Ok(())
        } else {
            self.move_to(out, buf, y, x)
        }
    }

    /// Walk to a normalized target past an uncertain cluster in the
    /// displayed row. Return false when ordinary cursor movement suffices.
    pub(crate) fn walk_to_uncertain(
        &mut self,
        out: &mut Vec<u8>,
        buf: &RenderBuffer,
        y: u16,
        x: u16,
    ) -> io::Result<bool> {
        let Some(from) = buf
            .line(y)
            .and_then(|l| self.uncertain_from(&l[..l.len().min(x as usize)]))
        else {
            return Ok(false);
        };
        let line = buf.line(y).expect("row scanned above");

        self.move_to(out, buf, y, from as u16)?;

        // A target inside a cluster is reached by stopping on the column
        // that owns it, the one place in it the cursor can rest.
        let stop = cluster_start(line, x as usize);
        if stop > from {
            // A cluster the terminal draws wider than its parts sum to can
            // carry the walk across the right margin, and the wrap would
            // spill onto a row nothing in the model accounts for.
            ansi::mode::Mode::AUTO_WRAP.reset(out)?;
            if self.emit_range(out, buf, line, from, stop - 1, true)? {
                // ECH clears the trailing blanks but leaves the cursor
                // before them. Cross their length without naming a column.
                ansi::cursor::write_cuf(out, stop as u16 - self.cur.pos().x)?;
            }
            ansi::mode::Mode::AUTO_WRAP.set(out)?;

            // The frame epilogue already returned the pen to default, and
            // these cells carry their own style and links past it. Close
            // them here, or the style of whatever the walk happened to
            // end on rides out with the frame and paints what follows.
            self.reset_pen(out)?;
        }

        // The walk ended where the terminal put the last cell, which is a
        // column only the terminal knows. Dropping the tracked column has
        // the next frame address this row absolutely, and the only column
        // it addresses here is the cluster or one left of it, where the
        // two still agree. [`Renderer::cursor_known`] reports false until
        // then, which is what it already means: a position the renderer
        // placed but cannot name.
        self.cur.x = None;
        Ok(true)
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
