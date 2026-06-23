//! Key payloads, modifiers, display forms, and binding parsing.
//!
//! ## Purpose
//!
//! This module defines the key identity used by [`Event::KeyPress`],
//! [`Event::KeyRepeat`], and [`Event::KeyRelease`]. It normalizes the many wire
//! encodings a terminal may use into a stable [`Key`] value suitable for
//! equality, hashing, display, and string-based shortcut matching.
//!
//! ```text
//! terminal encoding ──▶ KeyCode + KeyModifiers ──▶ normalize ──▶ Key
//!       │                         │
//!       │                         └─ lock-state bits are informational
//!       └─ optional text / shifted / base glyph metadata
//! ```
//!
//! ## Key types
//!
//! * [`KeyCode`] identifies the logical key: a character, navigation key,
//!   function key, keypad key, media key, or modifier key.
//! * [`KeyModifiers`] separates binding modifiers (`Ctrl`, `Alt`, `Shift`,
//!   `Super`, `Hyper`, `Meta`) from lock states.
//! * [`Key`] combines a code and modifiers with optional text metadata supplied
//!   by richer keyboard protocols.
//! * [`ParseKeyError`] reports why a user-facing binding string did not parse.
//!
//! ## Matching and display
//!
//! [`Key`] equality and hashing use only the code plus binding modifiers. Text,
//! alternate-key metadata, and lock-state bits are ignored so bindings are
//! stable across terminal protocols and keyboard latch states. [`Key::matches`]
//! first checks exact produced text, then falls back to parsing the pattern as a
//! key string such as `"ctrl+c"`, `"shift+f1"`, or `"alt+plus"`.
//!
//! ## Gotchas
//!
//! Uppercase character codes normalize to lowercase plus [`KeyModifiers::SHIFT`]
//! unless [`KeyModifiers::CAPS_LOCK`] explains the case. Bare legacy encodings
//! cannot always recover the physical base key for shifted symbols or
//! Ctrl+Shift letters; richer encodings may provide [`Key::text`],
//! [`Key::shifted_key`], or [`Key::base_key`] for that extra context.
use bitflags::bitflags;
use std::fmt;

bitflags! {
    /// Keyboard modifier flags.
    ///
    /// Modifiers split into two categories:
    ///
    /// * **Binding modifiers** — `SHIFT`, `ALT`, `CTRL`, `META`,
    ///   `HYPER`, `SUPER`. These participate in [`Key`] equality and
    ///   in [`Key::matches`].
    /// * **Lock states** — `CAPS_LOCK`, `NUM_LOCK`, `SCROLL_LOCK`,
    ///   collectively [`LOCK_MASK`](Self::LOCK_MASK). These report
    ///   the keyboard latch and are *never* binding modifiers: they
    ///   are ignored by `Key` equality, hashing, [`Key::matches`],
    ///   and the [`Display`](fmt::Display) form. Hosts that need to
    ///   inspect lock state can mask `modifiers` with `LOCK_MASK`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct KeyModifiers: u16 {
        /// Shift key modifier.
        const SHIFT       = 0b0000_0000_0000_0001;
        /// Alt key modifier.
        const ALT         = 0b0000_0000_0000_0010;
        /// Control key modifier.
        const CTRL        = 0b0000_0000_0000_0100;
        /// Meta key modifier.
        const META        = 0b0000_0000_0000_1000;
        /// Hyper key modifier.
        const HYPER       = 0b0000_0000_0001_0000;
        /// Super key modifier.
        const SUPER       = 0b0000_0000_0010_0000;
        /// Caps Lock state.
        const CAPS_LOCK   = 0b0000_0000_0100_0000;
        /// Num Lock state.
        const NUM_LOCK    = 0b0000_0000_1000_0000;
        /// Scroll Lock state.
        const SCROLL_LOCK = 0b0000_0001_0000_0000;

        /// Mask of lock-state bits (`CAPS_LOCK | NUM_LOCK | SCROLL_LOCK`).
        ///
        /// Lock states report the current keyboard latch and are *not*
        /// binding modifiers: they are ignored by [`Key`] equality,
        /// hashing, [`Key::matches`], and [`Display`](fmt::Display).
        /// Callers that need to inspect or compare lock state can mask
        /// the modifier set with this constant.
        const LOCK_MASK   = Self::CAPS_LOCK.bits()
                          | Self::NUM_LOCK.bits()
                          | Self::SCROLL_LOCK.bits();
    }
}

/// A key event.
///
/// Equality and hashing intentionally consider only [`code`](Self::code)
/// and the *binding* portion of [`modifiers`](Self::modifiers): two
/// keys representing the same press compare equal regardless of
/// whether informational fields ([`text`](Self::text),
/// [`shifted_key`](Self::shifted_key), [`base_key`](Self::base_key))
/// were populated by the producing decoder, and regardless of lock
/// state ([`CAPS_LOCK`](KeyModifiers::CAPS_LOCK),
/// [`NUM_LOCK`](KeyModifiers::NUM_LOCK),
/// [`SCROLL_LOCK`](KeyModifiers::SCROLL_LOCK)). This makes `Key` safe
/// to use as a `HashMap` key for binding lookups across decoder paths
/// and across keyboard latch states.
///
/// For string-based binding matching that additionally folds letter
/// case and consults [`text`](Self::text) for layout-specific glyphs,
/// see [`matches`](Self::matches) and [`matches_any`](Self::matches_any).
#[derive(Debug, Clone)]
pub struct Key {
    /// The key code (which key was pressed).
    pub code: KeyCode,
    /// Active modifiers.
    pub modifiers: KeyModifiers,
    /// Associated text (from Kitty protocol or composed input).
    ///
    /// Informational; ignored by `==` and `Hash`. Consulted by
    /// [`Key::matches`] when it carries a layout-specific glyph that
    /// differs from `code` (for example `"!"` for `shift+1` on a US
    /// layout), enabling layout-independent string bindings.
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
        self.code == other.code
            && self.modifiers.difference(KeyModifiers::LOCK_MASK)
                == other.modifiers.difference(KeyModifiers::LOCK_MASK)
    }
}

impl Eq for Key {}

impl std::hash::Hash for Key {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.code.hash(state);
        self.modifiers
            .difference(KeyModifiers::LOCK_MASK)
            .hash(state);
    }
}

impl Key {
    /// Construct a [`Key`] with the given code and modifiers.
    ///
    /// This is a transparent constructor: no canonicalization is
    /// performed. Optional fields (`text`, `shifted_key`, `base_key`)
    /// start empty. Decoder paths populate the optional fields as
    /// needed and then chain [`Key::normalized`] (or call
    /// [`Key::normalize`] in place) to apply the canonical identity
    /// rules (case folding, printable text auto-population).
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self {
            code,
            modifiers,
            text: None,
            shifted_key: None,
            base_key: None,
        }
    }

    /// Consume `self` and return the canonical form. The recommended
    /// way to build a canonical key in expression position
    /// (`Key::new(code, mods).normalized()`); see [`Key::normalize`]
    /// for the in-place equivalent and the full list of rules.
    pub fn normalized(mut self) -> Self {
        self.normalize();
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

    /// Test whether this key matches a string pattern.
    ///
    /// Matching is two-tier:
    ///
    /// 1. If this key carries a [`text`](Self::text) value and it
    ///    equals `pattern` byte-for-byte, the match succeeds. Lets
    ///    bindings be written as the produced glyph (`"?"`, `"!"`,
    ///    `"@"`) and hit any keystroke that resolves to that text,
    ///    independent of keyboard layout. Note that `text` reflects
    ///    the user-perceived character, so `CapsLock` *can* shift
    ///    the matched form (pressing `g` with CapsLock on yields
    ///    `text == "G"` and matches the pattern `"G"`).
    /// 2. Otherwise, `pattern` is parsed as a [`Key`] with the same
    ///    grammar as [`FromStr`](std::str::FromStr) (for example
    ///    `"ctrl+c"`, `"shift+f1"`, `"alt+plus"`) and compared with
    ///    [`PartialEq`]. Lock state (`CapsLock`, `NumLock`,
    ///    `ScrollLock`) is ignored by `==`, so step 2 is layout-
    ///    sensitive but lock-insensitive. Matching is otherwise
    ///    case-sensitive — `"g"` and `"G"` are distinct (`"G"` is a
    ///    synonym for `"shift+g"`).
    ///
    /// Returns `false` for any pattern that fails to parse and has
    /// no matching `text`; invalid patterns never panic.
    ///
    /// # Examples
    ///
    /// ```
    /// use uncurses::event::{Key, KeyCode, KeyModifiers};
    ///
    /// // Case-sensitive: g and G are distinct (vim-style).
    /// let plain_g = Key::new(KeyCode::Char('g'), KeyModifiers::empty()).normalized();
    /// let big_g   = Key::new(KeyCode::Char('G'), KeyModifiers::empty()).normalized();
    /// assert!(plain_g.matches("g"));
    /// assert!(!plain_g.matches("G"));
    /// assert!(big_g.matches("G"));
    /// assert!(big_g.matches("shift+g")); // "G" and "shift+g" are synonyms.
    /// assert!(!big_g.matches("g"));
    /// ```
    pub fn matches(&self, pattern: &str) -> bool {
        if let Some(text) = self.text.as_deref()
            && text == pattern
        {
            return true;
        }
        pattern.parse::<Key>().is_ok_and(|p| *self == p)
    }

    /// Test whether this key matches any pattern in the iterator.
    ///
    /// Equivalent to calling [`matches`](Self::matches) for each
    /// pattern and stopping at the first hit. Accepts any iterable
    /// yielding string-like items, including arrays, slices, and
    /// vectors.
    ///
    /// # Examples
    ///
    /// ```
    /// use uncurses::event::{Key, KeyCode, KeyModifiers};
    ///
    /// let key = Key::new(KeyCode::Char('c'), KeyModifiers::CTRL).normalized();
    /// assert!(key.matches_any(["esc", "ctrl+c", "q"]));
    /// assert!(!key.matches_any(["esc", "q"]));
    /// ```
    pub fn matches_any<I, S>(&self, patterns: I) -> bool
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        patterns.into_iter().any(|p| self.matches(p.as_ref()))
    }

    /// Apply the canonical identity rules in place. Idempotent.
    ///
    /// Decoder paths construct keys via [`Key::new`], populate optional
    /// fields, and then call this method (or chain
    /// [`Key::normalized`]) to produce the canonical form. The rules
    /// applied are:
    ///
    /// - Case folding for printable `Char` codes: uppercase chars are
    ///   lowercased and the original is stored in `shifted_key`;
    ///   `SHIFT` is added only when `CAPS_LOCK` is not set (with
    ///   CapsLock the case change is attributed to the lock state, not
    ///   a Shift press).
    /// - Printable text auto-population when `text` is empty and the
    ///   modifier set is a subset of `SHIFT | CAPS_LOCK | NUM_LOCK`.
    pub fn normalize(&mut self) {
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

/// Logical identity of a key, before modifiers are considered.
///
/// `KeyCode` is intentionally broader than printable text: it covers named
/// navigation/editing keys, keypad keys, media keys, and the richer modifier-key
/// identities reported by modern keyboard protocols. Combine it with
/// [`KeyModifiers`] in a [`Key`] to represent a full key event.
///
/// Use [`KeyCode::function`] when constructing function keys from untrusted
/// numeric input; the raw [`KeyCode::F`] variant is public for pattern matching
/// and decoder construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    /// A Unicode character key.
    ///
    /// Printable spaces are normally represented as [`KeyCode::Space`] for
    /// cross-decoder stability. Other printable and non-control Unicode scalar
    /// values use this variant after [`Key::normalize`] applies case folding.
    Char(char),
    /// Function key. Valid range is `1..=35` (xterm goes to F20,
    /// kitty extends to F35). Construct via [`KeyCode::function`] for
    /// a checked builder; the bare variant is `pub` so decoders can
    /// emit literals, but downstream code matching on `F(n)` should
    /// treat values outside the valid range as bugs.
    F(u8),
    /// Legacy VT220 Find key, distinct from Home. Only emitted when the
    /// decoder is configured to recognize the legacy Find key.
    Find,
    /// Legacy VT220 Select key, distinct from End. Only emitted when the
    /// decoder is configured to recognize the legacy Select key.
    Select,
    // Navigation
    /// Up arrow key.
    Up,
    /// Down arrow key.
    Down,
    /// Left arrow key.
    Left,
    /// Right arrow key.
    Right,
    /// Home key.
    Home,
    /// End key.
    End,
    /// Page Up key.
    PageUp,
    /// Page Down key.
    PageDown,
    // Editing
    /// Backspace key.
    Backspace,
    /// Delete key.
    Delete,
    /// Insert key.
    Insert,
    /// Tab key.
    Tab,
    /// Enter key.
    Enter,
    // Whitespace
    /// Space key.
    Space,
    // Special
    /// Escape key.
    Escape,
    /// Caps Lock key.
    CapsLock,
    /// Scroll Lock key.
    ScrollLock,
    /// Num Lock key.
    NumLock,
    /// Print Screen key.
    PrintScreen,
    /// Pause key.
    Pause,
    /// Menu key.
    Menu,
    // Keypad
    /// Keypad Enter key.
    KpEnter,
    /// Keypad Add key.
    KpAdd,
    /// Keypad Subtract key.
    KpSubtract,
    /// Keypad Multiply key.
    KpMultiply,
    /// Keypad Divide key.
    KpDivide,
    /// Keypad Decimal key.
    KpDecimal,
    /// Keypad Equal key.
    KpEqual,
    /// Keypad Separator key.
    KpSeparator,
    /// Keypad Left key.
    KpLeft,
    /// Keypad Right key.
    KpRight,
    /// Keypad Up key.
    KpUp,
    /// Keypad Down key.
    KpDown,
    /// Keypad Page Up key.
    KpPageUp,
    /// Keypad Page Down key.
    KpPageDown,
    /// Keypad Home key.
    KpHome,
    /// Keypad End key.
    KpEnd,
    /// Keypad Insert key.
    KpInsert,
    /// Keypad Delete key.
    KpDelete,
    /// Keypad Begin key.
    KpBegin,
    /// Keypad 0 key.
    Kp0,
    /// Keypad 1 key.
    Kp1,
    /// Keypad 2 key.
    Kp2,
    /// Keypad 3 key.
    Kp3,
    /// Keypad 4 key.
    Kp4,
    /// Keypad 5 key.
    Kp5,
    /// Keypad 6 key.
    Kp6,
    /// Keypad 7 key.
    Kp7,
    /// Keypad 8 key.
    Kp8,
    /// Keypad 9 key.
    Kp9,
    // Media
    /// Media Play key.
    MediaPlay,
    /// Media Pause key.
    MediaPause,
    /// Media Play/Pause key.
    MediaPlayPause,
    /// Media Reverse key.
    MediaReverse,
    /// Media Stop key.
    MediaStop,
    /// Media Rewind key.
    MediaRewind,
    /// Media Fast Forward key.
    MediaFastForward,
    /// Media Next key.
    MediaNext,
    /// Media Previous key.
    MediaPrev,
    /// Media Record key.
    MediaRecord,
    /// Volume Up key.
    VolumeUp,
    /// Volume Down key.
    VolumeDown,
    /// Volume Mute key.
    VolumeMute,
    // Modifier keys (reported with Kitty protocol)
    /// Left Shift key.
    LeftShift,
    /// Right Shift key.
    RightShift,
    /// Left Control key.
    LeftCtrl,
    /// Right Control key.
    RightCtrl,
    /// Left Alt key.
    LeftAlt,
    /// Right Alt key.
    RightAlt,
    /// Left Super key.
    LeftSuper,
    /// Right Super key.
    RightSuper,
    /// Left Hyper key.
    LeftHyper,
    /// Right Hyper key.
    RightHyper,
    /// Left Meta key.
    LeftMeta,
    /// Right Meta key.
    RightMeta,
    /// ISO Level 3 Shift (typically AltGr).
    IsoLevel3Shift,
    /// ISO Level 5 Shift.
    IsoLevel5Shift,
}

impl KeyCode {
    /// Highest valid function-key index accepted by [`KeyCode::function`].
    ///
    /// Function keys are represented as `F(1)` through `F(35)`. Values outside
    /// that range are not produced by the checked constructor.
    pub const FUNCTION_KEY_MAX: u8 = 35;

    /// Construct a checked function-key code.
    ///
    /// Returns `Some(KeyCode::F(n))` when `n` is in
    /// `1..=KeyCode::FUNCTION_KEY_MAX`; returns `None` for `0` or values above
    /// the supported range. This function is useful when decoding or accepting
    /// user input that may contain an invalid function-key number. It never
    /// panics.
    pub fn function(n: u8) -> Option<KeyCode> {
        if (1..=Self::FUNCTION_KEY_MAX).contains(&n) {
            Some(KeyCode::F(n))
        } else {
            None
        }
    }
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
            KeyCode::Char('-') => f.write_str("minus"),
            KeyCode::Char('=') => f.write_str("equals"),
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
            KeyCode::PageUp => f.write_str("pageup"),
            KeyCode::PageDown => f.write_str("pagedown"),
            KeyCode::Backspace => f.write_str("backspace"),
            KeyCode::Delete => f.write_str("delete"),
            KeyCode::Insert => f.write_str("insert"),
            KeyCode::Tab => f.write_str("tab"),
            KeyCode::Enter => f.write_str("enter"),
            KeyCode::Space => f.write_str("space"),
            KeyCode::Escape => f.write_str("escape"),
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
            KeyCode::KpPageUp => f.write_str("kppageup"),
            KeyCode::KpPageDown => f.write_str("kppagedown"),
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

/// Error produced when parsing a [`Key`] or [`KeyCode`] from a binding string.
///
/// Parsing is used by [`Key::matches`], [`std::str::FromStr`] for [`Key`], and
/// [`std::str::FromStr`] for [`KeyCode`]. The variants retain the offending
/// token where useful so configuration UIs can report actionable messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseKeyError {
    /// Input was empty or whitespace-only.
    Empty,
    /// A `+`-separated component was empty.
    ///
    /// Examples include `"ctrl++a"`, `"ctrl+"`, and strings with a leading
    /// separator such as `"+a"`.
    EmptyComponent,
    /// A modifier token was not recognized.
    ///
    /// The contained string is the exact modifier component that failed to
    /// parse, before any case normalization beyond comparison.
    UnknownModifier(String),
    /// The terminal key token was not recognized.
    ///
    /// The contained string is the key component after modifiers have been
    /// split off. Single-character tokens are accepted as [`KeyCode::Char`], so
    /// this usually indicates an unknown named key.
    UnknownKey(String),
    /// A function-key token (`f<n>`) had an out-of-range index.
    ///
    /// Valid function keys are `f1` through `f35`, matching
    /// [`KeyCode::FUNCTION_KEY_MAX`].
    InvalidFunctionKey(String),
}

impl fmt::Display for ParseKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("empty key string"),
            Self::EmptyComponent => f.write_str("empty `+`-separated component"),
            Self::UnknownModifier(s) => write!(f, "unknown modifier: {s:?}"),
            Self::UnknownKey(s) => write!(f, "unknown key name: {s:?}"),
            Self::InvalidFunctionKey(s) => write!(f, "invalid function key: {s:?}"),
        }
    }
}

impl std::error::Error for ParseKeyError {}

fn parse_modifier(token: &str) -> Result<KeyModifiers, ParseKeyError> {
    let mods = if token.eq_ignore_ascii_case("ctrl") || token.eq_ignore_ascii_case("control") {
        KeyModifiers::CTRL
    } else if token.eq_ignore_ascii_case("alt")
        || token.eq_ignore_ascii_case("opt")
        || token.eq_ignore_ascii_case("option")
    {
        KeyModifiers::ALT
    } else if token.eq_ignore_ascii_case("shift") {
        KeyModifiers::SHIFT
    } else if token.eq_ignore_ascii_case("super")
        || token.eq_ignore_ascii_case("win")
        || token.eq_ignore_ascii_case("cmd")
        || token.eq_ignore_ascii_case("command")
    {
        KeyModifiers::SUPER
    } else if token.eq_ignore_ascii_case("hyper") {
        KeyModifiers::HYPER
    } else if token.eq_ignore_ascii_case("meta") {
        KeyModifiers::META
    } else {
        return Err(ParseKeyError::UnknownModifier(token.to_string()));
    };
    Ok(mods)
}

/// Maximum byte length of any recognized key-code keyword. The
/// longest spellings (`isolevel3shift`, `mediafastforward`) are 14
/// and 16 bytes respectively, so anything longer than this cap cannot
/// match a known keyword and parsing can bail out without further
/// work. The buffer in [`parse_key_code`] is sized to this value so
/// the ASCII-lowercase conversion stays on the stack.
const KEY_KEYWORD_MAX_LEN: usize = 16;

fn parse_key_code(token: &str) -> Result<KeyCode, ParseKeyError> {
    // Single character: treat as Char (case preserved so `Key::new`
    // can canonicalize uppercase into shift+lowercase). Includes `+`
    // — the modifier-separator handling in `Key::from_str` passes
    // the literal `+` through to this function as a single-char
    // token.
    let mut chars = token.chars();
    if let Some(first) = chars.next()
        && chars.next().is_none()
    {
        return Ok(KeyCode::Char(first));
    }

    // All recognized keywords are ASCII and fit in KEY_KEYWORD_MAX_LEN
    // bytes. Tokens longer than that cannot match — bail to the
    // unknown-key error without touching the allocator. Tokens
    // containing non-ASCII bytes also can't match a keyword; fall
    // through to the same error.
    if token.len() > KEY_KEYWORD_MAX_LEN || !token.is_ascii() {
        return Err(ParseKeyError::UnknownKey(token.to_string()));
    }
    let mut buf = [0u8; KEY_KEYWORD_MAX_LEN];
    for (i, b) in token.as_bytes().iter().enumerate() {
        buf[i] = b.to_ascii_lowercase();
    }
    let lower = std::str::from_utf8(&buf[..token.len()])
        .expect("ASCII lowercase of ASCII input is valid UTF-8");

    // Function key: f<n> where 1 <= n <= KeyCode::FUNCTION_KEY_MAX.
    if let Some(rest) = lower.strip_prefix('f')
        && !rest.is_empty()
        && rest.bytes().all(|c| c.is_ascii_digit())
    {
        let n: u8 = rest
            .parse()
            .map_err(|_| ParseKeyError::InvalidFunctionKey(token.to_string()))?;
        return KeyCode::function(n)
            .ok_or_else(|| ParseKeyError::InvalidFunctionKey(token.to_string()));
    }

    Ok(match lower {
        // Navigation
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "find" => KeyCode::Find,
        "select" => KeyCode::Select,
        "pgup" | "pageup" => KeyCode::PageUp,
        "pgdn" | "pgdown" | "pagedown" => KeyCode::PageDown,
        // Editing
        "backspace" | "bs" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "tab" => KeyCode::Tab,
        "enter" | "return" | "ret" => KeyCode::Enter,
        // Whitespace / special
        "space" | "spc" => KeyCode::Space,
        "esc" | "escape" => KeyCode::Escape,
        "capslock" => KeyCode::CapsLock,
        "scrolllock" => KeyCode::ScrollLock,
        "numlock" => KeyCode::NumLock,
        "printscreen" | "prtsc" => KeyCode::PrintScreen,
        "pause" => KeyCode::Pause,
        "menu" => KeyCode::Menu,
        // Punctuation aliases. `plus`, `minus`, and `equals` are the
        // Display forms for `Char('+')`, `Char('-')`, and `Char('=')`
        // respectively; the literal characters are also accepted via
        // the single-character path. `dash` / `hyphen` and `equal` are
        // additional spellings for the same two keys.
        "plus" => KeyCode::Char('+'),
        "minus" | "dash" | "hyphen" => KeyCode::Char('-'),
        "equals" | "equal" => KeyCode::Char('='),
        // Keypad
        "kpenter" => KeyCode::KpEnter,
        "kpadd" => KeyCode::KpAdd,
        "kpsubtract" => KeyCode::KpSubtract,
        "kpmultiply" => KeyCode::KpMultiply,
        "kpdivide" => KeyCode::KpDivide,
        "kpdecimal" => KeyCode::KpDecimal,
        "kpequal" => KeyCode::KpEqual,
        "kpseparator" => KeyCode::KpSeparator,
        "kpleft" => KeyCode::KpLeft,
        "kpright" => KeyCode::KpRight,
        "kpup" => KeyCode::KpUp,
        "kpdown" => KeyCode::KpDown,
        "kppgup" | "kppageup" => KeyCode::KpPageUp,
        "kppgdn" | "kppgdown" | "kppagedown" => KeyCode::KpPageDown,
        "kphome" => KeyCode::KpHome,
        "kpend" => KeyCode::KpEnd,
        "kpinsert" => KeyCode::KpInsert,
        "kpdelete" => KeyCode::KpDelete,
        "kpbegin" => KeyCode::KpBegin,
        "kp0" => KeyCode::Kp0,
        "kp1" => KeyCode::Kp1,
        "kp2" => KeyCode::Kp2,
        "kp3" => KeyCode::Kp3,
        "kp4" => KeyCode::Kp4,
        "kp5" => KeyCode::Kp5,
        "kp6" => KeyCode::Kp6,
        "kp7" => KeyCode::Kp7,
        "kp8" => KeyCode::Kp8,
        "kp9" => KeyCode::Kp9,
        // Media
        "mediaplay" => KeyCode::MediaPlay,
        "mediapause" => KeyCode::MediaPause,
        "mediaplaypause" => KeyCode::MediaPlayPause,
        "mediareverse" => KeyCode::MediaReverse,
        "mediastop" => KeyCode::MediaStop,
        "mediarewind" => KeyCode::MediaRewind,
        "mediafastforward" => KeyCode::MediaFastForward,
        "medianext" => KeyCode::MediaNext,
        "mediaprev" => KeyCode::MediaPrev,
        "mediarecord" => KeyCode::MediaRecord,
        "volumeup" => KeyCode::VolumeUp,
        "volumedown" => KeyCode::VolumeDown,
        "volumemute" => KeyCode::VolumeMute,
        // Modifier keys
        "leftshift" => KeyCode::LeftShift,
        "rightshift" => KeyCode::RightShift,
        "leftctrl" => KeyCode::LeftCtrl,
        "rightctrl" => KeyCode::RightCtrl,
        "leftalt" => KeyCode::LeftAlt,
        "rightalt" => KeyCode::RightAlt,
        "leftsuper" => KeyCode::LeftSuper,
        "rightsuper" => KeyCode::RightSuper,
        "lefthyper" => KeyCode::LeftHyper,
        "righthyper" => KeyCode::RightHyper,
        "leftmeta" => KeyCode::LeftMeta,
        "rightmeta" => KeyCode::RightMeta,
        "isolevel3shift" => KeyCode::IsoLevel3Shift,
        "isolevel5shift" => KeyCode::IsoLevel5Shift,
        _ => return Err(ParseKeyError::UnknownKey(token.to_string())),
    })
}

impl std::str::FromStr for Key {
    type Err = ParseKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(ParseKeyError::Empty);
        }

        // Single-character input: treat as a literal key with no
        // modifiers. This lets `+`, `-`, and any other single character
        // round-trip cleanly without being misread as a `+`-separator.
        if s.chars().nth(1).is_none() {
            let code = parse_key_code(s)?;
            return Ok(Key::new(code, KeyModifiers::empty()).normalized());
        }

        // Split on the last `+` to separate modifiers from the key.
        // The literal `+` key is spelled `plus` in Display output, but
        // forms like `ctrl++` are also accepted: a trailing `++` means
        // "the `+` key, with the preceding `+` as separator". A single
        // trailing `+` with no preceding `+` is a dangling separator.
        // A leading `+` (e.g. `"+a"`) is also a dangling separator
        // since there is no modifier before it.
        let (mod_part, key_part) = if let Some(head) = s.strip_suffix('+') {
            match head.strip_suffix('+') {
                Some(mods) => (mods, "+"),
                None => return Err(ParseKeyError::EmptyComponent),
            }
        } else {
            match s.rsplit_once('+') {
                Some(("", _)) => return Err(ParseKeyError::EmptyComponent),
                Some(parts) => parts,
                None => ("", s),
            }
        };

        if key_part.is_empty() {
            return Err(ParseKeyError::EmptyComponent);
        }

        let mut modifiers = KeyModifiers::empty();
        if !mod_part.is_empty() {
            for tok in mod_part.split('+') {
                if tok.is_empty() {
                    return Err(ParseKeyError::EmptyComponent);
                }
                modifiers |= parse_modifier(tok)?;
            }
        }

        // `backtab` is a legacy spelling for `shift+tab`. Accept it as
        // a key-part alias (also when combined with other modifiers,
        // e.g. `alt+backtab`) even though there is no
        // `KeyCode::BackTab` variant; the canonical form is the
        // uniform `Tab + SHIFT`.
        if key_part.eq_ignore_ascii_case("backtab") {
            return Ok(Key::new(KeyCode::Tab, modifiers | KeyModifiers::SHIFT).normalized());
        }

        let code = parse_key_code(key_part)?;
        Ok(Key::new(code, modifiers).normalized())
    }
}

impl std::str::FromStr for KeyCode {
    type Err = ParseKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(ParseKeyError::Empty);
        }
        parse_key_code(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_display() {
        let k = Key::new(KeyCode::Char('a'), KeyModifiers::CTRL).normalized();
        assert_eq!(k.to_string(), "ctrl+a");
    }

    #[test]
    fn test_key_display_function() {
        let k = Key::new(KeyCode::F(12), KeyModifiers::empty()).normalized();
        assert_eq!(k.to_string(), "f12");
    }

    #[test]
    fn test_key_char() {
        let k = Key::new(KeyCode::Char('x'), KeyModifiers::empty()).normalized();
        assert_eq!(k.char(), Some('x'));
    }

    #[test]
    fn test_key_char_special() {
        let k = Key::new(KeyCode::Enter, KeyModifiers::empty()).normalized();
        assert_eq!(k.char(), None);
    }

    #[test]
    fn new_uppercase_ascii_lowers_code_and_adds_shift() {
        let k = Key::new(KeyCode::Char('A'), KeyModifiers::empty()).normalized();
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("A"));
    }

    #[test]
    fn new_uppercase_with_caps_lock_does_not_add_shift() {
        let k = Key::new(KeyCode::Char('A'), KeyModifiers::CAPS_LOCK).normalized();
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(!k.modifiers.contains(KeyModifiers::SHIFT));
        assert!(k.modifiers.contains(KeyModifiers::CAPS_LOCK));
        assert_eq!(k.text.as_deref(), Some("A"));
    }

    #[test]
    fn new_lowercase_with_shift_populates_shifted_key() {
        let k = Key::new(KeyCode::Char('a'), KeyModifiers::SHIFT).normalized();
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
        let k = Key::new(KeyCode::Char('a'), KeyModifiers::empty()).normalized();
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, None);
        assert!(k.modifiers.is_empty());
        assert_eq!(k.text.as_deref(), Some("a"));
    }

    #[test]
    fn new_ctrl_uppercase_does_not_set_text() {
        let k = Key::new(KeyCode::Char('A'), KeyModifiers::CTRL).normalized();
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(k.modifiers.contains(KeyModifiers::CTRL));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert!(k.text.is_none());
    }

    #[test]
    fn new_ctrl_shift_lowercase_does_not_set_text() {
        let k = Key::new(KeyCode::Char('a'), KeyModifiers::CTRL | KeyModifiers::SHIFT).normalized();
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(k.text.is_none());
    }

    #[test]
    fn new_cyrillic_uppercase() {
        let k = Key::new(KeyCode::Char('Ц'), KeyModifiers::empty()).normalized();
        assert_eq!(k.code, KeyCode::Char('ц'));
        assert_eq!(k.shifted_key, Some('Ц'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("Ц"));
    }

    #[test]
    fn new_greek_lowercase_with_shift() {
        let k = Key::new(KeyCode::Char('α'), KeyModifiers::SHIFT).normalized();
        assert_eq!(k.code, KeyCode::Char('α'));
        assert_eq!(k.shifted_key, Some('Α'));
        assert_eq!(k.text.as_deref(), Some("Α"));
    }

    #[test]
    fn new_multi_codepoint_lower_left_alone() {
        // 'İ' lowercases to "i\u{307}" — two codepoints; leave as-is.
        let k = Key::new(KeyCode::Char('İ'), KeyModifiers::empty()).normalized();
        assert_eq!(k.code, KeyCode::Char('İ'));
        assert_eq!(k.shifted_key, None);
        assert!(k.modifiers.is_empty());
        // Still printable input — text auto-populates with the original codepoint.
        assert_eq!(k.text.as_deref(), Some("İ"));
    }

    #[test]
    fn new_titlecase_digraph_left_alone() {
        // 'ǅ' is titlecase; is_uppercase() and is_lowercase() are both false.
        let k = Key::new(KeyCode::Char('ǅ'), KeyModifiers::empty()).normalized();
        assert_eq!(k.code, KeyCode::Char('ǅ'));
        assert_eq!(k.shifted_key, None);
        assert_eq!(k.text.as_deref(), Some("ǅ"));
    }

    #[test]
    fn new_digit_with_shift_keeps_digit_text() {
        // '1' has no case variant; text auto-populates from the codepoint.
        let k = Key::new(KeyCode::Char('1'), KeyModifiers::SHIFT).normalized();
        assert_eq!(k.code, KeyCode::Char('1'));
        assert_eq!(k.shifted_key, None);
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("1"));
    }

    #[test]
    fn new_non_char_no_text() {
        let k = Key::new(KeyCode::Enter, KeyModifiers::SHIFT).normalized();
        assert_eq!(k.code, KeyCode::Enter);
        assert_eq!(k.shifted_key, None);
        assert!(k.text.is_none());
    }

    #[test]
    fn new_space_populates_text() {
        let k = Key::new(KeyCode::Space, KeyModifiers::empty()).normalized();
        assert_eq!(k.code, KeyCode::Space);
        assert_eq!(k.text.as_deref(), Some(" "));
    }

    #[test]
    fn new_space_with_ctrl_no_text() {
        let k = Key::new(KeyCode::Space, KeyModifiers::CTRL).normalized();
        assert_eq!(k.code, KeyCode::Space);
        assert!(k.text.is_none());
    }

    #[test]
    fn direct_field_mutation_overrides_auto_text() {
        let mut k = Key::new(KeyCode::Char('2'), KeyModifiers::SHIFT).normalized();
        // Simulate a decoder that knows the terminal-reported shifted glyph.
        k.shifted_key = Some('@');
        k.text = Some("@".to_string());
        assert_eq!(k.text.as_deref(), Some("@"));
        assert_eq!(k.shifted_key, Some('@'));
    }

    #[test]
    fn eq_ignores_informational_fields() {
        let bare = Key::new(KeyCode::Char('a'), KeyModifiers::SHIFT).normalized();
        let mut decorated = Key::new(KeyCode::Char('a'), KeyModifiers::SHIFT).normalized();
        decorated.text = Some("custom".to_string());
        decorated.shifted_key = Some('Z');
        decorated.base_key = Some('q');
        assert_eq!(bare, decorated);
    }

    #[test]
    fn hash_ignores_informational_fields() {
        use std::collections::HashMap;
        let mut map: HashMap<Key, &'static str> = HashMap::new();
        map.insert(
            Key::new(KeyCode::Char('a'), KeyModifiers::CTRL).normalized(),
            "ctrl-a",
        );

        let mut lookup = Key::new(KeyCode::Char('a'), KeyModifiers::CTRL).normalized();
        lookup.text = Some("ignored".to_string());
        lookup.shifted_key = Some('X');
        assert_eq!(map.get(&lookup), Some(&"ctrl-a"));
    }

    #[test]
    fn eq_distinguishes_code_and_modifiers() {
        let a = Key::new(KeyCode::Char('a'), KeyModifiers::empty()).normalized();
        let ctrl_a = Key::new(KeyCode::Char('a'), KeyModifiers::CTRL).normalized();
        let b = Key::new(KeyCode::Char('b'), KeyModifiers::empty()).normalized();
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
        let k = Key::new(KeyCode::Char('+'), KeyModifiers::CTRL).normalized();
        assert_eq!(k.to_string(), "ctrl+plus");
    }

    #[test]
    fn display_modifier_key_variants() {
        let k = Key::new(KeyCode::LeftShift, KeyModifiers::empty()).normalized();
        assert_eq!(k.to_string(), "leftshift");
        let k = Key::new(KeyCode::RightAlt, KeyModifiers::empty()).normalized();
        assert_eq!(k.to_string(), "rightalt");
        let k = Key::new(KeyCode::IsoLevel3Shift, KeyModifiers::empty()).normalized();
        assert_eq!(k.to_string(), "isolevel3shift");
    }

    #[test]
    fn display_uses_full_words() {
        // Display always emits the unabbreviated name; the short forms
        // (`pgup`, `pgdn`, `esc`, etc.) live only on the parse side.
        assert_eq!(
            Key::new(KeyCode::PageUp, KeyModifiers::empty())
                .normalized()
                .to_string(),
            "pageup"
        );
        assert_eq!(
            Key::new(KeyCode::PageDown, KeyModifiers::empty())
                .normalized()
                .to_string(),
            "pagedown"
        );
        assert_eq!(
            Key::new(KeyCode::Escape, KeyModifiers::empty())
                .normalized()
                .to_string(),
            "escape"
        );
        // Round-trip the short forms through Display and back to the
        // same Key.
        assert_eq!(parse("pgup"), parse("pageup"));
        assert_eq!(parse("pgdn"), parse("pagedown"));
        assert_eq!(parse("pgdown"), parse("pagedown"));
        assert_eq!(parse("esc"), parse("escape"));
    }

    #[test]
    fn display_keypad_variants() {
        assert_eq!(
            Key::new(KeyCode::Kp0, KeyModifiers::empty())
                .normalized()
                .to_string(),
            "kp0"
        );
        assert_eq!(
            Key::new(KeyCode::KpEnter, KeyModifiers::empty())
                .normalized()
                .to_string(),
            "kpenter"
        );
        assert_eq!(
            Key::new(KeyCode::KpPageUp, KeyModifiers::empty())
                .normalized()
                .to_string(),
            "kppageup"
        );
    }

    #[test]
    fn display_media_variants() {
        assert_eq!(
            Key::new(KeyCode::MediaPlayPause, KeyModifiers::empty())
                .normalized()
                .to_string(),
            "mediaplaypause"
        );
        assert_eq!(
            Key::new(KeyCode::VolumeMute, KeyModifiers::empty())
                .normalized()
                .to_string(),
            "volumemute"
        );
    }

    // --- FromStr ------------------------------------------------------

    fn parse(s: &str) -> Key {
        s.parse::<Key>()
            .unwrap_or_else(|e| panic!("parse {s:?}: {e}"))
    }

    #[test]
    fn fromstr_single_char_lowercase() {
        let k = parse("a");
        assert_eq!(k.code, KeyCode::Char('a'));
        assert!(k.modifiers.is_empty());
    }

    #[test]
    fn fromstr_uppercase_char_canonicalizes_to_shift_lowercase() {
        // Reuse Key::new's canonicalization so callers writing
        // "A" or "shift+a" end up at the same identity.
        assert_eq!(
            "A".parse::<Key>().unwrap(),
            "shift+a".parse::<Key>().unwrap()
        );
    }

    #[test]
    fn fromstr_modifier_order_independent() {
        let canonical = parse("ctrl+shift+a");
        assert_eq!(parse("shift+ctrl+a"), canonical);
        assert_eq!(parse("Shift+Ctrl+a"), canonical);
    }

    #[test]
    fn fromstr_function_keys() {
        assert_eq!(parse("f1").code, KeyCode::F(1));
        assert_eq!(parse("f12").code, KeyCode::F(12));
        assert_eq!(parse("f24").code, KeyCode::F(24));
        assert_eq!(parse("f35").code, KeyCode::F(35));
    }

    #[test]
    fn fromstr_function_key_out_of_range() {
        assert!(matches!(
            "f0".parse::<Key>(),
            Err(ParseKeyError::InvalidFunctionKey(_))
        ));
        assert!(matches!(
            "f36".parse::<Key>(),
            Err(ParseKeyError::InvalidFunctionKey(_))
        ));
    }

    #[test]
    fn keycode_function_validates_range() {
        assert_eq!(KeyCode::function(1), Some(KeyCode::F(1)));
        assert_eq!(
            KeyCode::function(KeyCode::FUNCTION_KEY_MAX),
            Some(KeyCode::F(KeyCode::FUNCTION_KEY_MAX))
        );
        assert_eq!(KeyCode::function(0), None);
        assert_eq!(KeyCode::function(KeyCode::FUNCTION_KEY_MAX + 1), None);
    }

    #[test]
    fn fromstr_named_keys() {
        assert_eq!(parse("esc").code, KeyCode::Escape);
        assert_eq!(parse("escape").code, KeyCode::Escape);
        assert_eq!(parse("pgup").code, KeyCode::PageUp);
        assert_eq!(parse("pageup").code, KeyCode::PageUp);
        assert_eq!(parse("enter").code, KeyCode::Enter);
        assert_eq!(parse("return").code, KeyCode::Enter);
        assert_eq!(parse("ret").code, KeyCode::Enter);
        assert_eq!(parse("backspace").code, KeyCode::Backspace);
        assert_eq!(parse("bs").code, KeyCode::Backspace);
        // `backtab` is an input alias for `shift+tab`. There is no
        // dedicated `KeyCode::BackTab` variant. The alias also
        // composes with other modifiers.
        let backtab = parse("backtab");
        assert_eq!(backtab.code, KeyCode::Tab);
        assert_eq!(backtab.modifiers, KeyModifiers::SHIFT);
        assert_eq!(parse("shift+tab"), backtab);
        // Display formats this identity as `shift+tab`, never `backtab`.
        assert_eq!(backtab.to_string(), "shift+tab");
        let alt_backtab = parse("alt+backtab");
        assert_eq!(alt_backtab.code, KeyCode::Tab);
        assert_eq!(
            alt_backtab.modifiers,
            KeyModifiers::SHIFT | KeyModifiers::ALT
        );
        assert_eq!(parse("alt+shift+tab"), alt_backtab);
        assert_eq!(alt_backtab.to_string(), "alt+shift+tab");
        assert_eq!(parse("delete").code, KeyCode::Delete);
        assert_eq!(parse("del").code, KeyCode::Delete);
        assert_eq!(parse("space").code, KeyCode::Space);
    }

    #[test]
    fn fromstr_modifier_aliases() {
        assert_eq!(parse("control+a"), parse("ctrl+a"));
        assert_eq!(parse("option+a"), parse("alt+a"));
        assert_eq!(parse("cmd+a"), parse("super+a"));
        assert_eq!(parse("command+a"), parse("super+a"));
        assert_eq!(parse("win+a"), parse("super+a"));
    }

    #[test]
    fn fromstr_plus_alias_round_trips() {
        let k = parse("ctrl+plus");
        assert_eq!(k.code, KeyCode::Char('+'));
        assert!(k.modifiers.contains(KeyModifiers::CTRL));
        assert_eq!(k.to_string(), "ctrl+plus");
    }

    #[test]
    fn fromstr_keypad_and_media() {
        assert_eq!(parse("kp0").code, KeyCode::Kp0);
        assert_eq!(parse("kpenter").code, KeyCode::KpEnter);
        assert_eq!(parse("mediaplaypause").code, KeyCode::MediaPlayPause);
        assert_eq!(parse("volumemute").code, KeyCode::VolumeMute);
    }

    #[test]
    fn fromstr_modifier_keys_themselves() {
        assert_eq!(parse("leftshift").code, KeyCode::LeftShift);
        assert_eq!(parse("rightalt").code, KeyCode::RightAlt);
        assert_eq!(parse("isolevel3shift").code, KeyCode::IsoLevel3Shift);
    }

    #[test]
    fn fromstr_unicode_char() {
        assert_eq!(parse("ц").code, KeyCode::Char('ц'));
        assert_eq!(parse("α").code, KeyCode::Char('α'));
    }

    #[test]
    fn fromstr_trims_whitespace() {
        assert_eq!(parse("  ctrl+a  "), parse("ctrl+a"));
    }

    #[test]
    fn fromstr_errors() {
        assert_eq!("".parse::<Key>(), Err(ParseKeyError::Empty));
        assert_eq!("   ".parse::<Key>(), Err(ParseKeyError::Empty));
        assert_eq!("ctrl+".parse::<Key>(), Err(ParseKeyError::EmptyComponent));
        // `ctrl++a` still errors: the inner `+` produces an empty
        // modifier token. (`ctrl++` alone is the ctrl+literal-`+` form.)
        assert_eq!("ctrl++a".parse::<Key>(), Err(ParseKeyError::EmptyComponent));
        // Leading `+` is a dangling separator with no modifier before it.
        assert_eq!("+a".parse::<Key>(), Err(ParseKeyError::EmptyComponent));
        assert_eq!("+ctrl+a".parse::<Key>(), Err(ParseKeyError::EmptyComponent));
        assert!(matches!(
            "foo+a".parse::<Key>(),
            Err(ParseKeyError::UnknownModifier(_))
        ));
        assert!(matches!(
            "ctrl+xyz".parse::<Key>(),
            Err(ParseKeyError::UnknownKey(_))
        ));
    }

    #[test]
    fn fromstr_keycode_only() {
        let kc: KeyCode = "esc".parse().unwrap();
        assert_eq!(kc, KeyCode::Escape);
        let kc: KeyCode = "f5".parse().unwrap();
        assert_eq!(kc, KeyCode::F(5));
        assert!(matches!(
            "ctrl+a".parse::<KeyCode>(),
            Err(ParseKeyError::UnknownKey(_))
        ));
    }

    #[test]
    fn fromstr_literal_plus() {
        // Bare `+` is the literal key character (single-char shortcut).
        assert_eq!(parse("+").code, KeyCode::Char('+'));
        // `plus` alias parses identically.
        assert_eq!(parse("plus").code, KeyCode::Char('+'));
        // `ctrl++` is ctrl + the literal `+` key.
        assert_eq!(
            parse("ctrl++"),
            Key::new(KeyCode::Char('+'), KeyModifiers::CTRL).normalized()
        );
        // `ctrl+plus` resolves to the same Key.
        assert_eq!(parse("ctrl++"), parse("ctrl+plus"));
    }

    #[test]
    fn fromstr_literal_symbols() {
        // Symbol literals are accepted via the single-char path.
        assert_eq!(parse("-").code, KeyCode::Char('-'));
        assert_eq!(parse("*").code, KeyCode::Char('*'));
        assert_eq!(parse("/").code, KeyCode::Char('/'));
        assert_eq!(parse("=").code, KeyCode::Char('='));
        assert_eq!(parse("[").code, KeyCode::Char('['));
        assert_eq!(parse("ctrl+-").code, KeyCode::Char('-'));
        assert_eq!(parse("ctrl+/").code, KeyCode::Char('/'));
        assert_eq!(parse("alt+[").code, KeyCode::Char('['));
    }

    #[test]
    fn fromstr_minus_aliases() {
        assert_eq!(parse("minus").code, KeyCode::Char('-'));
        assert_eq!(parse("dash").code, KeyCode::Char('-'));
        assert_eq!(parse("hyphen").code, KeyCode::Char('-'));
        assert_eq!(parse("ctrl+minus"), parse("ctrl+-"));
        assert_eq!(parse("ctrl+dash"), parse("ctrl+-"));
    }

    #[test]
    fn fromstr_equals_aliases() {
        assert_eq!(parse("equals").code, KeyCode::Char('='));
        assert_eq!(parse("equal").code, KeyCode::Char('='));
        assert_eq!(parse("ctrl+equals"), parse("ctrl+="));
    }

    #[test]
    fn display_uses_named_symbol_forms() {
        assert_eq!(
            Key::new(KeyCode::Char('-'), KeyModifiers::empty())
                .normalized()
                .to_string(),
            "minus"
        );
        assert_eq!(
            Key::new(KeyCode::Char('='), KeyModifiers::empty())
                .normalized()
                .to_string(),
            "equals"
        );
        assert_eq!(
            Key::new(KeyCode::Char('+'), KeyModifiers::empty())
                .normalized()
                .to_string(),
            "plus"
        );
        assert_eq!(
            Key::new(KeyCode::Char('-'), KeyModifiers::CTRL)
                .normalized()
                .to_string(),
            "ctrl+minus"
        );
    }

    #[test]
    fn fromstr_rejects_hyphen_aliases() {
        // Hyphenated key-name aliases are intentionally not accepted;
        // `+` is the only valid binding separator.
        assert!(matches!(
            "page-up".parse::<Key>(),
            Err(ParseKeyError::UnknownKey(_))
        ));
        assert!(matches!(
            "back-tab".parse::<Key>(),
            Err(ParseKeyError::UnknownKey(_))
        ));
        assert!(matches!(
            "caps-lock".parse::<Key>(),
            Err(ParseKeyError::UnknownKey(_))
        ));
    }

    #[test]
    fn display_kp_pageup_long_form() {
        assert_eq!(
            Key::new(KeyCode::KpPageDown, KeyModifiers::empty())
                .normalized()
                .to_string(),
            "kppagedown"
        );
        // The short `kppgup`/`kppgdn` forms remain accepted on input.
        assert_eq!(parse("kppgup").code, KeyCode::KpPageUp);
        assert_eq!(parse("kppgdn").code, KeyCode::KpPageDown);
    }

    #[test]
    fn display_fromstr_roundtrip_named_variants() {
        // Every variant emitted by Display must round-trip back to an
        // equal Key.
        let cases: &[(KeyCode, KeyModifiers)] = &[
            (KeyCode::Char('a'), KeyModifiers::empty()),
            (KeyCode::Char('a'), KeyModifiers::CTRL),
            (KeyCode::Char('a'), KeyModifiers::CTRL | KeyModifiers::ALT),
            (
                KeyCode::Char('a'),
                KeyModifiers::CTRL
                    | KeyModifiers::ALT
                    | KeyModifiers::SHIFT
                    | KeyModifiers::SUPER
                    | KeyModifiers::HYPER
                    | KeyModifiers::META,
            ),
            (KeyCode::Char('+'), KeyModifiers::CTRL),
            (KeyCode::Char('-'), KeyModifiers::CTRL),
            (KeyCode::Char('='), KeyModifiers::CTRL),
            (KeyCode::Char('-'), KeyModifiers::empty()),
            (KeyCode::Char('='), KeyModifiers::empty()),
            (KeyCode::Char('ц'), KeyModifiers::empty()),
            (KeyCode::F(1), KeyModifiers::empty()),
            (KeyCode::F(24), KeyModifiers::CTRL),
            (KeyCode::F(35), KeyModifiers::empty()),
            (KeyCode::Up, KeyModifiers::empty()),
            (KeyCode::Down, KeyModifiers::SHIFT),
            (KeyCode::Left, KeyModifiers::ALT),
            (KeyCode::Right, KeyModifiers::CTRL),
            (KeyCode::Home, KeyModifiers::empty()),
            (KeyCode::End, KeyModifiers::empty()),
            (KeyCode::PageUp, KeyModifiers::empty()),
            (KeyCode::PageDown, KeyModifiers::empty()),
            (KeyCode::Backspace, KeyModifiers::empty()),
            (KeyCode::Delete, KeyModifiers::empty()),
            (KeyCode::Insert, KeyModifiers::empty()),
            (KeyCode::Tab, KeyModifiers::empty()),
            (KeyCode::Tab, KeyModifiers::SHIFT),
            (KeyCode::Enter, KeyModifiers::empty()),
            (KeyCode::Space, KeyModifiers::empty()),
            (KeyCode::Escape, KeyModifiers::empty()),
            (KeyCode::CapsLock, KeyModifiers::empty()),
            (KeyCode::ScrollLock, KeyModifiers::empty()),
            (KeyCode::NumLock, KeyModifiers::empty()),
            (KeyCode::PrintScreen, KeyModifiers::empty()),
            (KeyCode::Pause, KeyModifiers::empty()),
            (KeyCode::Menu, KeyModifiers::empty()),
            (KeyCode::Kp0, KeyModifiers::empty()),
            (KeyCode::Kp9, KeyModifiers::empty()),
            (KeyCode::KpEnter, KeyModifiers::empty()),
            (KeyCode::KpPageUp, KeyModifiers::empty()),
            (KeyCode::KpPageDown, KeyModifiers::empty()),
            (KeyCode::KpBegin, KeyModifiers::empty()),
            (KeyCode::MediaPlay, KeyModifiers::empty()),
            (KeyCode::MediaPlayPause, KeyModifiers::empty()),
            (KeyCode::VolumeUp, KeyModifiers::empty()),
            (KeyCode::VolumeMute, KeyModifiers::empty()),
            (KeyCode::LeftShift, KeyModifiers::empty()),
            (KeyCode::RightMeta, KeyModifiers::empty()),
            (KeyCode::IsoLevel3Shift, KeyModifiers::empty()),
            (KeyCode::IsoLevel5Shift, KeyModifiers::empty()),
        ];
        for (code, mods) in cases {
            let k = Key::new(*code, *mods).normalized();
            let s = k.to_string();
            let parsed = s
                .parse::<Key>()
                .unwrap_or_else(|e| panic!("failed to parse {s:?} (from {code:?}, {mods:?}): {e}"));
            assert_eq!(parsed, k, "round-trip mismatch for {s:?}");
        }
    }

    #[test]
    fn eq_ignores_lock_state() {
        let plain = Key::new(KeyCode::Char('c'), KeyModifiers::CTRL).normalized();
        let with_caps = Key::new(
            KeyCode::Char('c'),
            KeyModifiers::CTRL | KeyModifiers::CAPS_LOCK,
        )
        .normalized();
        let with_num = Key::new(
            KeyCode::Char('c'),
            KeyModifiers::CTRL | KeyModifiers::NUM_LOCK,
        )
        .normalized();
        let with_scroll = Key::new(
            KeyCode::Char('c'),
            KeyModifiers::CTRL | KeyModifiers::SCROLL_LOCK,
        )
        .normalized();
        let with_all_locks = Key::new(
            KeyCode::Char('c'),
            KeyModifiers::CTRL | KeyModifiers::LOCK_MASK,
        )
        .normalized();
        assert_eq!(plain, with_caps);
        assert_eq!(plain, with_num);
        assert_eq!(plain, with_scroll);
        assert_eq!(plain, with_all_locks);
    }

    #[test]
    fn hash_ignores_lock_state() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        fn h(k: &Key) -> u64 {
            let mut s = DefaultHasher::new();
            k.hash(&mut s);
            s.finish()
        }
        let plain = Key::new(KeyCode::Char('c'), KeyModifiers::CTRL).normalized();
        let with_locks = Key::new(
            KeyCode::Char('c'),
            KeyModifiers::CTRL | KeyModifiers::LOCK_MASK,
        )
        .normalized();
        assert_eq!(h(&plain), h(&with_locks));
    }

    #[test]
    fn eq_still_distinguishes_binding_modifiers() {
        let ctrl = Key::new(KeyCode::Char('c'), KeyModifiers::CTRL).normalized();
        let alt = Key::new(KeyCode::Char('c'), KeyModifiers::ALT).normalized();
        let ctrl_alt =
            Key::new(KeyCode::Char('c'), KeyModifiers::CTRL | KeyModifiers::ALT).normalized();
        assert_ne!(ctrl, alt);
        assert_ne!(ctrl, ctrl_alt);
    }

    #[test]
    fn parsed_key_equals_key_with_lock_state() {
        let parsed: Key = "ctrl+c".parse().expect("parse");
        let live = Key::new(
            KeyCode::Char('c'),
            KeyModifiers::CTRL | KeyModifiers::CAPS_LOCK,
        )
        .normalized();
        assert_eq!(parsed, live);
    }

    #[test]
    fn matches_simple_char() {
        let k = Key::new(KeyCode::Char('a'), KeyModifiers::empty()).normalized();
        assert!(k.matches("a"));
        assert!(!k.matches("b"));
    }

    #[test]
    fn matches_modifier_combo() {
        let k = Key::new(KeyCode::Char('c'), KeyModifiers::CTRL).normalized();
        assert!(k.matches("ctrl+c"));
        assert!(!k.matches("alt+c"));
        assert!(!k.matches("ctrl+x"));
    }

    #[test]
    fn matches_named_key() {
        let k = Key::new(KeyCode::F(5), KeyModifiers::SHIFT).normalized();
        assert!(k.matches("shift+f5"));
        assert!(!k.matches("f5"));
    }

    #[test]
    fn matches_ignores_lock_state() {
        let k = Key::new(
            KeyCode::Char('a'),
            KeyModifiers::CTRL | KeyModifiers::CAPS_LOCK | KeyModifiers::NUM_LOCK,
        )
        .normalized();
        assert!(k.matches("ctrl+a"));
    }

    #[test]
    fn matches_invalid_pattern_is_false() {
        let k = Key::new(KeyCode::Char('a'), KeyModifiers::empty()).normalized();
        assert!(!k.matches(""));
        assert!(!k.matches("frob+nargle"));
        assert!(!k.matches("ctrl+"));
    }

    #[test]
    fn matches_any_basic() {
        let k = Key::new(KeyCode::Escape, KeyModifiers::empty()).normalized();
        assert!(k.matches_any(["esc", "ctrl+c", "q"]));
        assert!(!k.matches_any(["enter", "tab"]));
    }

    #[test]
    fn matches_any_empty_is_false() {
        let k = Key::new(KeyCode::Char('a'), KeyModifiers::empty()).normalized();
        let none: [&str; 0] = [];
        assert!(!k.matches_any(none));
    }

    #[test]
    fn matches_any_accepts_string_and_str() {
        let k = Key::new(KeyCode::Char('q'), KeyModifiers::empty()).normalized();
        let owned: Vec<String> = vec!["esc".into(), "q".into()];
        assert!(k.matches_any(&owned));
        let borrowed: Vec<&str> = vec!["esc", "q"];
        assert!(k.matches_any(borrowed));
    }

    #[test]
    fn matches_is_case_sensitive_for_letters() {
        // Vim-style: g and G are distinct bindings.
        let plain_g = Key::new(KeyCode::Char('g'), KeyModifiers::empty()).normalized();
        assert!(plain_g.matches("g"));
        assert!(!plain_g.matches("G"));
        assert!(!plain_g.matches("shift+g"));

        // `Key::new(Char('G'), empty).normalized()` normalizes to Char('g') + SHIFT,
        // matching what a decoder emits for a shift+g press.
        let big_g = Key::new(KeyCode::Char('G'), KeyModifiers::empty()).normalized();
        assert_eq!(big_g.code, KeyCode::Char('g'));
        assert!(big_g.modifiers.contains(KeyModifiers::SHIFT));
        assert!(big_g.matches("G"));
        assert!(big_g.matches("shift+g"));
        assert!(!big_g.matches("g"));
    }

    #[test]
    fn matches_modifier_combos_with_letters() {
        let ctrl_g = Key::new(KeyCode::Char('g'), KeyModifiers::CTRL).normalized();
        assert!(ctrl_g.matches("ctrl+g"));
        assert!(!ctrl_g.matches("ctrl+G"));
        assert!(!ctrl_g.matches("ctrl+shift+g"));
        assert!(!ctrl_g.matches("g"));
        assert!(!ctrl_g.matches("alt+g"));
    }

    #[test]
    fn matches_text_first_layout_independent() {
        // shift+1 on a US layout produces "!". Pattern "!" should hit
        // the key by its produced text even though the code is `1`.
        let mut shift_1 = Key::new(KeyCode::Char('1'), KeyModifiers::SHIFT).normalized();
        shift_1.text = Some("!".to_string());
        assert!(shift_1.matches("!"));
        // The physical-key spelling still matches via strict equality.
        assert!(shift_1.matches("shift+1"));
    }

    #[test]
    fn matches_text_first_respects_binding_modifiers() {
        // ctrl+! must not silently match a key whose text happens to
        // be "!" — binding modifiers gate the text fallback.
        let mut shift_1 = Key::new(KeyCode::Char('1'), KeyModifiers::SHIFT).normalized();
        shift_1.text = Some("!".to_string());
        assert!(!shift_1.matches("ctrl+!"));
    }

    #[test]
    fn matches_text_first_handles_layout_glyph() {
        // shift+/ produces "?" on a US layout.
        let mut shift_slash = Key::new(KeyCode::Char('/'), KeyModifiers::SHIFT).normalized();
        shift_slash.text = Some("?".to_string());
        assert!(shift_slash.matches("?"));
        assert!(shift_slash.matches("shift+/"));
    }
}
