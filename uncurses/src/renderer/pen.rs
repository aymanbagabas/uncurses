//! Pen state transitions: SGR and OSC 8 deltas for the renderer's
//! tracked cell style and hyperlink.

use std::io;

use crate::ansi;
use crate::cell::{Cell, Link};
use crate::renderer::Renderer;
use crate::style::Style;
use crate::style::diff::write_style_diff;

impl Renderer {
    /// Update the pen (style + link) with minimal escape sequences.
    /// `None` resets to default style/link (emits SGR reset and/or
    /// hyperlink terminator only when the current pen actually has
    /// something set).
    pub(crate) fn update_pen(&mut self, out: &mut Vec<u8>, cell: Option<&Cell>) -> io::Result<()> {
        let (style, link) = match cell {
            Some(c) => (c.style(), c.link()),
            None => (&Style::EMPTY, &Link::EMPTY),
        };

        // Raw-equality fast path: most cells in a styled run share the
        // exact same (style, link) as the last cell that updated the
        // pen, so a single PartialEq check (no clones, no color-profile
        // conversion) short-circuits the whole function.
        if *style == *self.cur.style() && link == self.cur.link() {
            return Ok(());
        }

        let target_style = self.color_profile.convert_style(style);
        let current_style = self.color_profile.convert_style(self.cur.style());

        write_style_diff(out, &current_style, &target_style)?;
        self.cur.set_style(*style);

        // Hyperlink diff. Both sides are routed through the color
        // profile so a `Disabled` profile collapses OSC 8 to nothing.
        // Compare url/params by reference first to skip the convert
        // calls whenever the diff lines up; only clone into self.cur
        // when we actually emit a hyperlink change.
        let cur_link = self.cur.link();
        if link.url != cur_link.url || link.params != cur_link.params {
            let target_link = self.color_profile.convert_link(link);
            let current_link = self.color_profile.convert_link(cur_link);
            if current_link != target_link {
                if target_link.is_empty() {
                    ansi::write_hyperlink_end(out)?;
                } else {
                    ansi::write_hyperlink_start(out, &target_link.url, &target_link.params)?;
                }
                self.cur.set_link(target_link.clone());
            }
        }

        Ok(())
    }

    /// Reset pen to default style, emitting SGR reset and any pending
    /// hyperlink terminator. Delegates to [`Renderer::update_pen`] with
    /// `None` so both reset paths agree on what bytes are emitted and
    /// neither leaks a half-open hyperlink to the terminal.
    pub(crate) fn reset_pen(&mut self, out: &mut Vec<u8>) -> io::Result<()> {
        self.update_pen(out, None)
    }
}
