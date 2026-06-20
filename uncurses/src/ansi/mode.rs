//! Terminal mode management (ANSI and DEC private modes).

use std::io::{self, Write};

/// A terminal mode that can be set or reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Standard ANSI mode.
    Ansi(u16),
    /// DEC private mode (CSI ? ... h/l).
    Dec(u16),
}

// Well-known DEC private modes
impl Mode {
    /// Cursor keys application mode (DECCKM).
    pub const CURSOR_KEYS: Mode = Mode::Dec(1);
    /// ANSI/VT52 mode (DECANM).
    pub const ANSI_VT52: Mode = Mode::Dec(2);
    /// 132 column mode (DECCOLM).
    pub const COLUMN_132: Mode = Mode::Dec(3);
    /// Smooth scroll mode (DECSCLM).
    pub const SMOOTH_SCROLL: Mode = Mode::Dec(4);
    /// Reverse video mode (DECSCNM).
    pub const REVERSE_VIDEO: Mode = Mode::Dec(5);
    /// Origin mode (DECOM).
    pub const ORIGIN: Mode = Mode::Dec(6);
    /// Auto-wrap mode (DECAWM).
    pub const AUTO_WRAP: Mode = Mode::Dec(7);
    /// Auto-repeat keys (DECARM).
    pub const AUTO_REPEAT: Mode = Mode::Dec(8);
    /// X10 mouse tracking.
    pub const MOUSE_X10: Mode = Mode::Dec(9);
    /// Print form-feed mode (DECPFF).
    pub const PRINT_FORM_FEED: Mode = Mode::Dec(18);
    /// Print extent mode (DECPEX).
    pub const PRINT_EXTENT: Mode = Mode::Dec(19);
    /// Cursor visibility (DECTCEM).
    pub const CURSOR_VISIBLE: Mode = Mode::Dec(25);
    /// No-clear column mode (DECNCSM).
    pub const NO_CLEAR_COLUMN: Mode = Mode::Dec(40);
    /// Numeric keypad mode (DECNKM).
    pub const NUMERIC_KEYPAD: Mode = Mode::Dec(66);
    /// Backarrow key mode (DECBKM).
    pub const BACKARROW_KEY: Mode = Mode::Dec(67);
    /// Left/right margin mode (DECLRMM).
    pub const LEFT_RIGHT_MARGIN: Mode = Mode::Dec(69);
    /// Legacy alternate screen buffer (no clear, no save).
    pub const ALT_SCREEN_LEGACY: Mode = Mode::Dec(47);
    /// Normal mouse tracking (button events).
    pub const MOUSE_NORMAL: Mode = Mode::Dec(1000);
    /// Highlight mouse tracking.
    pub const MOUSE_HIGHLIGHT: Mode = Mode::Dec(1001);
    /// Button-event mouse tracking.
    pub const MOUSE_BUTTON: Mode = Mode::Dec(1002);
    /// Any-event mouse tracking.
    pub const MOUSE_ANY: Mode = Mode::Dec(1003);
    /// Focus tracking.
    pub const FOCUS: Mode = Mode::Dec(1004);
    /// UTF-8 mouse encoding.
    pub const MOUSE_UTF8: Mode = Mode::Dec(1005);
    /// SGR mouse encoding.
    pub const MOUSE_SGR: Mode = Mode::Dec(1006);
    /// URXVT mouse encoding.
    pub const MOUSE_URXVT: Mode = Mode::Dec(1015);
    /// SGR-pixel mouse encoding.
    pub const MOUSE_SGR_PIXEL: Mode = Mode::Dec(1016);
    /// Alternate screen buffer (1047).
    pub const ALT_SCREEN: Mode = Mode::Dec(1047);
    /// Save/restore cursor (1048).
    pub const SAVE_CURSOR: Mode = Mode::Dec(1048);
    /// Alternate screen buffer with save/restore cursor and clear (1049).
    pub const ALT_SCREEN_SAVE_CURSOR: Mode = Mode::Dec(1049);
    /// Bracketed paste mode.
    pub const BRACKETED_PASTE: Mode = Mode::Dec(2004);
    /// Synchronized output.
    pub const SYNCHRONIZED_OUTPUT: Mode = Mode::Dec(2026);
    /// Unicode core mode.
    pub const UNICODE_CORE: Mode = Mode::Dec(2027);
    /// Light/dark color scheme notifications.
    pub const LIGHT_DARK: Mode = Mode::Dec(2031);
    /// In-band resize.
    pub const IN_BAND_RESIZE: Mode = Mode::Dec(2048);
    /// Win32 input mode.
    pub const WIN32_INPUT: Mode = Mode::Dec(9001);
}

// Well-known ANSI modes
impl Mode {
    /// Keyboard action mode (KAM).
    pub const KEYBOARD_ACTION: Mode = Mode::Ansi(2);
    /// Insert/Replace mode (IRM).
    pub const INSERT: Mode = Mode::Ansi(4);
    /// Bi-directional support mode (BDSM).
    pub const BIDI_SUPPORT: Mode = Mode::Ansi(8);
    /// Send-receive mode / local echo (SRM).
    pub const SEND_RECEIVE: Mode = Mode::Ansi(12);
    /// Line feed / new line mode (LNM).
    pub const LINE_FEED_NEW_LINE: Mode = Mode::Ansi(20);
}

/// Result of a mode query (DECRPM / RM-style response).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModeSetting {
    /// Mode is not recognized by the terminal (response value 0).
    NotRecognized,
    /// Mode is currently set (response value 1).
    Set,
    /// Mode is currently reset (response value 2).
    Reset,
    /// Mode is permanently set, cannot be reset (response value 3).
    PermanentlySet,
    /// Mode is permanently reset, cannot be set (response value 4).
    PermanentlyReset,
}

impl ModeSetting {
    /// Parse a DECRPM/ANSI mode response value.
    pub fn from_value(v: u16) -> Self {
        match v {
            1 => ModeSetting::Set,
            2 => ModeSetting::Reset,
            3 => ModeSetting::PermanentlySet,
            4 => ModeSetting::PermanentlyReset,
            _ => ModeSetting::NotRecognized,
        }
    }

    /// Return the numeric value used in DECRPM/ANSI mode reports.
    pub fn value(self) -> u16 {
        match self {
            ModeSetting::NotRecognized => 0,
            ModeSetting::Set => 1,
            ModeSetting::Reset => 2,
            ModeSetting::PermanentlySet => 3,
            ModeSetting::PermanentlyReset => 4,
        }
    }

    /// Is this mode set (either temporarily or permanently)?
    pub fn is_set(self) -> bool {
        matches!(self, ModeSetting::Set | ModeSetting::PermanentlySet)
    }

    /// Is this mode reset (either temporarily or permanently)?
    pub fn is_reset(self) -> bool {
        matches!(self, ModeSetting::Reset | ModeSetting::PermanentlyReset)
    }

    /// Did the terminal recognize the mode? A reply of anything other than
    /// [`NotRecognized`](ModeSetting::NotRecognized) means the terminal
    /// knows the mode, i.e. it is supported.
    pub fn is_recognized(self) -> bool {
        !matches!(self, ModeSetting::NotRecognized)
    }
}

impl std::fmt::Display for ModeSetting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            ModeSetting::NotRecognized => "not recognized",
            ModeSetting::Set => "set",
            ModeSetting::Reset => "reset",
            ModeSetting::PermanentlySet => "permanently set",
            ModeSetting::PermanentlyReset => "permanently reset",
        };
        f.write_str(label)
    }
}

/// Set (enable) one or more modes.
pub fn write_set_mode<W: Write>(w: &mut W, modes: &[Mode]) -> io::Result<()> {
    write_mode_seq(w, modes, b'h')
}

/// Reset (disable) one or more modes.
pub fn write_reset_mode<W: Write>(w: &mut W, modes: &[Mode]) -> io::Result<()> {
    write_mode_seq(w, modes, b'l')
}

impl Mode {
    /// Write the sequence that sets (enables) this mode.
    ///
    /// Shorthand for `write_set_mode(w, &[self])`. Use [`write_set_mode`]
    /// directly to batch multiple modes into a single CSI sequence.
    pub fn set<W: Write>(self, w: &mut W) -> io::Result<()> {
        write_set_mode(w, &[self])
    }

    /// Write the sequence that resets (disables) this mode.
    ///
    /// Shorthand for `write_reset_mode(w, &[self])`.
    pub fn reset<W: Write>(self, w: &mut W) -> io::Result<()> {
        write_reset_mode(w, &[self])
    }

    /// Write a DECRQM request asking for this mode's current state.
    pub fn request<W: Write>(self, w: &mut W) -> io::Result<()> {
        write_request_mode(w, self)
    }
}

fn write_mode_seq<W: Write>(w: &mut W, modes: &[Mode], final_byte: u8) -> io::Result<()> {
    if modes.is_empty() {
        return Ok(());
    }

    // Separate ANSI and DEC modes — they use different CSI prefixes
    let mut ansi_modes = Vec::new();
    let mut dec_modes = Vec::new();

    for &mode in modes {
        match mode {
            Mode::Ansi(n) => ansi_modes.push(n),
            Mode::Dec(n) => dec_modes.push(n),
        }
    }

    if !dec_modes.is_empty() {
        w.write_all(b"\x1b[?")?;
        for (i, &n) in dec_modes.iter().enumerate() {
            if i > 0 {
                w.write_all(b";")?;
            }
            write!(w, "{n}")?;
        }
        w.write_all(&[final_byte])?;
    }

    if !ansi_modes.is_empty() {
        w.write_all(b"\x1b[")?;
        for (i, &n) in ansi_modes.iter().enumerate() {
            if i > 0 {
                w.write_all(b";")?;
            }
            write!(w, "{n}")?;
        }
        w.write_all(&[final_byte])?;
    }

    Ok(())
}

/// Request mode status (DECRQM).
pub fn write_request_mode<W: Write>(w: &mut W, mode: Mode) -> io::Result<()> {
    match mode {
        Mode::Dec(n) => write!(w, "\x1b[?{n}$p"),
        Mode::Ansi(n) => write!(w, "\x1b[{n}$p"),
    }
}

/// Write a DECRPM/RM mode-report response (`CSI ? mode ; setting $ y` for DEC,
/// `CSI mode ; setting $ y` for ANSI).
pub fn write_report_mode<W: Write>(w: &mut W, mode: Mode, setting: ModeSetting) -> io::Result<()> {
    let v = setting.value();
    match mode {
        Mode::Dec(n) => write!(w, "\x1b[?{n};{v}$y"),
        Mode::Ansi(n) => write!(w, "\x1b[{n};{v}$y"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_dec_mode() {
        let mut buf = Vec::new();
        write_set_mode(&mut buf, &[Mode::ALT_SCREEN_SAVE_CURSOR]).unwrap();
        assert_eq!(buf, b"\x1b[?1049h");
    }

    #[test]
    fn test_reset_dec_mode() {
        let mut buf = Vec::new();
        write_reset_mode(&mut buf, &[Mode::ALT_SCREEN_SAVE_CURSOR]).unwrap();
        assert_eq!(buf, b"\x1b[?1049l");
    }

    #[test]
    fn test_set_multiple_dec_modes() {
        let mut buf = Vec::new();
        write_set_mode(&mut buf, &[Mode::MOUSE_ANY, Mode::MOUSE_SGR]).unwrap();
        assert_eq!(buf, b"\x1b[?1003;1006h");
    }

    #[test]
    fn test_mode_setting_roundtrip() {
        for s in [
            ModeSetting::NotRecognized,
            ModeSetting::Set,
            ModeSetting::Reset,
            ModeSetting::PermanentlySet,
            ModeSetting::PermanentlyReset,
        ] {
            assert_eq!(ModeSetting::from_value(s.value()), s);
        }
        assert!(ModeSetting::Set.is_set());
        assert!(ModeSetting::PermanentlySet.is_set());
        assert!(!ModeSetting::Reset.is_set());
        assert!(ModeSetting::Reset.is_reset());
        assert!(ModeSetting::PermanentlyReset.is_reset());
        // A recognized mode is anything other than NotRecognized.
        assert!(!ModeSetting::NotRecognized.is_recognized());
        for s in [
            ModeSetting::Set,
            ModeSetting::Reset,
            ModeSetting::PermanentlySet,
            ModeSetting::PermanentlyReset,
        ] {
            assert!(s.is_recognized());
        }
    }

    #[test]
    fn test_mode_setting_display() {
        assert_eq!(ModeSetting::NotRecognized.to_string(), "not recognized");
        assert_eq!(ModeSetting::Set.to_string(), "set");
        assert_eq!(ModeSetting::Reset.to_string(), "reset");
        assert_eq!(ModeSetting::PermanentlySet.to_string(), "permanently set");
        assert_eq!(
            ModeSetting::PermanentlyReset.to_string(),
            "permanently reset"
        );
    }

    #[test]
    fn test_write_report_mode_dec() {
        let mut buf = Vec::new();
        write_report_mode(&mut buf, Mode::ALT_SCREEN_SAVE_CURSOR, ModeSetting::Set).unwrap();
        assert_eq!(buf, b"\x1b[?1049;1$y");
    }

    #[test]
    fn test_write_report_mode_ansi() {
        let mut buf = Vec::new();
        write_report_mode(&mut buf, Mode::INSERT, ModeSetting::Reset).unwrap();
        assert_eq!(buf, b"\x1b[4;2$y");
    }

    #[test]
    fn test_request_mode_dec() {
        let mut buf = Vec::new();
        write_request_mode(&mut buf, Mode::ALT_SCREEN_SAVE_CURSOR).unwrap();
        assert_eq!(buf, b"\x1b[?1049$p");
    }

    #[test]
    fn test_new_mode_constants() {
        assert_eq!(Mode::KEYBOARD_ACTION, Mode::Ansi(2));
        assert_eq!(Mode::BIDI_SUPPORT, Mode::Ansi(8));
        assert_eq!(Mode::SEND_RECEIVE, Mode::Ansi(12));
        assert_eq!(Mode::LINE_FEED_NEW_LINE, Mode::Ansi(20));
        assert_eq!(Mode::MOUSE_UTF8, Mode::Dec(1005));
        assert_eq!(Mode::MOUSE_URXVT, Mode::Dec(1015));
        assert_eq!(Mode::WIN32_INPUT, Mode::Dec(9001));
        assert_eq!(Mode::ALT_SCREEN_LEGACY, Mode::Dec(47));
        assert_eq!(Mode::LIGHT_DARK, Mode::Dec(2031));
    }
}
