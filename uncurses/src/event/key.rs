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
///
/// Equality and hashing intentionally consider only [`code`](Self::code)
/// and [`modifiers`](Self::modifiers): two keys representing the same
/// press compare equal regardless of whether informational fields
/// ([`text`](Self::text), [`shifted_key`](Self::shifted_key),
/// [`base_key`](Self::base_key)) were populated by the producing
/// decoder. This makes `Key` safe to use as a `HashMap` key for binding
/// lookups even when some decoder paths surface richer metadata than
/// others.
#[derive(Debug, Clone)]
pub struct Key {
    /// The key code (which key was pressed).
    pub code: KeyCode,
    /// Active modifiers.
    pub modifiers: KeyModifiers,
    /// Associated text (from Kitty protocol or composed input).
    ///
    /// Informational; ignored by `==` and `Hash`.
    pub text: Option<String>,
    /// Shifted-layer codepoint (Kitty Report-Alternate-Keys).
    ///
    /// Informational; ignored by `==` and `Hash`.
    pub shifted_key: Option<char>,
    /// Base-layout codepoint (Kitty Report-Alternate-Keys).
    ///
    /// Informational; ignored by `==` and `Hash`.
    pub base_key: Option<char>,
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code && self.modifiers == other.modifiers
    }
}

impl Eq for Key {}

impl std::hash::Hash for Key {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.code.hash(state);
        self.modifiers.hash(state);
    }
}

impl Key {
    /// Construct a canonical key from its required fields.
    ///
    /// Runs normalization once at construction. The exact rules are
    /// applied in this order:
    ///
    /// 1. **Tab + Shift → BackTab.** If `code` is [`KeyCode::Tab`] and
    ///    `modifiers` contains `SHIFT`, the code is rewritten to
    ///    [`KeyCode::BackTab`] and `SHIFT` is removed.
    /// 2. **Char case folding.** Always applied for `Char` codes
    ///    regardless of the other modifiers:
    ///    * Uppercase `Char` with a single-codepoint lowercase mapping:
    ///      code is lowered, the original uppercase is stored in
    ///      `shifted_key`, and `SHIFT` is added to `modifiers` (unless
    ///      `CAPS_LOCK` is already set, in which case no synthetic
    ///      Shift is added).
    ///    * Lowercase `Char` with `modifiers` already containing
    ///      `SHIFT` or `CAPS_LOCK`: `shifted_key` is populated with
    ///      the single-codepoint uppercase mapping.
    /// 3. **Printable text auto-population.** Only when `modifiers`
    ///    is a subset of `SHIFT | CAPS_LOCK | NUM_LOCK` and the
    ///    resulting code is printable (`Char` non-control or
    ///    [`KeyCode::Space`]): `text` is filled with the
    ///    user-perceived glyph. The shifted form is used only when
    ///    `SHIFT` or `CAPS_LOCK` is in effect; otherwise the bare
    ///    code character is used. Modifier sets containing Ctrl,
    ///    Alt, Super, Hyper, or Meta suppress this step.
    ///
    /// Steps 1 and 2 are unaffected by the presence of Ctrl/Alt/etc.;
    /// only step 3 is gated on the modifier set.
    ///
    /// Codepoints without a proper single-codepoint case flip (Turkish
    /// `İ`, titlecase digraphs like `ǅ`) leave the code unchanged.
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
    pub(crate) fn normalize(&mut self) {
        // Shift+Tab collapses to BackTab with no Shift modifier. This is
        // a universal convention (legacy `CSI Z`, kitty CSI u, MOK2),
        // not a protocol detail — apply it once here so every decoder
        // path emits the same canonical BackTab identity.
        if self.code == KeyCode::Tab && self.modifiers.contains(KeyModifiers::SHIFT) {
            self.code = KeyCode::BackTab;
            self.modifiers.remove(KeyModifiers::SHIFT);
        }

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
                // The shifted glyph is only what the user perceives
                // when the shifted layer is actually engaged. Decoders
                // may populate `shifted_key` regardless of whether
                // Shift is actually held, so we must not blindly use
                // it here.
                //
                // CapsLock only acts as a shifted layer for cased
                // letters. `Key::new` normalizes uppercase to lowercase
                // + SHIFT before we get here, so `is_lowercase()` is
                // the right test: it covers every Unicode cased letter
                // (ASCII, Cyrillic, Greek, Latin-Extended, …) and
                // excludes digits, symbols, and uncased scripts where
                // CapsLock has no effect on common layouts.
                //
                // Whether Shift cancels CapsLock on letters is a host
                // convention (some platforms cancel, most OR); the
                // library takes the OR-side that matches the majority.
                // Hosts wanting cancellation should populate `text`
                // directly from the resolved character.
                let glyph: Option<char> = match self.code {
                    KeyCode::Char(c) if !c.is_control() => {
                        let shifted = if c.is_lowercase() {
                            self.modifiers
                                .intersects(KeyModifiers::SHIFT | KeyModifiers::CAPS_LOCK)
                        } else {
                            self.modifiers.contains(KeyModifiers::SHIFT)
                        };
                        Some(if shifted {
                            self.shifted_key.unwrap_or(c)
                        } else {
                            c
                        })
                    }
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
        // Canonical order: ctrl, alt, shift, super, hyper, meta. Lock
        // states (Caps/Num/Scroll) are intentionally omitted — they are
        // session state, not binding modifiers.
        if mods.contains(KeyModifiers::CTRL) {
            f.write_str("ctrl+")?;
        }
        if mods.contains(KeyModifiers::ALT) {
            f.write_str("alt+")?;
        }
        if mods.contains(KeyModifiers::SHIFT) {
            f.write_str("shift+")?;
        }
        if mods.contains(KeyModifiers::SUPER) {
            f.write_str("super+")?;
        }
        if mods.contains(KeyModifiers::HYPER) {
            f.write_str("hyper+")?;
        }
        if mods.contains(KeyModifiers::META) {
            f.write_str("meta+")?;
        }

        match self.code {
            KeyCode::Char('+') => f.write_str("plus"),
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
            // Keypad
            KeyCode::KpEnter => f.write_str("kpenter"),
            KeyCode::KpAdd => f.write_str("kpadd"),
            KeyCode::KpSubtract => f.write_str("kpsubtract"),
            KeyCode::KpMultiply => f.write_str("kpmultiply"),
            KeyCode::KpDivide => f.write_str("kpdivide"),
            KeyCode::KpDecimal => f.write_str("kpdecimal"),
            KeyCode::KpEqual => f.write_str("kpequal"),
            KeyCode::KpSeparator => f.write_str("kpseparator"),
            KeyCode::KpLeft => f.write_str("kpleft"),
            KeyCode::KpRight => f.write_str("kpright"),
            KeyCode::KpUp => f.write_str("kpup"),
            KeyCode::KpDown => f.write_str("kpdown"),
            KeyCode::KpPageUp => f.write_str("kppgup"),
            KeyCode::KpPageDown => f.write_str("kppgdn"),
            KeyCode::KpHome => f.write_str("kphome"),
            KeyCode::KpEnd => f.write_str("kpend"),
            KeyCode::KpInsert => f.write_str("kpinsert"),
            KeyCode::KpDelete => f.write_str("kpdelete"),
            KeyCode::KpBegin => f.write_str("kpbegin"),
            KeyCode::Kp0 => f.write_str("kp0"),
            KeyCode::Kp1 => f.write_str("kp1"),
            KeyCode::Kp2 => f.write_str("kp2"),
            KeyCode::Kp3 => f.write_str("kp3"),
            KeyCode::Kp4 => f.write_str("kp4"),
            KeyCode::Kp5 => f.write_str("kp5"),
            KeyCode::Kp6 => f.write_str("kp6"),
            KeyCode::Kp7 => f.write_str("kp7"),
            KeyCode::Kp8 => f.write_str("kp8"),
            KeyCode::Kp9 => f.write_str("kp9"),
            // Media
            KeyCode::MediaPlay => f.write_str("mediaplay"),
            KeyCode::MediaPause => f.write_str("mediapause"),
            KeyCode::MediaPlayPause => f.write_str("mediaplaypause"),
            KeyCode::MediaReverse => f.write_str("mediareverse"),
            KeyCode::MediaStop => f.write_str("mediastop"),
            KeyCode::MediaRewind => f.write_str("mediarewind"),
            KeyCode::MediaFastForward => f.write_str("mediafastforward"),
            KeyCode::MediaNext => f.write_str("medianext"),
            KeyCode::MediaPrev => f.write_str("mediaprev"),
            KeyCode::MediaRecord => f.write_str("mediarecord"),
            KeyCode::VolumeUp => f.write_str("volumeup"),
            KeyCode::VolumeDown => f.write_str("volumedown"),
            KeyCode::VolumeMute => f.write_str("volumemute"),
            // Modifier keys
            KeyCode::LeftShift => f.write_str("leftshift"),
            KeyCode::RightShift => f.write_str("rightshift"),
            KeyCode::LeftCtrl => f.write_str("leftctrl"),
            KeyCode::RightCtrl => f.write_str("rightctrl"),
            KeyCode::LeftAlt => f.write_str("leftalt"),
            KeyCode::RightAlt => f.write_str("rightalt"),
            KeyCode::LeftSuper => f.write_str("leftsuper"),
            KeyCode::RightSuper => f.write_str("rightsuper"),
            KeyCode::LeftHyper => f.write_str("lefthyper"),
            KeyCode::RightHyper => f.write_str("righthyper"),
            KeyCode::LeftMeta => f.write_str("leftmeta"),
            KeyCode::RightMeta => f.write_str("rightmeta"),
            KeyCode::IsoLevel3Shift => f.write_str("isolevel3shift"),
            KeyCode::IsoLevel5Shift => f.write_str("isolevel5shift"),
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
    fn shifted_key_does_not_pollute_text_without_shift() {
        // A decoder may have pre-populated `shifted_key` from protocol
        // metadata (e.g. an alternate-key report) even on a bare key
        // press. `text` must reflect what the user actually typed,
        // which is the unshifted glyph, not the shifted one.
        let mut k = Key {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::empty(),
            text: None,
            shifted_key: Some('A'),
            base_key: None,
        };
        k.normalize();
        assert_eq!(k.text.as_deref(), Some("a"));
        // With Shift, the shifted glyph wins.
        let mut k = Key {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::SHIFT,
            text: None,
            shifted_key: Some('A'),
            base_key: None,
        };
        k.normalize();
        assert_eq!(k.text.as_deref(), Some("A"));
    }

    #[test]
    fn caps_lock_shifts_cased_letters_only() {
        // CapsLock does not produce the shifted glyph for digits or
        // symbols on any common layout. Even if a decoder populated
        // `shifted_key` for a non-letter key, `text` derived under
        // CapsLock alone must use the base glyph.
        let mut k = Key {
            code: KeyCode::Char('2'),
            modifiers: KeyModifiers::CAPS_LOCK,
            text: None,
            shifted_key: Some('@'),
            base_key: None,
        };
        k.normalize();
        assert_eq!(k.text.as_deref(), Some("2"));

        // ASCII letters treat CapsLock as a shifted layer.
        let mut k = Key {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::CAPS_LOCK,
            text: None,
            shifted_key: Some('A'),
            base_key: None,
        };
        k.normalize();
        assert_eq!(k.text.as_deref(), Some("A"));

        // Non-ASCII cased letters (e.g. Cyrillic, Greek, Latin
        // Extended) participate too — the rule is "any cased letter",
        // not "ASCII only".
        let mut k = Key {
            code: KeyCode::Char('ä'),
            modifiers: KeyModifiers::CAPS_LOCK,
            text: None,
            shifted_key: Some('Ä'),
            base_key: None,
        };
        k.normalize();
        assert_eq!(k.text.as_deref(), Some("Ä"));

        let mut k = Key {
            code: KeyCode::Char('д'),
            modifiers: KeyModifiers::CAPS_LOCK,
            text: None,
            shifted_key: Some('Д'),
            base_key: None,
        };
        k.normalize();
        assert_eq!(k.text.as_deref(), Some("Д"));

        // With Shift held, non-letters still take the shifted glyph.
        let mut k = Key {
            code: KeyCode::Char('2'),
            modifiers: KeyModifiers::SHIFT,
            text: None,
            shifted_key: Some('@'),
            base_key: None,
        };
        k.normalize();
        assert_eq!(k.text.as_deref(), Some("@"));
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

    #[test]
    fn eq_ignores_informational_fields() {
        let bare = Key::new(KeyCode::Char('a'), KeyModifiers::SHIFT);
        let mut decorated = Key::new(KeyCode::Char('a'), KeyModifiers::SHIFT);
        decorated.text = Some("custom".to_string());
        decorated.shifted_key = Some('Z');
        decorated.base_key = Some('q');
        assert_eq!(bare, decorated);
    }

    #[test]
    fn hash_ignores_informational_fields() {
        use std::collections::HashMap;
        let mut map: HashMap<Key, &'static str> = HashMap::new();
        map.insert(Key::new(KeyCode::Char('a'), KeyModifiers::CTRL), "ctrl-a");

        let mut lookup = Key::new(KeyCode::Char('a'), KeyModifiers::CTRL);
        lookup.text = Some("ignored".to_string());
        lookup.shifted_key = Some('X');
        assert_eq!(map.get(&lookup), Some(&"ctrl-a"));
    }

    #[test]
    fn eq_distinguishes_code_and_modifiers() {
        let a = Key::new(KeyCode::Char('a'), KeyModifiers::empty());
        let ctrl_a = Key::new(KeyCode::Char('a'), KeyModifiers::CTRL);
        let b = Key::new(KeyCode::Char('b'), KeyModifiers::empty());
        assert_ne!(a, ctrl_a);
        assert_ne!(a, b);
    }

    #[test]
    fn display_canonical_mod_order() {
        // Construct with all six binding modifiers; expect canonical
        // ctrl, alt, shift, super, hyper, meta order regardless of
        // input insertion.
        let mods = KeyModifiers::META
            | KeyModifiers::HYPER
            | KeyModifiers::SUPER
            | KeyModifiers::SHIFT
            | KeyModifiers::ALT
            | KeyModifiers::CTRL;
        let k = Key {
            code: KeyCode::Char('a'),
            modifiers: mods,
            text: None,
            shifted_key: None,
            base_key: None,
        };
        assert_eq!(k.to_string(), "ctrl+alt+shift+super+hyper+meta+a");
    }

    #[test]
    fn display_omits_lock_state() {
        let k = Key {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::CAPS_LOCK | KeyModifiers::NUM_LOCK | KeyModifiers::SCROLL_LOCK,
            text: None,
            shifted_key: None,
            base_key: None,
        };
        assert_eq!(k.to_string(), "a");
    }

    #[test]
    fn display_lock_state_combined_with_binding_mod() {
        // Real input: Ctrl+a while CapsLock is on. Caps drops from
        // Display so the binding string stays stable.
        let k = Key {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::CTRL | KeyModifiers::CAPS_LOCK,
            text: None,
            shifted_key: None,
            base_key: None,
        };
        assert_eq!(k.to_string(), "ctrl+a");
    }

    #[test]
    fn display_char_plus_uses_word() {
        let k = Key::new(KeyCode::Char('+'), KeyModifiers::CTRL);
        assert_eq!(k.to_string(), "ctrl+plus");
    }

    #[test]
    fn display_modifier_key_variants() {
        let k = Key::new(KeyCode::LeftShift, KeyModifiers::empty());
        assert_eq!(k.to_string(), "leftshift");
        let k = Key::new(KeyCode::RightAlt, KeyModifiers::empty());
        assert_eq!(k.to_string(), "rightalt");
        let k = Key::new(KeyCode::IsoLevel3Shift, KeyModifiers::empty());
        assert_eq!(k.to_string(), "isolevel3shift");
    }

    #[test]
    fn display_keypad_variants() {
        assert_eq!(
            Key::new(KeyCode::Kp0, KeyModifiers::empty()).to_string(),
            "kp0"
        );
        assert_eq!(
            Key::new(KeyCode::KpEnter, KeyModifiers::empty()).to_string(),
            "kpenter"
        );
        assert_eq!(
            Key::new(KeyCode::KpPageUp, KeyModifiers::empty()).to_string(),
            "kppgup"
        );
    }

    #[test]
    fn display_media_variants() {
        assert_eq!(
            Key::new(KeyCode::MediaPlayPause, KeyModifiers::empty()).to_string(),
            "mediaplaypause"
        );
        assert_eq!(
            Key::new(KeyCode::VolumeMute, KeyModifiers::empty()).to_string(),
            "volumemute"
        );
    }
}
