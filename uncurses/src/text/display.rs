//! Serialize a [`Surface`] back into escape sequences and text.
//!
//! This is the inverse of [`Painter`](super::Painter). Where a painter parses
//! a string of text and escape sequences *onto* a surface, [`Encode`] walks a
//! surface row by row and emits its styled cells *back out* as a stream of SGR
//! and OSC 8 sequences interleaved with grapheme-cluster content.
//!
//! Each terminal row becomes one output line, rows are separated by CRLF
//! (`\r\n`) so the result reproduces the surface when written to a terminal,
//! and every row begins and ends in the default style. Style transitions
//! between adjacent cells are minimized the same way the renderer minimizes
//! them, so a run of identically styled cells emits a single opener.
//!
//! ```
//! use uncurses::buffer::{Buffer, SurfaceMut};
//! use uncurses::cell::Cell;
//! use uncurses::style::Style;
//! use uncurses::text::Encode;
//!
//! let mut buf = Buffer::new(2, 1);
//! buf.set_cell((0, 0).into(), &Cell::narrow("h").style(Style::new().bold()));
//! buf.set_cell((1, 0).into(), &Cell::narrow("i").style(Style::new().bold()));
//!
//! // Render into a String via the Display adapter.
//! let s = buf.display().to_string();
//! assert!(s.contains("hi"));
//! ```

use std::fmt;
use std::io::{self, Write};

use crate::buffer::Surface;
use crate::color::Profile;
use crate::layout::Position;
use crate::style::Style;
use crate::style::diff::{convert_style, write_style_diff};

/// Serialize a [`Surface`] into escape sequences and text.
///
/// This extension trait is implemented for every [`Surface`], so any cell
/// grid (a [`Buffer`](crate::buffer::Buffer),
/// [`Window`](crate::buffer::Window), [`Canvas`](crate::canvas::Canvas), or
/// [`Screen`](crate::screen::Screen)) can be rendered back to its escape-code
/// form.
///
/// Use [`encode`](Self::encode) to stream straight into an [`io::Write`], or
/// [`display`](Self::display) to borrow the surface as a
/// [`Display`](fmt::Display) for `format!`, `to_string`, and the `write!`
/// macros. The [`encode_with`](Self::encode_with) and
/// [`display_with`](Self::display_with) variants downsample every cell's
/// colors to a [`Profile`] first, the same way the renderer adapts output to a
/// terminal's color capability.
pub trait Encode: Surface {
    /// Write the surface to `w` as escape sequences and text.
    ///
    /// One terminal row is written per line, separated by CRLF (`\r\n`) with
    /// no trailing newline after the final row. Each row starts and ends in
    /// the default style: any open SGR state or hyperlink is reset at the end
    /// of the row.
    ///
    /// Trailing unstyled blank cells are trimmed from each row, so a row with
    /// nothing but default spaces emits an empty line. A blank cell still
    /// counts as visible when it carries a style (for example a space with a
    /// background color), so styled trailing space is preserved.
    ///
    /// Wide-cell continuation placeholders are skipped because the wide
    /// primary already carries the full grapheme cluster. Colors are written
    /// as stored; use [`encode_with`](Self::encode_with) to downsample them to
    /// a color [`Profile`].
    ///
    /// # Errors
    ///
    /// Propagates any [`io::Error`] from `w`.
    fn encode<W: Write>(&self, w: &mut W) -> io::Result<()> {
        encode_surface(self, w, Profile::TrueColor)
    }

    /// Write the surface to `w`, downsampling colors to `profile`.
    ///
    /// Each cell's style is passed through
    /// [`convert_style`](crate::style::convert_style) before it is emitted, so
    /// the output matches what the renderer would produce for a terminal with
    /// that color capability: [`Profile::Ansi`] and [`Profile::Ansi256`]
    /// quantize to the nearest palette color, [`Profile::Ascii`] drops colors
    /// but keeps attributes, and [`Profile::Disabled`] drops all styling
    /// (including hyperlinks). [`Profile::TrueColor`] is identical to
    /// [`encode`](Self::encode).
    ///
    /// # Errors
    ///
    /// Propagates any [`io::Error`] from `w`.
    fn encode_with<W: Write>(&self, w: &mut W, profile: Profile) -> io::Result<()> {
        encode_surface(self, w, profile)
    }

    /// Borrow the surface as a [`Display`](fmt::Display) adapter.
    ///
    /// The returned value renders the same bytes as [`encode`](Self::encode)
    /// when formatted, so `surface.display().to_string()` produces the encoded
    /// string and `write!(w, "{}", surface.display())` writes it to any
    /// [`io::Write`] or [`fmt::Write`] sink.
    fn display(&self) -> SurfaceDisplay<'_, Self> {
        SurfaceDisplay {
            surface: self,
            profile: Profile::TrueColor,
        }
    }

    /// Borrow the surface as a [`Display`](fmt::Display) adapter that
    /// downsamples colors to `profile`.
    ///
    /// Formatting it renders the same bytes as
    /// [`encode_with`](Self::encode_with) with the same profile.
    fn display_with(&self, profile: Profile) -> SurfaceDisplay<'_, Self> {
        SurfaceDisplay {
            surface: self,
            profile,
        }
    }
}

impl<S: Surface + ?Sized> Encode for S {}

/// A [`Display`](fmt::Display) adapter over a [`Surface`], returned by
/// [`Encode::display`] and [`Encode::display_with`].
///
/// Formatting it emits the same bytes as [`Encode::encode_with`] with the
/// adapter's profile.
pub struct SurfaceDisplay<'a, S: Surface + ?Sized> {
    surface: &'a S,
    profile: Profile,
}

impl<S: Surface + ?Sized> fmt::Display for SurfaceDisplay<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SGR/OSC sequences are ASCII and cell content is UTF-8, so the
        // encoded bytes are always valid UTF-8.
        let mut buf = Vec::new();
        encode_surface(self.surface, &mut buf, self.profile).map_err(|_| fmt::Error)?;
        let s = std::str::from_utf8(&buf).map_err(|_| fmt::Error)?;
        f.write_str(s)
    }
}

/// Encode `surface` into `w`, row by row, downsampling colors to `profile`.
fn encode_surface<S: Surface + ?Sized, W: Write>(
    surface: &S,
    w: &mut W,
    profile: Profile,
) -> io::Result<()> {
    let bounds = surface.bounds();
    let default = Style::default();
    let x_end = bounds.x.saturating_add(bounds.width);
    let y_end = bounds.y.saturating_add(bounds.height);

    for (row, y) in (bounds.y..y_end).enumerate() {
        if row > 0 {
            w.write_all(b"\r\n")?;
        }

        // Trim trailing unstyled blank space: find the last column that is
        // visible, i.e. has real content or a non-empty style after
        // downsampling. A blank cell whose style is empty (a styled space with
        // a background still counts as visible) contributes nothing once the
        // row resets to default, so everything past `last_visible` is dropped.
        let last_visible = (bounds.x..x_end).rev().find(|&x| {
            surface.cell(Position::new(x, y)).is_some_and(|cell| {
                !cell.is_blank() || !convert_style(&cell.style, profile).is_empty()
            })
        });
        let Some(last_visible) = last_visible else {
            // Entirely blank row: emit nothing between the separators.
            continue;
        };

        // The pen starts each row in the default style with no open link.
        let mut pen = default.clone();
        for x in bounds.x..=last_visible {
            let Some(cell) = surface.cell(Position::new(x, y)) else {
                continue;
            };
            // Wide continuations carry no content of their own; the primary
            // cell already emitted the whole grapheme cluster.
            if cell.is_continuation() {
                continue;
            }

            // Downsample to the target profile, then emit the SGR and OSC 8
            // hyperlink delta from the current pen.
            let to = convert_style(&cell.style, profile);
            write_style_diff(w, &pen, &to)?;
            pen = to;
            w.write_all(cell.content().as_bytes())?;
        }

        // Return the row to the default style, closing any open SGR state and
        // hyperlink so the next row starts clean.
        write_style_diff(w, &pen, &default)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Buffer, SurfaceMut};
    use crate::cell::Cell;
    use crate::style::{RESET, Style};

    fn reset() -> &'static str {
        std::str::from_utf8(RESET).unwrap()
    }

    #[test]
    fn default_surface_trims_to_empty_rows() {
        let buf = Buffer::new(3, 2);
        // Every cell is an unstyled blank space, so each row trims to nothing,
        // leaving two empty rows separated by CRLF.
        assert_eq!(buf.display().to_string(), "\r\n");
    }

    #[test]
    fn styled_run_emits_one_opener_and_a_trailing_reset() {
        let mut buf = Buffer::new(2, 1);
        let bold = Style::new().bold();
        buf.set_cell((0, 0).into(), &Cell::narrow("h").style(&bold));
        buf.set_cell((1, 0).into(), &Cell::narrow("i").style(&bold));

        let out = buf.display().to_string();
        // The run shares a style, so the opener appears once before the text
        // and a single reset closes the row.
        assert!(out.contains("hi"), "content present: {out:?}");
        assert!(out.ends_with(reset()), "row reset: {out:?}");
        assert_eq!(out.matches(reset()).count(), 1, "single reset: {out:?}");
        assert_eq!(out.matches("\x1b[").count(), 2, "one opener + reset: {out:?}");
    }

    #[test]
    fn wide_continuation_is_skipped() {
        let mut buf = Buffer::new(2, 1);
        buf.set_cell((0, 0).into(), &Cell::wide("世"));
        // (1,0) is the continuation placeholder written by set_cell.
        let out = buf.display().to_string();
        assert!(out.starts_with("世"), "wide grapheme emitted once: {out:?}");
        // No stray empty content or extra cell after the wide grapheme.
        assert_eq!(out, "世");
    }

    #[test]
    fn encode_matches_display() {
        let mut buf = Buffer::new(2, 1);
        buf.set_cell((0, 0).into(), &Cell::narrow("A").style(Style::new().bold()));
        let mut bytes = Vec::new();
        buf.encode(&mut bytes).unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), buf.display().to_string());
    }

    #[test]
    fn rows_are_crlf_separated_without_trailing_newline() {
        let buf = Buffer::new(1, 3);
        // Blank rows trim to empty, leaving just the CRLF separators.
        let out = buf.display().to_string();
        assert_eq!(out, "\r\n\r\n");
    }

    #[test]
    fn trailing_unstyled_spaces_are_trimmed() {
        let mut buf = Buffer::new(5, 1);
        buf.set_cell((0, 0).into(), &Cell::narrow("h"));
        buf.set_cell((1, 0).into(), &Cell::narrow("i"));
        // Columns 2..5 stay blank and unstyled, so they are trimmed.
        assert_eq!(buf.display().to_string(), "hi");
    }

    #[test]
    fn interior_blanks_are_kept_only_trailing_trimmed() {
        let mut buf = Buffer::new(5, 1);
        buf.set_cell((0, 0).into(), &Cell::narrow("a"));
        buf.set_cell((2, 0).into(), &Cell::narrow("b"));
        // The blank at column 1 is positional and kept; columns 3..5 trim.
        assert_eq!(buf.display().to_string(), "a b");
    }

    #[test]
    fn styled_trailing_space_is_not_trimmed() {
        use crate::color::{BasicColor, Color};
        let mut buf = Buffer::new(3, 1);
        buf.set_cell((0, 0).into(), &Cell::narrow("a"));
        // A trailing space with a background is visible, so it survives.
        let bg = Style::new().bg(Color::Basic(BasicColor::Red));
        buf.set_cell((2, 0).into(), &Cell::narrow(" ").style(&bg));
        // "a", a positional blank, then the bg-styled space, then reset.
        assert_eq!(buf.display().to_string(), "a \x1b[41m \x1b[m");
    }

    #[test]
    fn disabled_profile_trims_styled_trailing_space() {
        use crate::color::{BasicColor, Color, Profile};
        let mut buf = Buffer::new(3, 1);
        buf.set_cell((0, 0).into(), &Cell::narrow("a"));
        let bg = Style::new().bg(Color::Basic(BasicColor::Red));
        buf.set_cell((2, 0).into(), &Cell::narrow(" ").style(&bg));
        // Under Disabled the background is dropped, so the trailing space is
        // unstyled and gets trimmed along with the interior blank.
        assert_eq!(buf.display_with(Profile::Disabled).to_string(), "a");
    }

    #[test]
    fn profile_disabled_drops_all_styling() {
        use crate::color::{BasicColor, Color, Profile};
        let mut buf = Buffer::new(2, 1);
        let styled = Style::new().bold().fg(Color::Basic(BasicColor::Red));
        buf.set_cell((0, 0).into(), &Cell::narrow("h").style(&styled));
        buf.set_cell((1, 0).into(), &Cell::narrow("i").style(&styled));
        // Disabled strips every escape: only the text remains.
        assert_eq!(buf.display_with(Profile::Disabled).to_string(), "hi");
    }

    #[test]
    fn profile_ansi_downsamples_truecolor_to_palette() {
        use crate::color::{Color, Profile};
        let mut buf = Buffer::new(1, 1);
        // A pure-red 24-bit color quantizes to the nearest palette entry,
        // xterm bright red (SGR 91), under Ansi.
        let red = Style::new().fg(Color::Rgb(255, 0, 0));
        buf.set_cell((0, 0).into(), &Cell::narrow("x").style(&red));
        let out = buf.display_with(Profile::Ansi).to_string();
        assert_eq!(out, "\x1b[91mx\x1b[m");
    }

    #[test]
    fn profile_ascii_keeps_attributes_but_drops_color() {
        use crate::color::{Color, Profile};
        let mut buf = Buffer::new(1, 1);
        let styled = Style::new().bold().fg(Color::Rgb(10, 20, 30));
        buf.set_cell((0, 0).into(), &Cell::narrow("x").style(&styled));
        // Bold (SGR 1) survives; the foreground color is dropped.
        assert_eq!(buf.display_with(Profile::Ascii).to_string(), "\x1b[1mx\x1b[m");
    }

    #[test]
    fn truecolor_profile_matches_default() {
        use crate::color::{Color, Profile};
        let mut buf = Buffer::new(2, 1);
        let styled = Style::new().fg(Color::Rgb(1, 2, 3)).link("https://e.x", "");
        buf.set_cell((0, 0).into(), &Cell::narrow("h").style(&styled));
        buf.set_cell((1, 0).into(), &Cell::narrow("i").style(&styled));
        assert_eq!(
            buf.display_with(Profile::TrueColor).to_string(),
            buf.display().to_string(),
        );
    }
}
