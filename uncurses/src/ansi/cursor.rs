//! Cursor addressing, movement, saving, reporting, and style sequences.
//!
//! ## Category
//!
//! This module emits cursor-related CSI, ESC, and OSC controls: absolute and
//! relative movement, line/index movement, cursor save/restore forms, cursor
//! style, pointer shape, and cursor-position reports.
//!
//! ## Coordinate conventions
//!
//! Public cursor-position arguments are zero-based unless the item explicitly
//! says otherwise. Writers add one when the terminal sequence is one-based and
//! omit default parameters when the emitted bytes support a shorter spelling.
//!
//! ```text
//! row=10 col=20  →  ESC [ 11 ; 21 H
//!                   ──┬── ─┬─  ┬─ ┬
//!                    CSI  row  col final
//! ```
//!
//! ## Mode interaction
//!
//! Origin mode ([`Mode::ORIGIN`](crate::ansi::mode::Mode::ORIGIN)) changes how
//! terminals interpret absolute row/column positions relative to scroll margins.
//! Cursor visibility is controlled by [`Mode::CURSOR_VISIBLE`](crate::ansi::mode::Mode::CURSOR_VISIBLE), while this module emits movement and report bytes.

use std::io::{self, Write};

/// Move the cursor to an absolute position with CUP, `ESC [ <row+1> ; <col+1> H`.
///
/// `row` and `col` are zero-based API coordinates. The writer omits the column when `col == 0` and emits bare `ESC [ H` for the origin.
pub fn write_cup<W: Write>(w: &mut W, row: u16, col: u16) -> io::Result<()> {
    if row == 0 && col == 0 {
        w.write_all(b"\x1b[H")
    } else if col == 0 {
        write!(w, "\x1b[{}H", row + 1)
    } else {
        write!(w, "\x1b[{};{}H", row + 1, col + 1)
    }
}

/// Move the cursor up `n` rows with CUU.
///
/// `n == 0` emits nothing, `n == 1` emits `ESC [ A`, and larger counts emit `ESC [ <n> A`.
pub fn write_cuu<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[A"),
        _ => write!(w, "\x1b[{n}A"),
    }
}

/// Move the cursor down `n` rows with CUD.
///
/// `n == 0` emits nothing, `n == 1` emits `ESC [ B`, and larger counts emit `ESC [ <n> B`.
pub fn write_cud<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[B"),
        _ => write!(w, "\x1b[{n}B"),
    }
}

/// Move the cursor forward/right `n` columns with CUF.
///
/// `n == 0` emits nothing, `n == 1` emits `ESC [ C`, and larger counts emit `ESC [ <n> C`.
pub fn write_cuf<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[C"),
        _ => write!(w, "\x1b[{n}C"),
    }
}

/// Move the cursor backward/left `n` columns with CUB.
///
/// `n == 0` emits nothing, `n == 1` emits `ESC [ D`, and larger counts emit `ESC [ <n> D`.
pub fn write_cub<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[D"),
        _ => write!(w, "\x1b[{n}D"),
    }
}

/// Move the cursor to an absolute column with CHA, `ESC [ <col+1> G`.
///
/// `col` is zero-based; `col == 0` emits the shorter `ESC [ G`.
pub fn write_cha<W: Write>(w: &mut W, col: u16) -> io::Result<()> {
    if col == 0 {
        w.write_all(b"\x1b[G")
    } else {
        write!(w, "\x1b[{}G", col + 1)
    }
}

/// Move the cursor to an absolute row with VPA, `ESC [ <row+1> d`.
///
/// `row` is zero-based; `row == 0` emits the shorter `ESC [ d`.
pub fn write_vpa<W: Write>(w: &mut W, row: u16) -> io::Result<()> {
    if row == 0 {
        w.write_all(b"\x1b[d")
    } else {
        write!(w, "\x1b[{}d", row + 1)
    }
}

/// Move to the first column of a following line with CNL.
///
/// `n == 0` emits nothing, `n == 1` emits `ESC [ E`, and larger counts emit `ESC [ <n> E`.
pub fn write_cnl<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[E"),
        _ => write!(w, "\x1b[{n}E"),
    }
}

/// Move to the first column of a preceding line with CPL.
///
/// `n == 0` emits nothing, `n == 1` emits `ESC [ F`, and larger counts emit `ESC [ <n> F`.
pub fn write_cpl<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[F"),
        _ => write!(w, "\x1b[{n}F"),
    }
}

/// Save the cursor with the DEC form `ESC 7` (DECSC).
///
/// This is distinct from [`write_save_cursor_position`], which emits the CSI `s` form.
pub fn write_save_cursor<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b7")
}

/// Restore the cursor with the DEC form `ESC 8` (DECRC).
///
/// This restores the state saved by [`write_save_cursor`] on terminals that support DECSC/DECRC.
pub fn write_restore_cursor<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b8")
}

/// Move up one line with Reverse Index, `ESC M`.
///
/// If the cursor is at the top margin, terminals scroll the region down. The single-byte 8-bit C1 equivalent is [`crate::ansi::c1::RI`].
pub fn write_reverse_index<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1bM")
}

/// Move down one line with Index, `ESC D`.
///
/// If the cursor is at the bottom margin, terminals scroll the region up. The single-byte 8-bit C1 equivalent is [`crate::ansi::c1::IND`].
pub fn write_index<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1bD")
}

/// Move the cursor with HVP, `ESC [ <row+1> ; <col+1> f`.
///
/// Arguments are zero-based API coordinates. The writer uses the same omission rules as [`write_cup`] but with final byte `f`.
pub fn write_hvp<W: Write>(w: &mut W, row: u16, col: u16) -> io::Result<()> {
    if row == 0 && col == 0 {
        w.write_all(b"\x1b[f")
    } else if col == 0 {
        write!(w, "\x1b[{}f", row + 1)
    } else {
        write!(w, "\x1b[{};{}f", row + 1, col + 1)
    }
}

/// Move to an absolute horizontal position with HPA, ``ESC [ <col+1> ` ``.
///
/// `col` is zero-based; `col == 0` emits the shorter ``ESC [ ` ``.
pub fn write_hpa<W: Write>(w: &mut W, col: u16) -> io::Result<()> {
    if col == 0 {
        w.write_all(b"\x1b[`")
    } else {
        write!(w, "\x1b[{}`", col + 1)
    }
}

/// Move horizontally right by `n` columns with HPR, `ESC [ <n> a`.
///
/// `n == 0` emits nothing and `n == 1` omits the parameter.
pub fn write_hpr<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[a"),
        _ => write!(w, "\x1b[{n}a"),
    }
}

/// Move vertically down by `n` rows with VPR, `ESC [ <n> e`.
///
/// `n == 0` emits nothing and `n == 1` omits the parameter.
pub fn write_vpr<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[e"),
        _ => write!(w, "\x1b[{n}e"),
    }
}

/// Advance `n` horizontal tab stops with CHT, `ESC [ <n> I`.
///
/// `n == 0` emits nothing and `n == 1` emits `ESC [ I`.
pub fn write_cht<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[I"),
        _ => write!(w, "\x1b[{n}I"),
    }
}

/// Save the cursor position with CSI `s`, exact bytes `ESC [ s`.
///
/// This is the alternate save form; [`write_save_cursor`] emits DEC `ESC 7`.
pub fn write_save_cursor_position<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b[s")
}

/// Restore the cursor position with CSI `u`, exact bytes `ESC [ u`.
///
/// This is the alternate restore form; [`write_restore_cursor`] emits DEC `ESC 8`.
pub fn write_restore_cursor_position<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b[u")
}

/// Set the pointer shape with OSC 22, `ESC ] 22 ; <shape> ESC \`.
///
/// `shape` is emitted verbatim. The sequence uses the `ST` terminator rather than BEL.
pub fn write_set_pointer_shape<W: Write>(w: &mut W, shape: &str) -> io::Result<()> {
    write!(w, "\x1b]22;{shape}\x1b\\")
}

/// Request an extended cursor-position report with `ESC [ ? 6 n` (DECXCPR).
///
/// Compatible terminals reply with a private cursor-position report such as `ESC [ ? <line> ; <column> R`.
pub fn write_request_extended_cursor_position<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b[?6n")
}

/// The visual shape of the text cursor, independent of whether it blinks.
///
/// DECSCUSR interleaves the two: each shape has a blinking and a steady
/// parameter. [`CursorStyle::new`] combines them, and
/// [`CursorStyle::shape`] / [`CursorStyle::blinking`] take them apart again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CursorShape {
    /// A full-cell block.
    #[default]
    Block,
    /// A horizontal underline at the bottom of the cell.
    Underline,
    /// A vertical bar at the left of the cell.
    Bar,
}

/// Cursor style values for DECSCUSR (`ESC [ Ps SP q`).
///
/// The variants map directly to the numeric `Ps` parameter used by
/// [`write_cursor_style`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CursorStyle {
    /// Terminal default, DECSCUSR parameter `0`.
    ///
    /// Writing this asks the terminal for the cursor its user configured.
    /// kitty, foot, rio, Alacritty, VTE, Konsole and Windows Terminal all
    /// read the shape back from configuration here, so it is the way to
    /// hand the cursor back at the end of a session. xterm documents `0` as
    /// a blinking block instead, which matches none of them.
    ///
    /// What it draws is therefore not knowable from the parameter: it is
    /// whatever that user chose, and terminals do not even agree on the
    /// blink, with kitty and Windows Terminal forcing it on where the
    /// others restore it from configuration too. So this names no shape and
    /// no blink state, and [`shape`](CursorStyle::shape) and
    /// [`blinking`](CursorStyle::blinking) answer `None` for it. Ask with
    /// [`Program::request_cursor_style`](crate::program::Program::request_cursor_style)
    /// when you need to know what the cursor actually is.
    #[default]
    Default,
    /// Blinking block cursor, DECSCUSR parameter `1`.
    BlinkingBlock,
    /// Steady block cursor, DECSCUSR parameter `2`.
    SteadyBlock,
    /// Blinking underline cursor, DECSCUSR parameter `3`.
    BlinkingUnderline,
    /// Steady underline cursor, DECSCUSR parameter `4`.
    SteadyUnderline,
    /// Blinking bar cursor, DECSCUSR parameter `5`.
    BlinkingBar,
    /// Steady bar cursor, DECSCUSR parameter `6`.
    SteadyBar,
}

impl CursorStyle {
    fn param(self) -> u8 {
        match self {
            CursorStyle::Default => 0,
            CursorStyle::BlinkingBlock => 1,
            CursorStyle::SteadyBlock => 2,
            CursorStyle::BlinkingUnderline => 3,
            CursorStyle::SteadyUnderline => 4,
            CursorStyle::BlinkingBar => 5,
            CursorStyle::SteadyBar => 6,
        }
    }

    /// The style a DECSCUSR parameter selects.
    ///
    /// # Returns
    ///
    /// `None` for a parameter outside `0..=6`, which selects no style this
    /// library knows. Used to read the style back out of a DECRPSS reply.
    pub fn from_param(n: u32) -> Option<Self> {
        Some(match n {
            0 => CursorStyle::Default,
            1 => CursorStyle::BlinkingBlock,
            2 => CursorStyle::SteadyBlock,
            3 => CursorStyle::BlinkingUnderline,
            4 => CursorStyle::SteadyUnderline,
            5 => CursorStyle::BlinkingBar,
            6 => CursorStyle::SteadyBar,
            _ => return None,
        })
    }

    /// The style that draws `shape`, blinking or steady.
    pub fn new(shape: CursorShape, blinking: bool) -> Self {
        match (shape, blinking) {
            (CursorShape::Block, true) => CursorStyle::BlinkingBlock,
            (CursorShape::Block, false) => CursorStyle::SteadyBlock,
            (CursorShape::Underline, true) => CursorStyle::BlinkingUnderline,
            (CursorShape::Underline, false) => CursorStyle::SteadyUnderline,
            (CursorShape::Bar, true) => CursorStyle::BlinkingBar,
            (CursorShape::Bar, false) => CursorStyle::SteadyBar,
        }
    }

    /// The shape this style draws.
    ///
    /// # Returns
    ///
    /// `None` for [`CursorStyle::Default`], which defers to the terminal
    /// rather than naming a shape.
    pub fn shape(self) -> Option<CursorShape> {
        Some(match self {
            CursorStyle::Default => return None,
            CursorStyle::BlinkingBlock | CursorStyle::SteadyBlock => CursorShape::Block,
            CursorStyle::BlinkingUnderline | CursorStyle::SteadyUnderline => CursorShape::Underline,
            CursorStyle::BlinkingBar | CursorStyle::SteadyBar => CursorShape::Bar,
        })
    }

    /// Whether this style blinks.
    ///
    /// # Returns
    ///
    /// `None` for [`CursorStyle::Default`], which defers to the terminal
    /// rather than choosing.
    pub fn blinking(self) -> Option<bool> {
        Some(match self {
            CursorStyle::Default => return None,
            CursorStyle::BlinkingBlock
            | CursorStyle::BlinkingUnderline
            | CursorStyle::BlinkingBar => true,
            CursorStyle::SteadyBlock | CursorStyle::SteadyUnderline | CursorStyle::SteadyBar => {
                false
            }
        })
    }
}

/// Set the cursor style with DECSCUSR, `ESC [ <style> SP q`.
///
/// The numeric parameter comes from [`CursorStyle`]: `0` for default, `1/2` block, `3/4` underline, and `5/6` bar.
pub fn write_cursor_style<W: Write>(w: &mut W, style: CursorStyle) -> io::Result<()> {
    write!(w, "\x1b[{} q", style.param())
}

/// Request a standard cursor-position report with `ESC [ 6 n` (DSR 6).
///
/// Compatible terminals reply with `ESC [ <line> ; <column> R`, using one-based coordinates.
pub fn write_request_cursor_position<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b[6n")
}

/// Write a literal horizontal tab byte, `HT` (`0x09`).
///
/// This is not a CSI sequence; the terminal advances according to its current tab stops.
pub fn write_tab<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\t")
}

/// Move backward by `n` tab stops with CBT, `ESC [ <n> Z`.
///
/// `n == 0` emits nothing, `n == 1` emits `ESC [ Z`, and larger counts include the decimal count.
pub fn write_backtab<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[Z"),
        _ => write!(w, "\x1b[{n}Z"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_style_round_trips_its_parameter() {
        // `from_param` reads back what DECSCUSR wrote, which is what makes a
        // DECRPSS reply decodable.
        for style in [
            CursorStyle::Default,
            CursorStyle::BlinkingBlock,
            CursorStyle::SteadyBlock,
            CursorStyle::BlinkingUnderline,
            CursorStyle::SteadyUnderline,
            CursorStyle::BlinkingBar,
            CursorStyle::SteadyBar,
        ] {
            assert_eq!(CursorStyle::from_param(style.param() as u32), Some(style));
        }
        assert_eq!(CursorStyle::from_param(7), None);
        assert_eq!(CursorStyle::from_param(u32::MAX), None);
    }

    #[test]
    fn shape_and_blinking_round_trip_through_a_style() {
        for shape in [CursorShape::Block, CursorShape::Underline, CursorShape::Bar] {
            for blinking in [true, false] {
                let style = CursorStyle::new(shape, blinking);
                assert_eq!(style.shape(), Some(shape));
                assert_eq!(style.blinking(), Some(blinking));
            }
        }
    }

    #[test]
    fn the_terminal_default_names_neither_shape_nor_blink() {
        // DECSCUSR 0 defers to the terminal, so there is nothing to report.
        assert_eq!(CursorStyle::Default.shape(), None);
        assert_eq!(CursorStyle::Default.blinking(), None);
    }

    #[test]
    fn test_cup_origin() {
        let mut buf = Vec::new();
        write_cup(&mut buf, 0, 0).unwrap();
        assert_eq!(buf, b"\x1b[H");
    }

    #[test]
    fn test_cup_row_only() {
        let mut buf = Vec::new();
        write_cup(&mut buf, 5, 0).unwrap();
        assert_eq!(buf, b"\x1b[6H");
    }

    #[test]
    fn test_cup_both() {
        let mut buf = Vec::new();
        write_cup(&mut buf, 10, 20).unwrap();
        assert_eq!(buf, b"\x1b[11;21H");
    }

    #[test]
    fn test_cuu_single() {
        let mut buf = Vec::new();
        write_cuu(&mut buf, 1).unwrap();
        assert_eq!(buf, b"\x1b[A");
    }

    #[test]
    fn test_cuu_multi() {
        let mut buf = Vec::new();
        write_cuu(&mut buf, 5).unwrap();
        assert_eq!(buf, b"\x1b[5A");
    }

    #[test]
    fn test_cursor_style() {
        let mut buf = Vec::new();
        write_cursor_style(&mut buf, CursorStyle::SteadyBar).unwrap();
        assert_eq!(buf, b"\x1b[6 q");
    }

    #[test]
    fn test_cup_cost() {
        use crate::ansi::cost::cup_cost;
        assert_eq!(cup_cost(0, 0), 3); // \x1b[H
        assert_eq!(cup_cost(9, 0), 5); // \x1b[10H
        assert_eq!(cup_cost(9, 9), 8); // \x1b[10;10H
    }

    // --- DECSCUSR encoding tests ---
    //
    // `CursorStyle` merges shape+blink into a single enum; each variant
    // maps to a stable DECSCUSR parameter emitted by
    // `write_cursor_style`. The mapping is exercised by writing each
    // variant and asserting the emitted byte stream matches
    // `CSI <param> SP q`.

    #[test]
    fn cursor_style_decscusr_encoding() {
        for (style, param) in [
            (CursorStyle::BlinkingBlock, 1),
            (CursorStyle::SteadyBlock, 2),
            (CursorStyle::BlinkingUnderline, 3),
            (CursorStyle::SteadyUnderline, 4),
            (CursorStyle::BlinkingBar, 5),
            (CursorStyle::SteadyBar, 6),
        ] {
            let mut buf: Vec<u8> = Vec::new();
            write_cursor_style(&mut buf, style).unwrap();
            let want = format!("\x1b[{param} q");
            assert_eq!(String::from_utf8(buf).unwrap(), want, "style {style:?}");
        }
    }
}
