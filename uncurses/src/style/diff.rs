//! Style diff algorithm — generates the minimal SGR sequence to transition
//! between styles.
//!
//! emitted as a *single* `CSI ... m` sequence whose parameters are joined by
//! `;` (with `:` only inside the underline sub-style token).

use std::io::{self, Write};

use super::sgr::{SgrSeq, push_bg_params, push_fg_params, push_sep, push_underline_color_params};
use super::{AttrFlags, RESET, Style, UnderlineStyle};

/// Write the minimal SGR sequence to transition from `from` to `to`.
///
/// Returns `true` if any output was written.
pub fn write_style_diff<W: Write>(w: &mut W, from: &Style, to: &Style) -> io::Result<bool> {
    if from == to {
        return Ok(false);
    }

    if to.is_empty() {
        w.write_all(RESET)?;
        return Ok(true);
    }

    if from.is_empty() {
        super::sgr::write_style(w, to)?;
        return Ok(true);
    }

    // Compute which attrs need to be cleared. SGR 22 clears both bold and
    // faint together; SGR 25 clears both blink kinds together. We coordinate
    // those by re-emitting any attrs in `to` that 22/25 would also wipe.
    let removed_attrs = from.attrs & !to.attrs;

    let mut seq = SgrSeq::new();
    seq.extend_from_slice(b"\x1b[");
    let body_start = seq.len();

    // If bold or faint is being removed, SGR 22 (normal intensity) turns off
    // *both*. Re-emit whichever of bold/faint survives in `to`.
    if removed_attrs.intersects(AttrFlags::BOLD | AttrFlags::FAINT) {
        push_sep(&mut seq, body_start);
        seq.extend_from_slice(b"22");
        if to.attrs.contains(AttrFlags::BOLD) {
            push_sep(&mut seq, body_start);
            seq.extend_from_slice(b"1");
        }
        if to.attrs.contains(AttrFlags::FAINT) {
            push_sep(&mut seq, body_start);
            seq.extend_from_slice(b"2");
        }
    }

    // If slow or rapid blink is being removed, SGR 25 turns off both kinds.
    if removed_attrs.intersects(AttrFlags::SLOW_BLINK | AttrFlags::RAPID_BLINK) {
        push_sep(&mut seq, body_start);
        seq.extend_from_slice(b"25");
        if to.attrs.contains(AttrFlags::SLOW_BLINK) {
            push_sep(&mut seq, body_start);
            seq.extend_from_slice(b"5");
        }
        if to.attrs.contains(AttrFlags::RAPID_BLINK) {
            push_sep(&mut seq, body_start);
            seq.extend_from_slice(b"6");
        }
    }

    // Remove individually-removable attrs.
    for (flag, code) in [
        (AttrFlags::ITALIC, b"23" as &[u8]),
        (AttrFlags::REVERSE, b"27"),
        (AttrFlags::CONCEAL, b"28"),
        (AttrFlags::STRIKETHROUGH, b"29"),
    ] {
        if removed_attrs.contains(flag) {
            push_sep(&mut seq, body_start);
            seq.extend_from_slice(code);
        }
    }

    // Add new attrs.
    let added_attrs = to.attrs & !from.attrs;
    let bold_faint_handled = removed_attrs.intersects(AttrFlags::BOLD | AttrFlags::FAINT);
    let blink_handled = removed_attrs.intersects(AttrFlags::SLOW_BLINK | AttrFlags::RAPID_BLINK);
    for (flag, code) in [
        (AttrFlags::BOLD, b"1" as &[u8]),
        (AttrFlags::FAINT, b"2"),
        (AttrFlags::ITALIC, b"3"),
        (AttrFlags::SLOW_BLINK, b"5"),
        (AttrFlags::RAPID_BLINK, b"6"),
        (AttrFlags::REVERSE, b"7"),
        (AttrFlags::CONCEAL, b"8"),
        (AttrFlags::STRIKETHROUGH, b"9"),
    ] {
        // bold/faint and blink-kind additions are only emitted by the
        // SGR 22 / SGR 25 coordination block above when something was
        // being removed. If nothing was removed, we still need to emit
        // the additions explicitly.
        if bold_faint_handled && matches!(flag, AttrFlags::BOLD | AttrFlags::FAINT) {
            continue;
        }
        if blink_handled && matches!(flag, AttrFlags::SLOW_BLINK | AttrFlags::RAPID_BLINK) {
            continue;
        }
        if added_attrs.contains(flag) {
            push_sep(&mut seq, body_start);
            seq.extend_from_slice(code);
        }
    }

    if from.underline != to.underline {
        let token: &[u8] = match to.underline {
            UnderlineStyle::None => b"24",
            UnderlineStyle::Single => b"4",
            UnderlineStyle::Double => b"4:2",
            UnderlineStyle::Curly => b"4:3",
            UnderlineStyle::Dotted => b"4:4",
            UnderlineStyle::Dashed => b"4:5",
        };
        push_sep(&mut seq, body_start);
        seq.extend_from_slice(token);
    }

    if from.fg != to.fg {
        push_sep(&mut seq, body_start);
        match to.fg {
            Some(c) => push_fg_params(&mut seq, c),
            None => seq.extend_from_slice(b"39"),
        }
    }
    if from.bg != to.bg {
        push_sep(&mut seq, body_start);
        match to.bg {
            Some(c) => push_bg_params(&mut seq, c),
            None => seq.extend_from_slice(b"49"),
        }
    }
    if from.underline_color != to.underline_color {
        push_sep(&mut seq, body_start);
        match to.underline_color {
            Some(c) => push_underline_color_params(&mut seq, c),
            None => seq.extend_from_slice(b"59"),
        }
    }

    if seq.len() == body_start {
        return Ok(false);
    }

    seq.push(b'm');
    seq.flush(w)?;
    Ok(true)
}

/// Downsample a style to the given color profile.
pub fn convert_style(style: &Style, profile: crate::color::Profile) -> Style {
    use crate::color::Profile;
    match profile {
        Profile::Disabled => Style::EMPTY,
        Profile::Ascii => Style {
            fg: None,
            bg: None,
            underline_color: None,
            ..*style
        },
        _ => Style {
            fg: style.fg.and_then(|c| profile.convert(c)),
            bg: style.bg.and_then(|c| profile.convert(c)),
            underline_color: style.underline_color.and_then(|c| profile.convert(c)),
            ..*style
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{BasicColor, Color};

    #[test]
    fn test_diff_no_change() {
        let mut buf = Vec::new();
        let s = Style::EMPTY.bold();
        let wrote = write_style_diff(&mut buf, &s, &s).unwrap();
        assert!(!wrote);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_diff_to_empty() {
        let mut buf = Vec::new();
        let from = Style::EMPTY.bold();
        let wrote = write_style_diff(&mut buf, &from, &Style::EMPTY).unwrap();
        assert!(wrote);
        assert_eq!(buf, b"\x1b[m");
    }

    #[test]
    fn test_diff_add_italic() {
        let mut buf = Vec::new();
        let from = Style::EMPTY.bold();
        let to = Style::EMPTY.bold().italic();
        let wrote = write_style_diff(&mut buf, &from, &to).unwrap();
        assert!(wrote);
        assert_eq!(buf, b"\x1b[3m");
    }

    #[test]
    fn test_diff_add_italic_and_fg_single_csi() {
        let mut buf = Vec::new();
        let from = Style::EMPTY.bold();
        let to = Style::EMPTY
            .bold()
            .italic()
            .with_fg(Color::Basic(BasicColor::Red));
        let wrote = write_style_diff(&mut buf, &from, &to).unwrap();
        assert!(wrote);
        // Combined into a single CSI ... m
        assert_eq!(buf, b"\x1b[3;31m");
    }

    #[test]
    fn test_diff_remove_bold_uses_sgr22() {
        let mut buf = Vec::new();
        let from = Style::EMPTY.bold().italic();
        let to = Style::EMPTY.italic();
        let wrote = write_style_diff(&mut buf, &from, &to).unwrap();
        assert!(wrote);
        // SGR 22 turns off bold; italic is preserved (no diff needed for it).
        assert_eq!(buf, b"\x1b[22m");
    }

    #[test]
    fn test_diff_remove_bold_keep_faint_uses_sgr22_then_2() {
        let mut buf = Vec::new();
        let mut from = Style::EMPTY;
        from.attrs |= AttrFlags::BOLD;
        from.attrs |= AttrFlags::FAINT;
        let mut to = Style::EMPTY;
        to.attrs |= AttrFlags::FAINT;
        let wrote = write_style_diff(&mut buf, &from, &to).unwrap();
        assert!(wrote);
        assert_eq!(buf, b"\x1b[22;2m");
    }

    #[test]
    fn test_diff_add_faint_only() {
        // Going from a plain selection (bg+fg, no bold/faint) to a faint
        // divider must emit SGR 2. Previously the "add" loop skipped
        // bold/faint unconditionally, so the faint never reached the wire.
        let mut buf = Vec::new();
        let from = Style::EMPTY
            .with_fg(Color::Basic(BasicColor::BrightWhite))
            .with_bg(Color::Basic(BasicColor::Blue));
        let to = Style::EMPTY.faint();
        let wrote = write_style_diff(&mut buf, &from, &to).unwrap();
        assert!(wrote);
        assert_eq!(buf, b"\x1b[2;39;49m");
    }

    #[test]
    fn test_diff_add_bold_only() {
        let mut buf = Vec::new();
        let from = Style::EMPTY.italic();
        let to = Style::EMPTY.italic().bold();
        let wrote = write_style_diff(&mut buf, &from, &to).unwrap();
        assert!(wrote);
        assert_eq!(buf, b"\x1b[1m");
    }

    #[test]
    fn test_diff_add_slow_blink_only() {
        let mut buf = Vec::new();
        let from = Style::EMPTY.italic();
        let mut to = Style::EMPTY;
        to.attrs |= AttrFlags::ITALIC | AttrFlags::SLOW_BLINK;
        let wrote = write_style_diff(&mut buf, &from, &to).unwrap();
        assert!(wrote);
        assert_eq!(buf, b"\x1b[5m");
    }

    #[test]
    fn test_diff_remove_blink_uses_sgr25() {
        let mut buf = Vec::new();
        let mut from = Style::EMPTY;
        from.attrs |= AttrFlags::SLOW_BLINK;
        from.attrs |= AttrFlags::ITALIC;
        let mut to = Style::EMPTY;
        to.attrs |= AttrFlags::ITALIC;
        let wrote = write_style_diff(&mut buf, &from, &to).unwrap();
        assert!(wrote);
        assert_eq!(buf, b"\x1b[25m");
    }

    #[test]
    fn test_diff_fg_change() {
        let mut buf = Vec::new();
        let from = Style::EMPTY.with_fg(Color::Basic(BasicColor::Red));
        let to = Style::EMPTY.with_fg(Color::Basic(BasicColor::Blue));
        let wrote = write_style_diff(&mut buf, &from, &to).unwrap();
        assert!(wrote);
        assert_eq!(buf, b"\x1b[34m");
    }

    #[test]
    fn test_diff_remove_fg() {
        let mut buf = Vec::new();
        let from = Style::EMPTY.with_fg(Color::Basic(BasicColor::Red));
        let to = Style::EMPTY;
        let wrote = write_style_diff(&mut buf, &from, &to).unwrap();
        assert!(wrote);
        assert_eq!(buf, b"\x1b[m");
    }

    #[test]
    fn test_diff_fg_and_bg_change_single_csi() {
        let mut buf = Vec::new();
        let from = Style::EMPTY
            .with_fg(Color::Basic(BasicColor::Red))
            .with_bg(Color::Basic(BasicColor::Black));
        let to = Style::EMPTY
            .with_fg(Color::Basic(BasicColor::Blue))
            .with_bg(Color::Indexed(7));
        let wrote = write_style_diff(&mut buf, &from, &to).unwrap();
        assert!(wrote);
        assert_eq!(buf, b"\x1b[34;48;5;7m");
    }

    #[test]
    fn test_convert_style_notty() {
        let s = Style::EMPTY.bold().with_fg(Color::Basic(BasicColor::Red));
        let converted = convert_style(&s, crate::color::Profile::Disabled);
        assert!(converted.is_empty());
    }

    #[test]
    fn test_convert_style_ascii() {
        let s = Style::EMPTY.bold().with_fg(Color::Basic(BasicColor::Red));
        let converted = convert_style(&s, crate::color::Profile::Ascii);
        assert!(converted.attrs.contains(AttrFlags::BOLD));
        assert_eq!(converted.fg, None);
    }
}
