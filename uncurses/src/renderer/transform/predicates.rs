//! Cell-shape predicates used by the transform algorithm to decide
//! whether a region can be reproduced by an erase, whether a row is
//! blank under the current pen, and whether bytes can be repeated
//! via REP.

use crate::cell::CellRef;
use crate::style::{AttrFlags, UnderlineStyle};

/// Whether `cell` is safe to reproduce by emitting one of the EL/ED
/// erase sequences.
///
/// When the terminal supports background-color erase (BCE), the erase
/// paints the cleared region with the current pen's background. In
/// that case bg / fg / underline colors are all fine. Without BCE, the
/// erase paints with the terminal's default background, so the cell
/// must additionally have a default background to be reproducible.
///
/// The cell must always be a plain space, with no link, no underline,
/// and only "invisible-on-blank" attributes (bold/faint/italic/blink).
pub(super) fn can_clear_with(cell: CellRef<'_>, bce: bool) -> bool {
    if cell.width() != 1 || cell.content_bytes() != b" " || cell.link().is_some() {
        return false;
    }
    let allowed_attrs = AttrFlags::BOLD
        | AttrFlags::FAINT
        | AttrFlags::ITALIC
        | AttrFlags::SLOW_BLINK
        | AttrFlags::RAPID_BLINK;
    if cell.underline_shape() != UnderlineStyle::None
        || !(cell.attributes() - allowed_attrs).is_empty()
    {
        return false;
    }
    // Without BCE, the cleared region is painted with the terminal's
    // default background — so any non-default bg in the blank would be
    // lost. Underline color rides on the same erase path, gate it too.
    bce || (cell.bg_color().is_none() && cell.ul_color().is_none())
}

/// Equality used by `clear_bottom` to decide whether a row's cells
/// match the cell ED would produce with the current pen. Continuation
/// cells (the second half of a wide grapheme) are not blank — a wide
/// cell straddling into the row means the row is non-blank.
pub(super) fn cells_equal_blank(cell: CellRef<'_>, blank: CellRef<'_>) -> bool {
    if cell.is_continuation() {
        return false;
    }
    cell.width() == blank.width()
        && (cell.content_bytes() == blank.content_bytes()
            || (cell.content_bytes().is_empty() && blank.content_bytes() == b" ")
            || (cell.content_bytes() == b" " && blank.content_bytes().is_empty()))
        && cell.style_eq_ref(blank)
}

/// Whether `bytes` is safe to repeat with the REP escape.
///
/// REP repeats the last rune the terminal printed, not the full
/// grapheme cluster. Restrict to single printable ASCII bytes in the
/// US..DEL range so multi-byte and multi-rune cells aren't corrupted
/// by terminals that apply REP to the trailing rune only.
pub(super) fn is_rep_ascii(bytes: &[u8]) -> bool {
    bytes.len() == 1 && bytes[0] >= 0x1F && bytes[0] < 0x7F
}
