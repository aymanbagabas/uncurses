//! Pen state transitions: SGR and OSC 8 deltas for the renderer's
//! tracked cell style.

use std::io;

use crate::cell::Cell;
use crate::renderer::Renderer;
use crate::style::diff::write_style_diff;

impl Renderer {
    /// Update the pen with minimal escape sequences. `None` resets to
    /// the default style (emits SGR reset and/or hyperlink terminator
    /// only when the current pen actually has something set).
    /// Bring the terminal's pen in line with `cell`.
    ///
    /// SGR and OSC 8 are separate state machines on the wire: a run can
    /// change style without breaking an open link, and a link can open or
    /// close without disturbing the style. They transition independently.
    pub(crate) fn update_pen(&mut self, out: &mut Vec<u8>, cell: Option<&Cell>) -> io::Result<()> {
        self.update_sgr(out, cell)?;
        self.update_link(out, cell)
    }

    fn update_sgr(&mut self, out: &mut Vec<u8>, cell: Option<&Cell>) -> io::Result<()> {
        let target = cell.map(|c| c.style.style).unwrap_or_default();
        // Most cells in a run share the previous cell's style, and `Style`
        // is a small `Copy` value, so this is a register comparison.
        if target == *self.cur.style() {
            return Ok(());
        }
        let to = self.color_profile.convert_style(&target);
        let from = self.color_profile.convert_style(self.cur.style());
        write_style_diff(out, &from, &to)?;
        self.cur.set_style(target);
        Ok(())
    }

    fn update_link(&mut self, out: &mut Vec<u8>, cell: Option<&Cell>) -> io::Result<()> {
        // A disabled profile drops hyperlinks the same way it drops colour.
        let target = match cell {
            Some(c) if self.color_profile.profile() != crate::color::Profile::Disabled => {
                c.style.link.clone()
            }
            _ => None,
        };
        let same = match (&target, self.cur.link()) {
            (None, None) => true,
            (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b) || a == b,
            _ => false,
        };
        if same {
            return Ok(());
        }
        match &target {
            Some(l) => crate::ansi::hyperlink::write_hyperlink(out, &l.url, &l.params)?,
            None => out.extend_from_slice(crate::ansi::hyperlink::HYPERLINK_RESET),
        }
        self.cur.set_link(target);
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
