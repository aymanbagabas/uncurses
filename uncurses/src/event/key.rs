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
    /// Construct a canonical key from its required fields.
    ///
    /// Runs normalization once at construction:
    ///
    /// * If `code` is an uppercase `Char` with a single-codepoint
    ///   lowercase mapping, the code is lowered, the original
    ///   uppercase is stored in `shifted_key`, and `SHIFT` is added to
    ///   `modifiers` (unless `CAPS_LOCK` is already set, in which case
    ///   no synthetic Shift is added).
    /// * If `code` is a lowercase `Char` and `modifiers` already
    ///   contains `SHIFT` or `CAPS_LOCK`, `shifted_key` is populated
    ///   with the single-codepoint uppercase mapping.
    /// * If the resulting code is printable (`Char` non-control or
    ///   `Space`) and `modifiers` is a printable subset
    ///   (`SHIFT | CAPS_LOCK | NUM_LOCK`), `text` is auto-populated
    ///   with the user-perceived glyph (shifted form when shifted).
    ///
    /// No-op for: non-`Char` codes, codepoints without a proper
    /// single-codepoint case flip (e.g. Turkish `İ`), titlecase
    /// digraphs (e.g. `ǅ`), and modifier sets that suppress printable
    /// input (Ctrl, Alt, Super, Hyper, Meta).
    ///
    /// Optional fields (`text`, `shifted_key`, `base_key`) start
    /// empty; mutate them directly on the returned [`Key`] when a
    /// decoder protocol surfaces extra information (e.g. kitty's
    /// reported shifted codepoint or associated text).
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        let mut k = Self {
            code,
            modifiers,
            text: None,
            shifted_key: None,
            base_key: None,
        };
        k.normalize();
        k
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

    /// Apply the canonicalization rules described on [`Self::new`].
    /// Idempotent.
    fn normalize(&mut self) {
        // Case folding for printable Char codes.
        if let KeyCode::Char(c) = self.code {
            if c.is_uppercase() {
                let mut iter = c.to_lowercase();
                if let Some(lower) = iter.next()
                    && iter.next().is_none()
                    && lower != c
                {
                    self.code = KeyCode::Char(lower);
                    if self.shifted_key.is_none() {
                        self.shifted_key = Some(c);
                    }
                    if !self.modifiers.contains(KeyModifiers::CAPS_LOCK) {
                        self.modifiers |= KeyModifiers::SHIFT;
                    }
                }
            } else if c.is_lowercase()
                && self
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::CAPS_LOCK)
                && self.shifted_key.is_none()
            {
                let mut iter = c.to_uppercase();
                if let Some(upper) = iter.next()
                    && iter.next().is_none()
                    && upper != c
                {
                    self.shifted_key = Some(upper);
                }
            }
        }

        // Text auto-population for printable input.
        if self.text.is_none() {
            const PRINTABLE_ALLOWED: KeyModifiers = KeyModifiers::SHIFT
                .union(KeyModifiers::CAPS_LOCK)
                .union(KeyModifiers::NUM_LOCK);
            if (self.modifiers - PRINTABLE_ALLOWED).is_empty() {
                let glyph: Option<char> = match self.code {
                    KeyCode::Char(c) if !c.is_control() => Some(self.shifted_key.unwrap_or(c)),
                    KeyCode::Space => Some(' '),
                    _ => None,
                };
                if let Some(g) = glyph {
                    self.text = Some(g.to_string());
                }
            }
        }
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
        let k = Key::new(KeyCode::Char('a'), KeyModifiers::CTRL);
        assert_eq!(k.to_string(), "ctrl+a");
    }

    #[test]
    fn test_key_display_function() {
        let k = Key::new(KeyCode::F(12), KeyModifiers::empty());
        assert_eq!(k.to_string(), "f12");
    }

    #[test]
    fn test_key_char() {
        let k = Key::new(KeyCode::Char('x'), KeyModifiers::empty());
        assert_eq!(k.char(), Some('x'));
    }

    #[test]
    fn test_key_char_special() {
        let k = Key::new(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(k.char(), None);
    }

    #[test]
    fn new_uppercase_ascii_lowers_code_and_adds_shift() {
        let k = Key::new(KeyCode::Char('A'), KeyModifiers::empty());
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("A"));
    }

    #[test]
    fn new_uppercase_with_caps_lock_does_not_add_shift() {
        let k = Key::new(KeyCode::Char('A'), KeyModifiers::CAPS_LOCK);
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(!k.modifiers.contains(KeyModifiers::SHIFT));
        assert!(k.modifiers.contains(KeyModifiers::CAPS_LOCK));
        assert_eq!(k.text.as_deref(), Some("A"));
    }

    #[test]
    fn new_lowercase_with_shift_populates_shifted_key() {
        let k = Key::new(KeyCode::Char('a'), KeyModifiers::SHIFT);
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert_eq!(k.text.as_deref(), Some("A"));
    }

    #[test]
    fn new_lowercase_without_shift_populates_text() {
        let k = Key::new(KeyCode::Char('a'), KeyModifiers::empty());
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, None);
        assert!(k.modifiers.is_empty());
        assert_eq!(k.text.as_deref(), Some("a"));
    }

    #[test]
    fn new_ctrl_uppercase_does_not_set_text() {
        let k = Key::new(KeyCode::Char('A'), KeyModifiers::CTRL);
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(k.modifiers.contains(KeyModifiers::CTRL));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert!(k.text.is_none());
    }

    #[test]
    fn new_ctrl_shift_lowercase_does_not_set_text() {
        let k = Key::new(KeyCode::Char('a'), KeyModifiers::CTRL | KeyModifiers::SHIFT);
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(k.text.is_none());
    }

    #[test]
    fn new_cyrillic_uppercase() {
        let k = Key::new(KeyCode::Char('Ц'), KeyModifiers::empty());
        assert_eq!(k.code, KeyCode::Char('ц'));
        assert_eq!(k.shifted_key, Some('Ц'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("Ц"));
    }

    #[test]
    fn new_greek_lowercase_with_shift() {
        let k = Key::new(KeyCode::Char('α'), KeyModifiers::SHIFT);
        assert_eq!(k.code, KeyCode::Char('α'));
        assert_eq!(k.shifted_key, Some('Α'));
        assert_eq!(k.text.as_deref(), Some("Α"));
    }

    #[test]
    fn new_multi_codepoint_lower_left_alone() {
        // 'İ' lowercases to "i\u{307}" — two codepoints; leave as-is.
        let k = Key::new(KeyCode::Char('İ'), KeyModifiers::empty());
        assert_eq!(k.code, KeyCode::Char('İ'));
        assert_eq!(k.shifted_key, None);
        assert!(k.modifiers.is_empty());
        // Still printable input — text auto-populates with the original codepoint.
        assert_eq!(k.text.as_deref(), Some("İ"));
    }

    #[test]
    fn new_titlecase_digraph_left_alone() {
        // 'ǅ' is titlecase; is_uppercase() and is_lowercase() are both false.
        let k = Key::new(KeyCode::Char('ǅ'), KeyModifiers::empty());
        assert_eq!(k.code, KeyCode::Char('ǅ'));
        assert_eq!(k.shifted_key, None);
        assert_eq!(k.text.as_deref(), Some("ǅ"));
    }

    #[test]
    fn new_digit_with_shift_keeps_digit_text() {
        // '1' has no case variant; text auto-populates from the codepoint.
        let k = Key::new(KeyCode::Char('1'), KeyModifiers::SHIFT);
        assert_eq!(k.code, KeyCode::Char('1'));
        assert_eq!(k.shifted_key, None);
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("1"));
    }

    #[test]
    fn new_non_char_no_text() {
        let k = Key::new(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(k.code, KeyCode::Enter);
        assert_eq!(k.shifted_key, None);
        assert!(k.text.is_none());
    }

    #[test]
    fn new_space_populates_text() {
        let k = Key::new(KeyCode::Space, KeyModifiers::empty());
        assert_eq!(k.code, KeyCode::Space);
        assert_eq!(k.text.as_deref(), Some(" "));
    }

    #[test]
    fn new_space_with_ctrl_no_text() {
        let k = Key::new(KeyCode::Space, KeyModifiers::CTRL);
        assert_eq!(k.code, KeyCode::Space);
        assert!(k.text.is_none());
    }

    #[test]
    fn direct_field_mutation_overrides_auto_text() {
        let mut k = Key::new(KeyCode::Char('2'), KeyModifiers::SHIFT);
        // Simulate a decoder that knows the terminal-reported shifted glyph.
        k.shifted_key = Some('@');
        k.text = Some("@".to_string());
        assert_eq!(k.text.as_deref(), Some("@"));
        assert_eq!(k.shifted_key, Some('@'));
    }
}
