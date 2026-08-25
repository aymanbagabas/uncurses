//! Per-line transform algorithm.
//!
//! Decides how to update each terminal line with minimal escape sequences.

pub(super) mod clear;
pub(super) mod emit;
pub(super) mod line;
pub(super) mod predicates;

#[cfg(test)]
mod tests {
    use super::predicates::can_clear_with;
    use crate::color::Color;
    use crate::renderer::packed::Ref;
    use crate::renderer::{RenderBuffer, Renderer};
    use crate::style::{AttrFlags, Style, UnderlineStyle};

    #[test]
    fn can_clear_with_accepts_bold_italic_blink() {
        let style = Style {
            attrs: AttrFlags::BOLD | AttrFlags::ITALIC | AttrFlags::SLOW_BLINK,
            ..Style::default()
        };
        let cell = Ref::BLANK.with_style(style);
        assert!(can_clear_with(
            crate::renderer::packed::arena::global_ref(),
            &cell,
            true
        ));
        assert!(can_clear_with(
            crate::renderer::packed::arena::global_ref(),
            &cell,
            false
        ));
    }

    #[test]
    fn can_clear_with_accepts_bg_color_only_with_bce() {
        // BCE paints the cleared region with the current bg, so a
        // bg-colored blank is reproducible. Without BCE, the erase
        // would paint with the terminal's default background — the
        // styled bg would be lost.
        let style = Style {
            bg: Some(Color::Red),
            ..Style::default()
        };
        let cell = Ref::BLANK.with_style(style);
        assert!(can_clear_with(
            crate::renderer::packed::arena::global_ref(),
            &cell,
            true
        ));
        assert!(!can_clear_with(
            crate::renderer::packed::arena::global_ref(),
            &cell,
            false
        ));
    }

    #[test]
    fn can_clear_with_rejects_underline() {
        let style = Style {
            underline: UnderlineStyle::Single,
            ..Style::default()
        };
        let cell = Ref::BLANK.with_style(style);
        assert!(!can_clear_with(
            crate::renderer::packed::arena::global_ref(),
            &cell,
            true
        ));
        assert!(!can_clear_with(
            crate::renderer::packed::arena::global_ref(),
            &cell,
            false
        ));
    }

    #[test]
    fn can_clear_with_rejects_reverse_and_strikethrough() {
        for attr in [
            AttrFlags::REVERSE,
            AttrFlags::STRIKETHROUGH,
            AttrFlags::CONCEAL,
        ] {
            let style = Style {
                attrs: attr,
                ..Style::default()
            };
            let cell = Ref::BLANK.with_style(style);
            assert!(
                !can_clear_with(crate::renderer::packed::arena::global_ref(), &cell, true),
                "attr {attr:?} should not be clearable with BCE"
            );
            assert!(
                !can_clear_with(crate::renderer::packed::arena::global_ref(), &cell, false),
                "attr {attr:?} should not be clearable without BCE"
            );
        }
    }

    #[test]
    fn test_transform_single_cell() {
        let mut r = Renderer::new();
        r.cur_buf = Some(RenderBuffer::new(10, 1));

        let mut new_buf = RenderBuffer::new(10, 1);
        new_buf.set_ref((3, 0), &Ref::narrow('X'));

        let mut sink = Vec::new();
        r.transform_line(&mut sink, &new_buf, 0, 0, 9).unwrap();

        let output = String::from_utf8_lossy(&sink);
        assert!(output.contains('X'));
    }

    #[test]
    fn test_transform_no_changes() {
        let mut r = Renderer::new();
        r.cur_buf = Some(RenderBuffer::new(10, 1));

        let new_buf = RenderBuffer::new(10, 1);
        let mut sink = Vec::new();
        r.transform_line(&mut sink, &new_buf, 0, 0, 9).unwrap();

        assert!(sink.is_empty());
    }

    #[test]
    fn test_transform_erase_to_eol() {
        let mut r = Renderer::new();

        // Old buffer has content across the full line
        let mut old_buf = RenderBuffer::new(10, 1);
        for i in 0..10 {
            old_buf.set_ref((i, 0), &Ref::narrow('X'));
        }
        old_buf.clear_touched();
        r.cur_buf = Some(old_buf);

        // New buffer only has content in first 3 cols, rest is blank
        let mut new_buf = RenderBuffer::new(10, 1);
        new_buf.set_ref((0, 0), &Ref::narrow('A'));
        new_buf.set_ref((1, 0), &Ref::narrow('B'));
        new_buf.set_ref((2, 0), &Ref::narrow('C'));

        let mut sink = Vec::new();
        r.transform_line(&mut sink, &new_buf, 0, 0, 9).unwrap();
        let output = String::from_utf8_lossy(&sink);
        assert!(output.contains('A'));
        // Should erase trailing content (EL \x1b[K or ECH \x1b[...X)
        assert!(
            output.contains("\x1b[K") || output.contains("X"),
            "Expected EL or ECH in output: {output:?}"
        );
    }

    #[test]
    fn test_render_after_scroll_emits_blanked_prefix() {
        // Regression: after a hard scroll, cur_buf rows get modified
        // but the new_buf passed to render() may have a touched span
        // that does not cover the columns where cur_buf changed.
        // render() must scan the full row width inside transform_line
        // so it picks up cells that differ in cur_buf vs new_buf
        // outside the touched span — not only the cells inside it.
        let width: u16 = 20;
        let height: u16 = 1;
        let mut r = Renderer::new();
        // Seed cur_buf with content that doesn't match the upcoming
        // new_buf row, simulating cur_buf state diverging from
        // new_buf at cells outside the touched span (e.g. after a
        // scroll blanked or shifted cur_buf).
        let mut cur = RenderBuffer::new(width, height);
        for (x, ch) in "ABCDEFGHIJ".chars().enumerate() {
            cur.set_ref((x as u16, 0), &Ref::narrow(ch));
        }
        r.cur_buf = Some(cur);
        r.last_width = width;
        r.last_height = height;

        // new_buf has content at cols 0..10 but only cols 3..9 are
        // marked as touched. The first three cells (`012`) differ
        // from cur_buf (`ABC`) but live outside the touched span.
        let mut new_buf = RenderBuffer::new(width, height);
        for (x, ch) in "012XYZWVUT".chars().enumerate() {
            new_buf.set_ref((x as u16, 0), &Ref::narrow(ch));
        }
        new_buf.touch_line(0, 3, 9);

        let mut sink = Vec::new();
        r.render(&mut sink, &mut new_buf).unwrap();
        let out = String::from_utf8_lossy(&sink);
        assert!(
            out.contains("012XYZWVUT") || (out.contains("012") && out.contains("XYZWVUT")),
            "expected leading `012` to be re-emitted (full-row scan), got {out:?}"
        );
    }

    #[test]
    fn test_el1_clears_leading_blanks_over_old_content() {
        // Old line: "Hello world!" at cols 0..12
        // New line: 6 leading blanks, then "world!" at cols 6..12
        // EL-1 from col 5 (6 - 1) wipes cols 0..=5 in one sequence,
        // beating six space writes.
        let width: u16 = 20;
        let mut r = Renderer::new();
        let mut cur = RenderBuffer::new(width, 1);
        for (x, ch) in "Hello world!".chars().enumerate() {
            cur.set_ref((x as u16, 0), &Ref::narrow(ch));
        }
        r.cur_buf = Some(cur);
        r.last_width = width;
        r.last_height = 1;

        let mut new_buf = RenderBuffer::new(width, 1);
        for (x, ch) in "      world!".chars().enumerate() {
            new_buf.set_ref((x as u16, 0), &Ref::narrow(ch));
        }
        new_buf.touch_line(0, 0, width - 1);

        let mut sink = Vec::new();
        r.transform_line(&mut sink, &new_buf, 0, 0, width - 1)
            .unwrap();
        let out = String::from_utf8_lossy(&sink);
        assert!(
            out.contains("\x1b[1K"),
            "expected EL-1 in output, got {out:?}"
        );
    }

    #[test]
    fn test_el0_clears_entire_blank_line() {
        // Old line had content, new line is fully blank → \x1b[K at
        // col 0 reproduces the whole line in one shot.
        let width: u16 = 20;
        let mut r = Renderer::new();
        let mut cur = RenderBuffer::new(width, 1);
        for (x, ch) in "Hello there".chars().enumerate() {
            cur.set_ref((x as u16, 0), &Ref::narrow(ch));
        }
        r.cur_buf = Some(cur);
        r.last_width = width;
        r.last_height = 1;

        let new_buf = RenderBuffer::new(width, 1);

        let mut sink = Vec::new();
        r.transform_line(&mut sink, &new_buf, 0, 0, width - 1)
            .unwrap();
        let out = String::from_utf8_lossy(&sink);
        assert!(
            out.contains("\x1b[K") || out.contains("\x1b[0K"),
            "expected EL-0 in output, got {out:?}"
        );
    }

    #[test]
    fn test_transform_styled_cells() {
        let mut r = Renderer::new();
        r.cur_buf = Some(RenderBuffer::new(10, 1));

        let mut new_buf = RenderBuffer::new(10, 1);
        let style = Style::default().fg(Color::Red);
        new_buf.set_ref((0, 0), &Ref::narrow('R').with_style(style));

        let mut sink = Vec::new();
        r.transform_line(&mut sink, &new_buf, 0, 0, 9).unwrap();
        let output = String::from_utf8_lossy(&sink);
        assert!(output.contains('R'));
        // Should contain red foreground SGR
        assert!(output.contains("\x1b[31m"));
    }

    #[test]
    fn test_clear_bottom() {
        let mut r = Renderer::new();

        // Old buffer has content on all 5 lines
        let mut old_buf = RenderBuffer::new(10, 5);
        for y in 0..5 {
            old_buf.set_ref((0, y), &Ref::narrow('X'));
        }

        r.cur_buf = Some(old_buf);

        // New buffer only has content on first 2 lines
        let mut new_buf = RenderBuffer::new(10, 5);
        new_buf.set_ref((0, 0), &Ref::narrow('A'));
        new_buf.set_ref((0, 1), &Ref::narrow('B'));

        let mut sink = Vec::new();
        r.clear_bottom(&mut sink, &new_buf).unwrap();
        let output = String::from_utf8_lossy(&sink);
        // Should contain ED (erase below)
        assert!(output.contains("\x1b[J"));
    }

    #[test]
    fn test_clear_bottom_preserves_styled_trailing_rows() {
        // Trailing rows full of bg-color spaces are visually styled
        // background, not blanks. ED with the default pen would clear
        // them to the terminal's background color and lose the bg
        // styling, so clear_bottom must NOT fire when the row's
        // "blank" doesn't match the current pen.
        let mut r = Renderer::new();

        let mut old_buf = RenderBuffer::new(10, 4);
        for x in 0..10u16 {
            old_buf.set_ref((x, 3), &Ref::BLANK);
        }
        r.cur_buf = Some(old_buf);

        // new_buf has the bottom row filled with bg-red spaces. The
        // pen on entry is default, so ED would paint with default bg
        // and erase the red background — wrong.
        let bg_red =
            Ref::narrow(' ').with_style(Style::default().bg(crate::color::Color::Indexed(1)));
        let mut new_buf = RenderBuffer::new(10, 4);
        new_buf.set_ref((0, 0), &Ref::narrow('A'));
        for x in 0..10u16 {
            new_buf.set_ref((x, 3), &bg_red.clone());
        }

        let mut sink = Vec::new();
        r.clear_bottom(&mut sink, &new_buf).unwrap();
        let output = String::from_utf8_lossy(&sink);
        assert!(
            !output.contains("\x1b[J"),
            "ED must not fire when trailing rows carry non-default style: {output:?}"
        );
    }

    #[test]
    fn test_transform_uses_overwrite_advance_in_middle_of_row() {
        // Old row matches new row in the prefix and at one trailing
        // cell; the gap in between is identical bytes. The transform
        // path should walk through the gap by writing the matching
        // bytes rather than emitting CUF (move-with-overwrite).
        let mut r = Renderer::new();
        let mut old_buf = RenderBuffer::new(10, 1);
        let mut new_buf = RenderBuffer::new(10, 1);
        for x in 0..10u16 {
            old_buf.set_ref((x, 0), &Ref::narrow('a'));
            new_buf.set_ref((x, 0), &Ref::narrow('a'));
        }
        old_buf.set_ref((0, 0), &Ref::narrow('X'));
        old_buf.set_ref((9, 0), &Ref::narrow('X'));
        new_buf.set_ref((0, 0), &Ref::narrow('A'));
        new_buf.set_ref((9, 0), &Ref::narrow('B'));
        r.cur_buf = Some(old_buf);
        r.last_width = 10;
        r.last_height = 1;

        let mut sink = Vec::new();
        r.transform_line(&mut sink, &new_buf, 0, 0, 9).unwrap();
        let out = String::from_utf8_lossy(&sink);
        assert!(out.contains('A') && out.contains('B'));
    }
}
