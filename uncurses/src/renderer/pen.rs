//! CellStyle state transitions: SGR and OSC 8 deltas for the renderer's
//! tracked cell style.

use crate::renderer::packed::arena::Arena as _;
use std::io;

use crate::renderer::Renderer;
use crate::cell::Cell;
use crate::style::Style;
use crate::style::diff::write_style_diff;

impl Renderer {
    /// Update the pen with minimal escape sequences. `None` resets to
    /// the default style (emits SGR reset and/or hyperlink terminator
    /// only when the current pen actually has something set).
    pub(crate) fn update_pen(&mut self, out: &mut Vec<u8>, cell: Option<&Cell>) -> io::Result<()> {
        // SGR and OSC 8 are independent terminal state machines, so the
        // style and the hyperlink are transitioned separately: a run can
        // change style without breaking an open link, and vice versa.
        let target_style = match cell {
            Some(c) => c.style.style,
            None => Style::default(),
        };

        if &target_style != self.cur.style() {

            let to = self.color_profile.convert_style(&target_style);
            let from = self.color_profile.convert_style(self.cur.style());
            write_style_diff(out, &from, &to)?;
            self.cur.set_style_with_id(target_style, target_id);
        }

        self.update_link(out, cell)?;
        Ok(())
    }

    /// Emit the OSC 8 delta for `cell`, closing any open hyperlink when the
    /// target cell has none.
    ///
    /// Hyperlinks are suppressed entirely when the color profile disables
    /// styling, matching the SGR path.
    fn update_link(&mut self, out: &mut Vec<u8>, cell: Option<&Cell>) -> io::Result<()> {
        let target = match cell {
            Some(c) if self.color_profile.profile() != crate::color::Profile::Disabled => c.style.link.clone(),
            _ => None,
        };
        if target == self.cur.link_id() {
            return Ok(());
        }
        // An empty URL is the OSC 8 close, so the terminator covers both
        // "no link" and an explicitly cleared one.
        let link = crate::renderer::packed::arena::GLOBAL.link(target);
        if link.is_empty() {
            out.extend_from_slice(crate::ansi::hyperlink::HYPERLINK_RESET);
        } else {
            crate::ansi::hyperlink::write_hyperlink(out, &link.url, &link.params)?;
        }
        self.cur.set_link_id(target);
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

#[cfg(test)]
mod tests {
    use crate::renderer::Renderer;
    use crate::cell::Cell;

    const URL: &str = "https://example.com";
    const OPEN: &[u8] = b"\x1b]8;;https://example.com\x1b\\";
    const CLOSE: &[u8] = b"\x1b]8;;\x1b\\";

    #[test]
    fn opens_and_closes_a_hyperlink() {
        let mut r = Renderer::new();
        let mut out = Vec::new();
        r.update_pen(&mut out, Some(&Cell::narrow('a').with_link(URL, "")))
            .unwrap();
        assert_eq!(out, OPEN);

        out.clear();
        r.update_pen(&mut out, Some(&Cell::narrow('b'))).unwrap();
        assert_eq!(out, CLOSE);
    }

    #[test]
    fn a_link_survives_an_sgr_change() {
        // OSC 8 and SGR are independent state machines: restyling a cell
        // inside a link span must not close the link.
        let mut r = Renderer::new();
        let mut out = Vec::new();
        r.update_pen(&mut out, Some(&Cell::narrow('a').with_link(URL, "")))
            .unwrap();

        out.clear();
        let bold = Cell::narrow('b')
            .with_style(crate::style::Style::default().bold())
            .with_link(URL, "");
        r.update_pen(&mut out, Some(&bold)).unwrap();
        assert_eq!(out, b"\x1b[1m");
    }

    #[test]
    fn an_sgr_run_survives_a_link_change() {
        let mut r = Renderer::new();
        let mut out = Vec::new();
        let bold = crate::style::Style::default().bold();
        r.update_pen(&mut out, Some(&Cell::narrow('a').with_style(bold)))
            .unwrap();

        // Same style, link added: only OSC 8 should move.
        out.clear();
        r.update_pen(
            &mut out,
            Some(&Cell::narrow('b').with_style(bold).with_link(URL, "")),
        )
        .unwrap();
        assert_eq!(out, OPEN);
    }

    #[test]
    fn an_unchanged_link_writes_nothing() {
        let mut r = Renderer::new();
        let mut out = Vec::new();
        let cell = Cell::narrow('a').with_link(URL, "");
        r.update_pen(&mut out, Some(&cell)).unwrap();

        out.clear();
        r.update_pen(&mut out, Some(&cell)).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn reset_pen_closes_an_open_link() {
        let mut r = Renderer::new();
        let mut out = Vec::new();
        r.update_pen(&mut out, Some(&Cell::narrow('a').with_link(URL, "")))
            .unwrap();

        out.clear();
        r.reset_pen(&mut out).unwrap();
        assert_eq!(out, CLOSE);
    }
}
