//! Mouse event types and encoding/decoding.

use super::Event;
use super::key::KeyModifiers;
use crate::ansi::params::Params;

/// Mouse button identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
    Button4,
    Button5,
    Button6,
    Button7,
    Button8,
    Button9,
    Button10,
    Button11,
    None,
}

/// A mouse event with position and button info.
///
/// Coordinates are zero-based; `(0, 0)` is the upper-left corner of the
/// terminal. When the terminal is using SGR-Pixel encoding (DEC mode 1016),
/// `x`/`y` are in pixel coordinates; otherwise they are in cell coordinates.
/// Use [`mouse_pixel_to_cell`] to convert pixel coordinates to cell
/// coordinates when appropriate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mouse {
    pub x: u16,
    pub y: u16,
    pub button: MouseButton,
    pub modifiers: KeyModifiers,
}

impl Mouse {
    /// Construct a mouse event with no modifiers.
    pub fn new(x: u16, y: u16, button: MouseButton) -> Self {
        Self {
            x,
            y,
            button,
            modifiers: KeyModifiers::empty(),
        }
    }
}

/// Convert a mouse event reported in pixel coordinates (SGR-Pixel / mode 1016)
/// to cell coordinates, using the given terminal pixel and cell dimensions.
///
/// `pixel_width` / `pixel_height` are the terminal's pixel dimensions (e.g.
/// from a `WindowOp(4)` query → [`crate::event::Event::WindowPixelSize`]);
/// `cols` / `rows` are the terminal's cell dimensions.
pub fn mouse_pixel_to_cell(
    m: Mouse,
    pixel_width: u16,
    pixel_height: u16,
    cols: u16,
    rows: u16,
) -> Mouse {
    let x = if pixel_width > 0 {
        (m.x as u32 * cols as u32 / pixel_width as u32) as u16
    } else {
        0
    };
    let y = if pixel_height > 0 {
        (m.y as u32 * rows as u32 / pixel_height as u32) as u16
    } else {
        0
    };
    Mouse {
        x,
        y,
        button: m.button,
        modifiers: m.modifiers,
    }
}

/// Decode a SGR mouse event from CSI < params.
///
/// Format: `CSI < Cb ; Cx ; Cy M` (press) or `CSI < Cb ; Cx ; Cy m` (release).
/// Coordinates are normalized to be zero-based regardless of encoding; callers
/// using SGR-Pixel (mode 1016) should treat the resulting `Mouse.x/y` as pixel
/// offsets (and use [`mouse_pixel_to_cell`] if cell coordinates are needed).
pub(crate) fn decode_sgr_mouse(params: Params<'_>, is_release: bool) -> Option<Event> {
    if params.len() < 3 {
        return None;
    }

    let cb = params.get_or(0, 0) as u16;
    let cx = (params.get_or(1, 0).saturating_sub(1)) as u16;
    let cy = (params.get_or(2, 0).saturating_sub(1)) as u16;

    Some(build_mouse_event(cb, cx, cy, Some(is_release)))
}

/// Decode an X10 mouse event.
///
/// Format: `CSI M Cb Cx Cy` (all bytes are raw + 32 offset)
pub(crate) fn decode_x10_mouse(cb: u8, cx: u8, cy: u8) -> Option<Event> {
    let cb = cb.wrapping_sub(32) as u16;
    let cx = cx.wrapping_sub(33) as u16;
    let cy = cy.wrapping_sub(33) as u16;
    Some(build_mouse_event(cb, cx, cy, None))
}

/// Decode an URxvt mouse event (mode 1015).
///
/// Format: `CSI Cb ; Cx ; Cy M`. Same semantics as X10 but with decimal
/// parameters; the release bit is encoded inside `Cb & 3 == 3`.
pub(crate) fn decode_urxvt_mouse(params: Params<'_>) -> Option<Event> {
    if params.len() < 3 {
        return None;
    }
    let cb = (params.get_or(0, 0).wrapping_sub(32)) as u16;
    let cx = (params.get_or(1, 0).saturating_sub(1)) as u16;
    let cy = (params.get_or(2, 0).saturating_sub(1)) as u16;
    Some(build_mouse_event(cb, cx, cy, None))
}

/// Decode a UTF-8 mouse event (mode 1005).
///
/// Format: `CSI M` followed by three UTF-8 codepoints, each = value + 32.
/// Returns `None` if `data` doesn't start with a complete sequence; returns
/// `Some((event, consumed))` on success.
pub(crate) fn decode_utf8_mouse(data: &[u8]) -> Option<(Event, usize)> {
    // Decode three codepoints
    let mut idx = 0;
    let (cb, n) = decode_utf8_codepoint(&data[idx..])?;
    idx += n;
    let (cx, n) = decode_utf8_codepoint(&data[idx..])?;
    idx += n;
    let (cy, n) = decode_utf8_codepoint(&data[idx..])?;
    idx += n;

    let cb = cb.wrapping_sub(32);
    let cx = cx.wrapping_sub(33);
    let cy = cy.wrapping_sub(33);
    Some((build_mouse_event(cb, cx, cy, None), idx))
}

fn decode_utf8_codepoint(data: &[u8]) -> Option<(u16, usize)> {
    if data.is_empty() {
        return None;
    }
    let b0 = data[0];
    let (cp, len) = if b0 < 0x80 {
        (b0 as u32, 1)
    } else if b0 < 0xc0 {
        return None;
    } else if b0 < 0xe0 {
        if data.len() < 2 {
            return None;
        }
        (((b0 as u32 & 0x1f) << 6) | (data[1] as u32 & 0x3f), 2)
    } else if b0 < 0xf0 {
        if data.len() < 3 {
            return None;
        }
        (
            ((b0 as u32 & 0x0f) << 12) | ((data[1] as u32 & 0x3f) << 6) | (data[2] as u32 & 0x3f),
            3,
        )
    } else {
        return None;
    };
    Some((cp as u16, len))
}

/// Common SGR/X10/URxvt/UTF-8 mouse cb decoder. `is_release` is `Some` only for
/// SGR mode where release is signalled by the `m` final byte; otherwise it's
/// derived from `cb & 3 == 3`.
fn build_mouse_event(cb: u16, cx: u16, cy: u16, is_release: Option<bool>) -> Event {
    let modifiers = decode_mouse_modifiers(cb);
    let is_move = cb & 32 != 0;
    let is_wheel = cb & 64 != 0;
    let is_extra = cb & 128 != 0;

    let button = if is_wheel {
        match cb & 3 {
            0 => MouseButton::WheelUp,
            1 => MouseButton::WheelDown,
            2 => MouseButton::WheelLeft,
            3 => MouseButton::WheelRight,
            _ => MouseButton::None,
        }
    } else if is_extra {
        // Extra buttons (8..=11) via bit 7
        match cb & 3 {
            0 => MouseButton::Button8,
            1 => MouseButton::Button9,
            2 => MouseButton::Button10,
            3 => MouseButton::Button11,
            _ => MouseButton::None,
        }
    } else {
        match cb & 3 {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            3 => MouseButton::None, // legacy release
            _ => MouseButton::None,
        }
    };

    let mouse = Mouse {
        x: cx,
        y: cy,
        button,
        modifiers,
    };

    let release = is_release.unwrap_or(cb & 3 == 3 && !is_wheel && !is_extra);
    if is_wheel {
        Event::MouseWheel(mouse)
    } else if release {
        Event::MouseRelease(mouse)
    } else if is_move {
        Event::MouseMove(mouse)
    } else {
        Event::MouseClick(mouse)
    }
}

/// Encode a mouse button + modifiers into the xterm bitfield for SGR encoding.
///
/// Returns `None` if `event` is not a mouse variant.
#[allow(dead_code)]
pub(crate) fn encode_sgr_mouse(event: &Event) -> Option<(u16, u16, u16, bool)> {
    let m = match event {
        Event::MouseClick(m)
        | Event::MouseRelease(m)
        | Event::MouseWheel(m)
        | Event::MouseMove(m) => m,
        _ => return None,
    };
    let mut cb: u16 = match m.button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::None => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
        MouseButton::WheelLeft => 66,
        MouseButton::WheelRight => 67,
        MouseButton::Button4 => 128,
        MouseButton::Button5 => 129,
        MouseButton::Button6 => 130,
        MouseButton::Button7 => 131,
        MouseButton::Button8 => 256,
        MouseButton::Button9 => 257,
        MouseButton::Button10 => 258,
        MouseButton::Button11 => 259,
    };

    if m.modifiers.contains(KeyModifiers::SHIFT) {
        cb |= 4;
    }
    if m.modifiers.contains(KeyModifiers::ALT) {
        cb |= 8;
    }
    if m.modifiers.contains(KeyModifiers::CTRL) {
        cb |= 16;
    }

    if matches!(event, Event::MouseMove(_)) {
        cb |= 32;
    }

    let is_release = matches!(event, Event::MouseRelease(_));
    Some((cb, m.x + 1, m.y + 1, is_release))
}

/// Write a SGR mouse event sequence. Returns `Ok(())` without writing if
/// `event` is not a mouse variant.
#[allow(dead_code)]
pub(crate) fn write_sgr_mouse<W: std::io::Write>(w: &mut W, event: &Event) -> std::io::Result<()> {
    let Some((cb, cx, cy, is_release)) = encode_sgr_mouse(event) else {
        return Ok(());
    };
    let final_char = if is_release { 'm' } else { 'M' };
    write!(w, "\x1b[<{cb};{cx};{cy}{final_char}")
}

fn decode_mouse_modifiers(cb: u16) -> KeyModifiers {
    let mut mods = KeyModifiers::empty();
    if cb & 4 != 0 {
        mods |= KeyModifiers::SHIFT;
    }
    if cb & 8 != 0 {
        mods |= KeyModifiers::ALT;
    }
    if cb & 16 != 0 {
        mods |= KeyModifiers::CTRL;
    }
    mods
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_sgr_left_click() {
        let event = decode_sgr_mouse(Params::from_raw(b"0;10;20"), false).unwrap();
        match event {
            Event::MouseClick(m) => {
                assert_eq!(m.button, MouseButton::Left);
                assert_eq!(m.x, 9);
                assert_eq!(m.y, 19);
            }
            _ => panic!("Expected MouseClick"),
        }
    }

    #[test]
    fn test_decode_sgr_release() {
        let event = decode_sgr_mouse(Params::from_raw(b"0;5;5"), true).unwrap();
        assert!(matches!(event, Event::MouseRelease(_)));
    }

    #[test]
    fn test_decode_sgr_wheel() {
        let event = decode_sgr_mouse(Params::from_raw(b"64;1;1"), false).unwrap();
        match event {
            Event::MouseWheel(m) => {
                assert_eq!(m.button, MouseButton::WheelUp);
            }
            _ => panic!("Expected MouseWheel"),
        }
    }

    #[test]
    fn test_mouse_pixel_to_cell() {
        // 800x600 pixels, 80x24 cells → each cell is 10px wide, 25px tall
        let m = Mouse::new(100, 200, MouseButton::Left);
        let c = mouse_pixel_to_cell(m, 800, 600, 80, 24);
        assert_eq!(c.x, 10);
        assert_eq!(c.y, 8);
    }

    #[test]
    fn test_decode_urxvt_click() {
        // cb = 32 (left, no mods), cx=11 (10), cy=21 (20)
        let event = decode_urxvt_mouse(Params::from_raw(b"32;11;21")).unwrap();
        match event {
            Event::MouseClick(m) => {
                assert_eq!(m.button, MouseButton::Left);
                assert_eq!(m.x, 10);
                assert_eq!(m.y, 20);
            }
            _ => panic!("Expected MouseClick"),
        }
    }

    #[test]
    fn test_decode_utf8_mouse() {
        // cb=32, cx=33, cy=33 (all single-byte ASCII)
        let (event, n) = decode_utf8_mouse(b" !!").unwrap();
        assert_eq!(n, 3);
        match event {
            Event::MouseClick(m) => {
                assert_eq!(m.x, 0);
                assert_eq!(m.y, 0);
            }
            _ => panic!("Expected MouseClick"),
        }
    }

    #[test]
    fn test_sgr_roundtrip() {
        let original = Event::MouseClick(Mouse::new(10, 20, MouseButton::Left));

        let mut buf = Vec::new();
        write_sgr_mouse(&mut buf, &original).unwrap();
        assert_eq!(buf, b"\x1b[<0;11;21M");
    }
}
