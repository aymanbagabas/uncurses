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
    /// Function key. Valid range is `1..=35` (xterm goes to F20,
    /// kitty extends to F35). Construct via [`KeyCode::function`] for
    /// a checked builder; the bare variant is `pub` so decoders can
    /// emit literals, but downstream code matching on `F(n)` should
    /// treat values outside the valid range as bugs.
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

impl KeyCode {
    /// Highest valid function-key index. Matches the kitty keyboard
    /// protocol's F1..F35 range (xterm tops out at F20).
    pub const FUNCTION_KEY_MAX: u8 = 35;

    /// Construct a function-key code, validating that `n` is in
    /// `1..=FUNCTION_KEY_MAX`. Returns `None` for `n == 0` or values
    /// above the supported range.
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
            KeyCode::BackTab => f.write_str("backtab"),
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

/// Errors produced when parsing a [`Key`] or [`KeyCode`] from a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseKeyError {
    /// Input was empty or whitespace-only.
    Empty,
    /// A `+`-separated component was empty (e.g. `"ctrl++a"` or
    /// `"ctrl+"`).
    EmptyComponent,
    /// A modifier token was not recognized.
    UnknownModifier(String),
    /// The terminal key token was not recognized.
    UnknownKey(String),
    /// A function-key token (`f<n>`) had an out-of-range index.
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
    let mods = match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => KeyModifiers::CTRL,
        "alt" | "opt" | "option" => KeyModifiers::ALT,
        "shift" => KeyModifiers::SHIFT,
        "super" | "win" | "cmd" | "command" => KeyModifiers::SUPER,
        "hyper" => KeyModifiers::HYPER,
        "meta" => KeyModifiers::META,
        _ => return Err(ParseKeyError::UnknownModifier(token.to_string())),
    };
    Ok(mods)
}

fn parse_key_code(token: &str) -> Result<KeyCode, ParseKeyError> {
    // Single non-`+` character: treat as Char (case preserved so
    // `Key::new` can canonicalize uppercase into shift+lowercase).
    let mut chars = token.chars();
    if let Some(first) = chars.next()
        && chars.next().is_none()
    {
        return Ok(KeyCode::Char(first));
    }

    // Function key: f<n> where 1 <= n <= KeyCode::FUNCTION_KEY_MAX.
    let lower = token.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix('f')
        && rest.chars().all(|c| c.is_ascii_digit())
        && !rest.is_empty()
    {
        let n: u8 = rest
            .parse()
            .map_err(|_| ParseKeyError::InvalidFunctionKey(token.to_string()))?;
        return KeyCode::function(n)
            .ok_or_else(|| ParseKeyError::InvalidFunctionKey(token.to_string()));
    }

    Ok(match lower.as_str() {
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
        "pgdn" | "pagedown" => KeyCode::PageDown,
        // Editing
        "backspace" | "bs" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
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
        // Punctuation aliases. `plus` is the Display form for `Char('+')`;
        // the literal `+` is also accepted via the single-character path.
        "plus" => KeyCode::Char('+'),
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
        "kppgdn" | "kppagedown" => KeyCode::KpPageDown,
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
            return Ok(Key::new(code, KeyModifiers::empty()));
        }

        // Split on the last `+` to separate modifiers from the key.
        // The literal `+` key is spelled `plus` in Display output, but
        // forms like `ctrl++` are also accepted: a trailing `++` means
        // "the `+` key, with the preceding `+` as separator". A single
        // trailing `+` with no preceding `+` is a dangling separator.
        let (mod_part, key_part) = if let Some(head) = s.strip_suffix('+') {
            match head.strip_suffix('+') {
                Some(mods) => (mods, "+"),
                None => return Err(ParseKeyError::EmptyComponent),
            }
        } else {
            s.rsplit_once('+').unwrap_or(("", s))
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

        let code = parse_key_code(key_part)?;
        Ok(Key::new(code, modifiers))
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
    fn display_uses_full_words() {
        // Display always emits the unabbreviated name; the short forms
        // (`pgup`, `pgdn`, `esc`, etc.) live only on the parse side.
        assert_eq!(
            Key::new(KeyCode::PageUp, KeyModifiers::empty()).to_string(),
            "pageup"
        );
        assert_eq!(
            Key::new(KeyCode::PageDown, KeyModifiers::empty()).to_string(),
            "pagedown"
        );
        assert_eq!(
            Key::new(KeyCode::Escape, KeyModifiers::empty()).to_string(),
            "escape"
        );
        // Round-trip the short forms through Display and back to the
        // same Key.
        assert_eq!(parse("pgup"), parse("pageup"));
        assert_eq!(parse("pgdn"), parse("pagedown"));
        assert_eq!(parse("esc"), parse("escape"));
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
            "kppageup"
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
            Key::new(KeyCode::Char('+'), KeyModifiers::CTRL)
        );
        // `ctrl+plus` resolves to the same Key.
        assert_eq!(parse("ctrl++"), parse("ctrl+plus"));
    }

    #[test]
    fn fromstr_literal_symbols() {
        // Symbols other than `+` parse literally with no aliases.
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
            Key::new(KeyCode::KpPageDown, KeyModifiers::empty()).to_string(),
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
            (KeyCode::BackTab, KeyModifiers::empty()),
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
            let k = Key::new(*code, *mods);
            let s = k.to_string();
            let parsed = s
                .parse::<Key>()
                .unwrap_or_else(|e| panic!("failed to parse {s:?} (from {code:?}, {mods:?}): {e}"));
            assert_eq!(parsed, k, "round-trip mismatch for {s:?}");
        }
    }
}
