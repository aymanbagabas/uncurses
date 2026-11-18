//! Pen state transitions: SGR and OSC 8 deltas for the renderer's
//! tracked cell style.

use std::io;

use crate::cell::Cell;
use crate::renderer::Renderer;
use crate::style::Style;
use crate::style::diff::write_style_diff;

impl Renderer {
    /// Update the pen with minimal escape sequences. `None` resets to
    /// the default style (emits SGR reset and/or hyperlink terminator
    /// only when the current pen actually has something set).
    pub(crate) fn update_pen(&mut self, out: &mut Vec<u8>, cell: Option<&Cell>) -> io::Result<()> {
        // Raw-equality fast path: most cells in a styled run share the
        // exact same style (including any open link) as the last cell
        // that updated the pen, so a direct comparison (no clones,
        // no color-profile conversion) short-circuits the whole
        // function.
        if cell.map_or_else(
            || self.cur.style().is_empty(),
            |cell| {
                let style = self.cur.style();
                cell.fg == style.fg
                    && cell.bg == style.bg
                    && cell.underline_color == style.underline_color
                    && cell.attrs == style.attrs
                    && cell.underline == style.underline
                    && cell.link == style.link
            },
        ) {
            return Ok(());
        }

        // Extract only after the comparison, so unchanged runs do not clone
        // the hyperlink. Keep the raw style for the tracked pen.
        let target_style = cell.map_or_else(Style::default, Cell::style);
        let to = self.color_profile.convert_style(&target_style);
        let from = self.color_profile.convert_style(self.cur.style());

        // `write_style_diff` emits both the SGR delta and the OSC 8 hyperlink
        // delta. Both sides come from `convert_style`, which drops the link
        // entirely under `Profile::Disabled`, so OSC 8 is auto-suppressed
        // there with no special-case code here.
        write_style_diff(out, &from, &to)?;

        self.cur.set_style(target_style);
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
