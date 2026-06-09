//! Key types and key codes for terminal input.

use bitflags::bitflags;
use std::fmt;

bitflags! {
    /// Keyboard modifier flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct KeyModifiers: u16 {
        const SHIFT       = 0b0000_0000_0000_0001;
        const ALT         = 0b0000_0000_0000_0010;
        const CTRL        = 0b0000_0000_0000_0100;
        const META        = 0b0000_0000_0000_1000;
        const HYPER       = 0b0000_0000_0001_0000;
        const SUPER       = 0b0000_0000_0010_0000;
        const CAPS_LOCK   = 0b0000_0000_0100_0000;
        const NUM_LOCK    = 0b0000_0000_1000_0000;
        const SCROLL_LOCK = 0b0000_0001_0000_0000;
    }
}

/// A key event.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    /// The key code (which key was pressed).
    pub code: KeyCode,
    /// Active modifiers.
    pub modifiers: KeyModifiers,
    /// Associated text (from Kitty protocol or composed input).
    pub text: Option<String>,
    /// Shifted-layer codepoint (Kitty Report-Alternate-Keys).
    pub shifted_key: Option<char>,
    /// Base-layout codepoint (Kitty Report-Alternate-Keys).
    pub base_key: Option<char>,
}

impl Key {
    /// Construct a key with default modifiers and no associated text.
    ///
    /// When `code` is a printable `KeyCode::Char`, `text` is
    /// pre-populated with the codepoint so callers reading typed
    /// input don't have to fall back to inspecting `code`. Control
    /// codepoints leave `text` as `None`.
    pub fn new(code: KeyCode) -> Self {
        let text = printable_text(code, KeyModifiers::empty());
        Self {
            code,
            modifiers: KeyModifiers::empty(),
            text,
            shifted_key: None,
            base_key: None,
        }
    }

    /// Replace the active modifiers. If `text` was the codepoint
    /// auto-populated by [`Self::new`] and the new modifiers make
    /// the key non-printable (anything beyond Shift), `text` is
    /// cleared. Modifier sets that still yield typed input (none,
    /// Shift) keep the existing text untouched. An explicitly set
    /// `text` (via [`Self::with_text`]) is never overwritten here.
    pub fn with_modifiers(mut self, mods: KeyModifiers) -> Self {
        if self.text.as_deref() == printable_text(self.code, KeyModifiers::empty()).as_deref()
            && printable_text(self.code, mods).is_none()
        {
            self.text = None;
        }
        self.modifiers = mods;
        self
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Return the character if this is a simple character key.
    /// [`KeyCode::Space`] reports `' '`; other named keys report `None`.
    pub fn char(&self) -> Option<char> {
        match self.code {
            KeyCode::Char(c) => Some(c),
            KeyCode::Space => Some(' '),
            _ => None,
        }
    }
}

/// Normalize the case of a [`Key`] produced by a decoder so the keycode
/// is always lowercase and the uppercase variant lives in `shifted_key`.
///
/// Two complementary transforms, both safe to run idempotently:
///
/// * When `code` is an uppercase `Char` with a single-codepoint
///   lowercase mapping, the code is lowered, the original uppercase is
///   stored in `shifted_key` (unless already populated), and `SHIFT` is
///   added to `modifiers` — except when `CAPS_LOCK` is already set, in
///   which case CapsLock is the legitimate reason for the uppercase and
///   no synthetic Shift is added.
/// * When `code` is a lowercase `Char` and `modifiers` already contains
///   `SHIFT` or `CAPS_LOCK`, `shifted_key` is populated with the
///   single-codepoint uppercase mapping (unless already populated).
///
/// In both branches, when the modifier set still represents typed input
/// (none, Shift, CapsLock, NumLock; i.e. no Ctrl/Alt/Super/Hyper/Meta)
/// and `text` is empty, `text` is populated with the *shifted* glyph —
/// the character the user perceives as typed.
///
/// No-op for any of: non-`Char` codes, codepoints with no proper case
/// flip, multi-codepoint case mappings (e.g. Turkish `İ` lowercases to
/// `i\u{307}`), or characters whose `to_lowercase`/`to_uppercase` is
/// the identity (titlecase digraphs like `ǅ`).
pub(crate) fn normalize_shift_case(key: &mut Key) {
    let KeyCode::Char(c) = key.code else { return };

    // Modifiers that do not suppress printable input.
    const PRINTABLE_ALLOWED: KeyModifiers = KeyModifiers::SHIFT
        .union(KeyModifiers::CAPS_LOCK)
        .union(KeyModifiers::NUM_LOCK);
    let printable = (key.modifiers - PRINTABLE_ALLOWED).is_empty();

    // `text` is treated as auto-populated (and thus safe to upgrade to
    // the shifted glyph) when it's empty or matches the codepoint we're
    // about to transform. Any other value is caller-explicit and we
    // must leave it untouched.
    let mut tmp = [0u8; 4];
    let original_str: &str = c.encode_utf8(&mut tmp);
    let text_is_pre_shift = key
        .text
        .as_deref()
        .map(|t| t == original_str)
        .unwrap_or(true);

    if c.is_uppercase() {
        let mut iter = c.to_lowercase();
        let Some(lower) = iter.next() else { return };
        if iter.next().is_some() || lower == c {
            return;
        }
        key.code = KeyCode::Char(lower);
        if key.shifted_key.is_none() {
            key.shifted_key = Some(c);
        }
        if !key.modifiers.contains(KeyModifiers::CAPS_LOCK) {
            key.modifiers |= KeyModifiers::SHIFT;
        }
        if text_is_pre_shift {
            if printable {
                key.text = Some(c.to_string());
            } else {
                // Non-printable mods (e.g. Ctrl): drop the auto-populated
                // glyph so consumers don't see "typed text" for Ctrl+A.
                key.text = None;
            }
        }
    } else if c.is_lowercase()
        && key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::CAPS_LOCK)
    {
        let mut iter = c.to_uppercase();
        let Some(upper) = iter.next() else { return };
        if iter.next().is_some() || upper == c {
            return;
        }
        if key.shifted_key.is_none() {
            key.shifted_key = Some(upper);
        }
        if text_is_pre_shift {
            if printable {
                key.text = Some(upper.to_string());
            } else {
                key.text = None;
            }
        }
    }
}

/// Codepoint-as-string for a printable [`KeyCode::Char`] or [`KeyCode::Space`]
/// with a modifier set that doesn't override the typed glyph (none / Shift /
/// locks); `None` for control codepoints or modifier sets that make the key
/// non-printable (Ctrl, Alt, …).
fn printable_text(code: KeyCode, mods: KeyModifiers) -> Option<String> {
    const ALLOWED: KeyModifiers = KeyModifiers::SHIFT
        .union(KeyModifiers::CAPS_LOCK)
        .union(KeyModifiers::NUM_LOCK);
    if (mods - ALLOWED).is_empty() {
        match code {
            KeyCode::Char(c) if !c.is_control() => Some(c.to_string()),
            KeyCode::Space => Some(" ".to_string()),
            _ => None,
        }
    } else {
        None
    }
}

/// Identifies which key was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    /// A printable character.
    Char(char),
    /// Function key (F1-F24).
    F(u8),
    /// Legacy VT220 Find key, distinct from Home. Only emitted when the
    /// decoder is configured with [`DecoderFlags::FIND_KEY`].
    Find,
    /// Legacy VT220 Select key, distinct from End. Only emitted when the
    /// decoder is configured with [`DecoderFlags::SELECT_KEY`].
    Select,
    // Navigation
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    // Editing
    Backspace,
    Delete,
    Insert,
    Tab,
    BackTab,
    Enter,
    // Whitespace
    Space,
    // Special
    Escape,
    CapsLock,
    ScrollLock,
    NumLock,
    PrintScreen,
    Pause,
    Menu,
    // Keypad
    KpEnter,
    KpAdd,
    KpSubtract,
    KpMultiply,
    KpDivide,
    KpDecimal,
    KpEqual,
    KpSeparator,
    KpLeft,
    KpRight,
    KpUp,
    KpDown,
    KpPageUp,
    KpPageDown,
    KpHome,
    KpEnd,
    KpInsert,
    KpDelete,
    KpBegin,
    Kp0,
    Kp1,
    Kp2,
    Kp3,
    Kp4,
    Kp5,
    Kp6,
    Kp7,
    Kp8,
    Kp9,
    // Media
    MediaPlay,
    MediaPause,
    MediaPlayPause,
    MediaReverse,
    MediaStop,
    MediaRewind,
    MediaFastForward,
    MediaNext,
    MediaPrev,
    MediaRecord,
    VolumeUp,
    VolumeDown,
    VolumeMute,
    // Modifier keys (reported with Kitty protocol)
    LeftShift,
    RightShift,
    LeftCtrl,
    RightCtrl,
    LeftAlt,
    RightAlt,
    LeftSuper,
    RightSuper,
    LeftHyper,
    RightHyper,
    LeftMeta,
    RightMeta,
    /// ISO Level 3 Shift (typically AltGr).
    IsoLevel3Shift,
    /// ISO Level 5 Shift.
    IsoLevel5Shift,
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mods = self.modifiers;
        if mods.contains(KeyModifiers::CTRL) {
            f.write_str("ctrl+")?;
        }
        if mods.contains(KeyModifiers::ALT) {
            f.write_str("alt+")?;
        }
        if mods.contains(KeyModifiers::SHIFT) {
            f.write_str("shift+")?;
        }
        if mods.contains(KeyModifiers::META) {
            f.write_str("meta+")?;
        }
        if mods.contains(KeyModifiers::HYPER) {
            f.write_str("hyper+")?;
        }
        if mods.contains(KeyModifiers::SUPER) {
            f.write_str("super+")?;
        }
        if mods.contains(KeyModifiers::CAPS_LOCK) {
            f.write_str("capslock+")?;
        }
        if mods.contains(KeyModifiers::NUM_LOCK) {
            f.write_str("numlock+")?;
        }
        if mods.contains(KeyModifiers::SCROLL_LOCK) {
            f.write_str("scrolllock+")?;
        }

        match self.code {
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::F(n) => write!(f, "f{n}"),
            KeyCode::Up => f.write_str("up"),
            KeyCode::Down => f.write_str("down"),
            KeyCode::Left => f.write_str("left"),
            KeyCode::Right => f.write_str("right"),
            KeyCode::Home => f.write_str("home"),
            KeyCode::End => f.write_str("end"),
            KeyCode::Find => f.write_str("find"),
            KeyCode::Select => f.write_str("select"),
            KeyCode::PageUp => f.write_str("pgup"),
            KeyCode::PageDown => f.write_str("pgdn"),
            KeyCode::Backspace => f.write_str("backspace"),
            KeyCode::Delete => f.write_str("delete"),
            KeyCode::Insert => f.write_str("insert"),
            KeyCode::Tab => f.write_str("tab"),
            KeyCode::BackTab => f.write_str("backtab"),
            KeyCode::Enter => f.write_str("enter"),
            KeyCode::Space => f.write_str("space"),
            KeyCode::Escape => f.write_str("esc"),
            KeyCode::CapsLock => f.write_str("capslock"),
            KeyCode::ScrollLock => f.write_str("scrolllock"),
            KeyCode::NumLock => f.write_str("numlock"),
            KeyCode::PrintScreen => f.write_str("printscreen"),
            KeyCode::Pause => f.write_str("pause"),
            KeyCode::Menu => f.write_str("menu"),
            _ => write!(f, "{:?}", self.code),
        }
    }
}

impl fmt::Display for KeyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Delegate to Key display with no modifiers
        write!(
            f,
            "{}",
            Key {
                code: *self,
                modifiers: KeyModifiers::empty(),
                text: None,
                shifted_key: None,
                base_key: None,
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_display() {
        let k = Key::new(KeyCode::Char('a')).with_modifiers(KeyModifiers::CTRL);
        assert_eq!(k.to_string(), "ctrl+a");
    }

    #[test]
    fn test_key_display_function() {
        let k = Key::new(KeyCode::F(12));
        assert_eq!(k.to_string(), "f12");
    }

    #[test]
    fn test_key_char() {
        let k = Key::new(KeyCode::Char('x'));
        assert_eq!(k.char(), Some('x'));
    }

    #[test]
    fn test_key_char_special() {
        let k = Key::new(KeyCode::Enter);
        assert_eq!(k.char(), None);
    }

    #[test]
    fn normalize_uppercase_ascii_lowers_code_and_adds_shift() {
        let mut k = Key::new(KeyCode::Char('A'));
        normalize_shift_case(&mut k);
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("A"));
    }

    #[test]
    fn normalize_uppercase_with_caps_lock_does_not_add_shift() {
        let mut k = Key::new(KeyCode::Char('A')).with_modifiers(KeyModifiers::CAPS_LOCK);
        normalize_shift_case(&mut k);
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(!k.modifiers.contains(KeyModifiers::SHIFT));
        assert!(k.modifiers.contains(KeyModifiers::CAPS_LOCK));
        assert_eq!(k.text.as_deref(), Some("A"));
    }

    #[test]
    fn normalize_lowercase_with_shift_populates_shifted_key() {
        let mut k = Key::new(KeyCode::Char('a')).with_modifiers(KeyModifiers::SHIFT);
        normalize_shift_case(&mut k);
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert_eq!(k.text.as_deref(), Some("A"));
    }

    #[test]
    fn normalize_lowercase_without_shift_unchanged() {
        let mut k = Key::new(KeyCode::Char('a'));
        normalize_shift_case(&mut k);
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, None);
        assert!(k.modifiers.is_empty());
        // text was auto-populated by Key::new for the printable char;
        // helper is a no-op for lowercase without Shift/Caps.
        assert_eq!(k.text.as_deref(), Some("a"));
    }

    #[test]
    fn normalize_ctrl_uppercase_does_not_set_text() {
        // Ctrl+A: code is uppercase, but Ctrl suppresses typed glyph —
        // text must stay None even though we still lower-case the code.
        let mut k = Key::new(KeyCode::Char('A')).with_modifiers(KeyModifiers::CTRL);
        normalize_shift_case(&mut k);
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(k.modifiers.contains(KeyModifiers::CTRL));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert!(k.text.is_none());
    }

    #[test]
    fn normalize_ctrl_shift_lowercase_does_not_set_text() {
        let mut k =
            Key::new(KeyCode::Char('a')).with_modifiers(KeyModifiers::CTRL | KeyModifiers::SHIFT);
        normalize_shift_case(&mut k);
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(k.text.is_none());
    }

    #[test]
    fn normalize_does_not_overwrite_existing_text() {
        let mut k = Key::new(KeyCode::Char('A')).with_text("A");
        normalize_shift_case(&mut k);
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.text.as_deref(), Some("A"));
    }

    #[test]
    fn normalize_cyrillic_uppercase() {
        let mut k = Key::new(KeyCode::Char('Ц'));
        normalize_shift_case(&mut k);
        assert_eq!(k.code, KeyCode::Char('ц'));
        assert_eq!(k.shifted_key, Some('Ц'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("Ц"));
    }

    #[test]
    fn normalize_greek_lowercase_with_shift() {
        let mut k = Key::new(KeyCode::Char('α')).with_modifiers(KeyModifiers::SHIFT);
        normalize_shift_case(&mut k);
        assert_eq!(k.code, KeyCode::Char('α'));
        assert_eq!(k.shifted_key, Some('Α'));
        assert_eq!(k.text.as_deref(), Some("Α"));
    }

    #[test]
    fn normalize_multi_codepoint_lower_left_alone() {
        // 'İ' lowercases to "i\u{307}" — two codepoints; leave as-is.
        let mut k = Key::new(KeyCode::Char('İ'));
        normalize_shift_case(&mut k);
        assert_eq!(k.code, KeyCode::Char('İ'));
        assert_eq!(k.shifted_key, None);
        assert!(k.modifiers.is_empty());
        // text auto-populated; helper bails on multi-cp lowercase.
        assert_eq!(k.text.as_deref(), Some("İ"));
    }

    #[test]
    fn normalize_titlecase_digraph_left_alone() {
        // 'ǅ' is titlecase; is_uppercase() and is_lowercase() are both false.
        let mut k = Key::new(KeyCode::Char('ǅ'));
        normalize_shift_case(&mut k);
        assert_eq!(k.code, KeyCode::Char('ǅ'));
        assert_eq!(k.shifted_key, None);
    }

    #[test]
    fn normalize_digit_unchanged() {
        let mut k = Key::new(KeyCode::Char('1')).with_modifiers(KeyModifiers::SHIFT);
        normalize_shift_case(&mut k);
        assert_eq!(k.code, KeyCode::Char('1'));
        assert_eq!(k.shifted_key, None);
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        // text auto-populated by Key::new and kept across with_modifiers
        // because Shift alone is still printable; helper doesn't touch
        // codepoints without a case variant.
        assert_eq!(k.text.as_deref(), Some("1"));
    }

    #[test]
    fn normalize_non_char_unchanged() {
        let mut k = Key::new(KeyCode::Enter).with_modifiers(KeyModifiers::SHIFT);
        normalize_shift_case(&mut k);
        assert_eq!(k.code, KeyCode::Enter);
        assert_eq!(k.shifted_key, None);
    }

    #[test]
    fn normalize_preserves_existing_shifted_key() {
        let mut k = Key {
            code: KeyCode::Char('A'),
            modifiers: KeyModifiers::empty(),
            text: Some("A".to_string()),
            shifted_key: Some('!'),
            base_key: None,
        };
        normalize_shift_case(&mut k);
        assert_eq!(k.code, KeyCode::Char('a'));
        // Pre-existing shifted_key must not be overwritten.
        assert_eq!(k.shifted_key, Some('!'));
        assert_eq!(k.text.as_deref(), Some("A"));
    }
}
