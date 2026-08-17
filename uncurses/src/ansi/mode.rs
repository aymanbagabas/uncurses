//! ANSI and DEC private terminal mode management.
//!
//! ## Category
//!
//! This module encodes SM/RM, DECSET/DECRST, DECRQM, and report responses for
//! terminal modes. It also names commonly used modes such as alternate screen,
//! mouse tracking, bracketed paste, synchronized output, and in-band resize.
//!
//! ## CSI anatomy
//!
//! Standard ANSI modes omit a private prefix; DEC private modes insert `?` after
//! CSI. Final byte `h` sets a mode, `l` resets it, `$p` requests state, and `$y`
//! reports state.
//!
//! ```text
//! ESC [  ?  2 0 4 8  h        CSI ? 2048 h  (enable mode 2048)
//! ──┬── ─┬─ ───┬──── ┬
//!  CSI  priv  params final
//! ```
//!
//! ## Batching conventions
//!
//! [`write_set_mode`] and [`write_reset_mode`] split mixed mode slices into DEC
//! and ANSI sequences because the prefixes differ. An empty slice emits nothing.

use std::io::{self, Write};

/// A terminal mode addressable by ANSI SM/RM or DECSET/DECRST.
///
/// [`Mode::Ansi`] writes ordinary CSI mode numbers such as `ESC [ 4 h`;
/// [`Mode::Dec`] writes private mode numbers with `?`, such as
/// `ESC [ ? 1049 h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mode {
    /// Standard ANSI mode number.
    ///
    /// Set/reset/request forms use `ESC [ <number> h`, `ESC [ <number> l`, and `ESC [ <number> $ p`.
    Ansi(u16),
    /// DEC private mode number.
    ///
    /// Set/reset/request forms use `ESC [ ? <number> h`, `ESC [ ? <number> l`, and `ESC [ ? <number> $ p`.
    Dec(u16),
}

// Well-known DEC private modes
impl Mode {
    /// DEC private mode 1 (DECCKM): cursor keys send application sequences when set and normal cursor sequences when reset.
    pub const CURSOR_KEYS: Mode = Mode::Dec(1);
    /// DEC private mode 2 (DECANM): selects ANSI mode when set and VT52 mode when reset on terminals that implement it.
    pub const ANSI_VT52: Mode = Mode::Dec(2);
    /// DEC private mode 3 (DECCOLM): 132-column mode when set; commonly resets margins and clears display on supporting terminals.
    pub const COLUMN_132: Mode = Mode::Dec(3);
    /// DEC private mode 4 (DECSCLM): smooth scrolling when set, jump scrolling when reset.
    pub const SMOOTH_SCROLL: Mode = Mode::Dec(4);
    /// DEC private mode 5 (DECSCNM): reverse-video screen mode.
    pub const REVERSE_VIDEO: Mode = Mode::Dec(5);
    /// DEC private mode 6 (DECOM): absolute cursor positions are relative to the scroll region when set.
    pub const ORIGIN: Mode = Mode::Dec(6);
    /// DEC private mode 7 (DECAWM): automatically wrap at the right margin when set.
    pub const AUTO_WRAP: Mode = Mode::Dec(7);
    /// DEC private mode 8 (DECARM): key auto-repeat enabled when set.
    pub const AUTO_REPEAT: Mode = Mode::Dec(8);
    /// DEC private mode 9: X10 mouse reporting.
    pub const MOUSE_X10: Mode = Mode::Dec(9);
    /// DEC private mode 18 (DECPFF): print form-feed mode.
    pub const PRINT_FORM_FEED: Mode = Mode::Dec(18);
    /// DEC private mode 19 (DECPEX): print extent mode.
    pub const PRINT_EXTENT: Mode = Mode::Dec(19);
    /// DEC private mode 25 (DECTCEM): cursor visible when set, hidden when reset.
    pub const CURSOR_VISIBLE: Mode = Mode::Dec(25);
    /// DEC private mode 40 (DECNCSM): allow column-mode changes without clearing on supporting terminals.
    pub const NO_CLEAR_COLUMN: Mode = Mode::Dec(40);
    /// DEC private mode 66 (DECNKM): keypad application/numeric behavior; see [`crate::ansi::keypad`].
    pub const NUMERIC_KEYPAD: Mode = Mode::Dec(66);
    /// DEC private mode 67 (DECBKM): backarrow key sends backspace/delete according to set/reset state.
    pub const BACKARROW_KEY: Mode = Mode::Dec(67);
    /// DEC private mode 69 (DECLRMM): enables left/right margin interpretation for DECSLRM.
    pub const LEFT_RIGHT_MARGIN: Mode = Mode::Dec(69);
    /// DEC private mode 47: legacy alternate screen buffer.
    pub const ALT_SCREEN_LEGACY: Mode = Mode::Dec(47);
    /// DEC private mode 1000: report mouse button press/release events.
    pub const MOUSE_NORMAL: Mode = Mode::Dec(1000);
    /// DEC private mode 1001: highlight mouse tracking.
    pub const MOUSE_HIGHLIGHT: Mode = Mode::Dec(1001);
    /// DEC private mode 1002: report button-motion mouse events.
    pub const MOUSE_BUTTON: Mode = Mode::Dec(1002);
    /// DEC private mode 1003: report any-motion mouse events.
    pub const MOUSE_ANY: Mode = Mode::Dec(1003);
    /// DEC private mode 1004: enable focus in/out reports (`ESC [ I` / `ESC [ O`).
    pub const FOCUS: Mode = Mode::Dec(1004);
    /// DEC private mode 1005: UTF-8 mouse coordinate encoding.
    pub const MOUSE_UTF8: Mode = Mode::Dec(1005);
    /// DEC private mode 1006: SGR mouse coordinate encoding.
    pub const MOUSE_SGR: Mode = Mode::Dec(1006);
    /// DEC private mode 1015: alternate mouse coordinate encoding.
    pub const MOUSE_URXVT: Mode = Mode::Dec(1015);
    /// DEC private mode 1016: SGR-pixel mouse coordinate encoding.
    pub const MOUSE_SGR_PIXEL: Mode = Mode::Dec(1016);
    /// Alternate screen buffer (1047).
    pub const ALT_SCREEN: Mode = Mode::Dec(1047);
    /// DEC private mode 1048: save/restore cursor around mode set/reset.
    pub const SAVE_CURSOR: Mode = Mode::Dec(1048);
    /// DEC private mode 1049: alternate screen buffer with cursor save/restore and clear semantics.
    pub const ALT_SCREEN_SAVE_CURSOR: Mode = Mode::Dec(1049);
    /// DEC private mode 2004: wrap pasted data in bracketed-paste delimiters.
    pub const BRACKETED_PASTE: Mode = Mode::Dec(2004);
    /// DEC private mode 2026: synchronized output batching.
    pub const SYNCHRONIZED_OUTPUT: Mode = Mode::Dec(2026);
    /// DEC private mode 2027: Unicode core keyboard/input behavior on supporting terminals.
    pub const UNICODE_CORE: Mode = Mode::Dec(2027);
    /// DEC private mode 2031: light/dark color-scheme notifications.
    pub const LIGHT_DARK: Mode = Mode::Dec(2031);
    /// DEC private mode 2048: in-band resize reports.
    pub const IN_BAND_RESIZE: Mode = Mode::Dec(2048);
    /// DEC private mode 9001: Win32-input reporting on supporting terminals.
    pub const WIN32_INPUT: Mode = Mode::Dec(9001);
}

// Well-known ANSI modes
impl Mode {
    /// ANSI mode 2 (KAM): keyboard action mode.
    pub const KEYBOARD_ACTION: Mode = Mode::Ansi(2);
    /// ANSI mode 4 (IRM): insert mode when set, replace mode when reset.
    pub const INSERT: Mode = Mode::Ansi(4);
    /// ANSI mode 8 (BDSM): bidirectional-support mode.
    pub const BIDI_SUPPORT: Mode = Mode::Ansi(8);
    /// ANSI mode 12 (SRM): send/receive mode, often associated with local echo behavior.
    pub const SEND_RECEIVE: Mode = Mode::Ansi(12);
    /// ANSI mode 20 (LNM): line-feed/new-line handling mode.
    pub const LINE_FEED_NEW_LINE: Mode = Mode::Ansi(20);
}

/// Decoded state value from a mode report response (`DECRPM` or ANSI report).
///
/// The numeric values are the `Ps` status field in `ESC [ ? mode ; Ps $ y` or
/// `ESC [ mode ; Ps $ y`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModeSetting {
    /// Report value `0`: the terminal does not recognize the requested mode.
    NotRecognized,
    /// Report value `1`: the mode is currently set.
    Set,
    /// Report value `2`: the mode is currently reset.
    Reset,
    /// Report value `3`: the mode is permanently set and cannot be reset.
    PermanentlySet,
    /// Report value `4`: the mode is permanently reset and cannot be set.
    PermanentlyReset,
}

impl ModeSetting {
    /// Convert a numeric mode-report status value into [`ModeSetting`].
    ///
    /// Values `1..=4` map to their defined states; every other value maps to [`ModeSetting::NotRecognized`].
    pub fn from_value(v: u16) -> Self {
        match v {
            1 => ModeSetting::Set,
            2 => ModeSetting::Reset,
            3 => ModeSetting::PermanentlySet,
            4 => ModeSetting::PermanentlyReset,
            _ => ModeSetting::NotRecognized,
        }
    }

    /// Return the numeric status value used in mode-report responses.
    pub fn value(self) -> u16 {
        match self {
            ModeSetting::NotRecognized => 0,
            ModeSetting::Set => 1,
            ModeSetting::Reset => 2,
            ModeSetting::PermanentlySet => 3,
            ModeSetting::PermanentlyReset => 4,
        }
    }

    /// Return `true` for [`ModeSetting::Set`] and [`ModeSetting::PermanentlySet`].
    pub fn is_set(self) -> bool {
        matches!(self, ModeSetting::Set | ModeSetting::PermanentlySet)
    }

    /// Return `true` for [`ModeSetting::Reset`] and [`ModeSetting::PermanentlyReset`].
    pub fn is_reset(self) -> bool {
        matches!(self, ModeSetting::Reset | ModeSetting::PermanentlyReset)
    }

    /// Return whether the terminal recognized the mode.
    ///
    /// Only [`ModeSetting::NotRecognized`] is considered unrecognized.
    pub fn is_recognized(self) -> bool {
        !matches!(self, ModeSetting::NotRecognized)
    }

    /// Return whether the mode is permanently fixed and cannot be toggled.
    ///
    /// Both [`ModeSetting::PermanentlySet`] and [`ModeSetting::PermanentlyReset`]
    /// report a state the host cannot change.
    pub fn is_permanent(self) -> bool {
        matches!(
            self,
            ModeSetting::PermanentlySet | ModeSetting::PermanentlyReset
        )
    }

    /// Return whether the mode can actually be used by the host.
    ///
    /// This is `true` for every recognized state except
    /// [`ModeSetting::PermanentlyReset`]: a permanently reset mode is
    /// recognized but the terminal will never allow it to be set, so the
    /// feature it gates is effectively unavailable. Use this (rather than
    /// [`is_recognized`](Self::is_recognized)) when deciding whether a
    /// capability can be relied upon.
    pub fn is_available(self) -> bool {
        matches!(
            self,
            ModeSetting::Set | ModeSetting::Reset | ModeSetting::PermanentlySet
        )
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

/// Set one or more modes.
///
/// DEC private modes emit `ESC [ ? ... h`; ANSI modes emit `ESC [ ... h`. If `modes` contains both kinds, this function writes one DEC sequence and one ANSI sequence. An empty slice emits nothing.
pub fn write_set_mode<W: Write>(w: &mut W, modes: &[Mode]) -> io::Result<()> {
    write_mode_seq(w, modes, b'h')
}

/// Reset one or more modes.
///
/// DEC private modes emit `ESC [ ? ... l`; ANSI modes emit `ESC [ ... l`. Mixed slices are split by mode kind. An empty slice emits nothing.
pub fn write_reset_mode<W: Write>(w: &mut W, modes: &[Mode]) -> io::Result<()> {
    write_mode_seq(w, modes, b'l')
}

impl Mode {
    /// Write the set-mode sequence for this single mode.
    ///
    /// This is a convenience wrapper around [`write_set_mode`].
    pub fn set<W: Write>(self, w: &mut W) -> io::Result<()> {
        write_set_mode(w, &[self])
    }

    /// Write the reset-mode sequence for this single mode.
    ///
    /// This is a convenience wrapper around [`write_reset_mode`].
    pub fn reset<W: Write>(self, w: &mut W) -> io::Result<()> {
        write_reset_mode(w, &[self])
    }

    /// Request this mode's current state with DECRQM/RQM.
    ///
    /// DEC modes emit `ESC [ ? <mode> $ p`; ANSI modes emit `ESC [ <mode> $ p`.
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

/// Request mode status.
///
/// DEC modes emit `ESC [ ? <mode> $ p`; ANSI modes emit `ESC [ <mode> $ p`. The terminal response can be represented by [`ModeSetting`].
pub fn write_request_mode<W: Write>(w: &mut W, mode: Mode) -> io::Result<()> {
    match mode {
        Mode::Dec(n) => write!(w, "\x1b[?{n}$p"),
        Mode::Ansi(n) => write!(w, "\x1b[{n}$p"),
    }
}

/// Write a mode-report response.
///
/// DEC modes emit `ESC [ ? <mode> ; <setting> $ y`; ANSI modes emit `ESC [ <mode> ; <setting> $ y`. Use when synthesizing input reports or testing parsers.
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
        // Permanent states are the two that cannot be toggled.
        assert!(ModeSetting::PermanentlySet.is_permanent());
        assert!(ModeSetting::PermanentlyReset.is_permanent());
        assert!(!ModeSetting::Set.is_permanent());
        assert!(!ModeSetting::Reset.is_permanent());
        assert!(!ModeSetting::NotRecognized.is_permanent());
        // A mode is usable unless it is unrecognized or permanently reset.
        assert!(ModeSetting::Set.is_available());
        assert!(ModeSetting::Reset.is_available());
        assert!(ModeSetting::PermanentlySet.is_available());
        assert!(!ModeSetting::PermanentlyReset.is_available());
        assert!(!ModeSetting::NotRecognized.is_available());
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
