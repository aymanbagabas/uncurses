//! Terminal capability snapshot.
//!
//! [`Capabilities`] aggregates feature support and identity
//! information learned from the environment and from reply events.
//! Build one with [`Capabilities::from_env`] passing an [`Env`] (which
//! seeds entries that can be inferred without a round-trip, like
//! iTerm2 inline image protocol support), then feed every incoming
//! [`Event`] through [`Capabilities::update`]; relevant replies
//! populate the remaining fields. Fields stay `None` until a positive
//! or negative reply arrives — treat `None` as "do not enable".

use std::io::{self, Write};

use crate::ansi::kitty::KittyFlags;
use crate::ansi::mode::{Mode, ModeSetting};
use crate::color::Color;
use crate::event::{Event, ModifyOtherKeysMode};
use crate::terminal::Env;

use bitflags::bitflags;

bitflags! {
    /// Features that [`Feature::write_probe`] knows how to query.
    ///
    /// Each bit corresponds to a single capability or identity reply
    /// the terminal can produce. Callers compose a set based on what
    /// they want to ask about, hand it to [`Feature::write_probe`]
    /// along with their terminal output stream, and feed the resulting
    /// reply events through [`Capabilities::update`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Feature: u32 {
        /// Primary device attributes (DA1).
        const DA1                  = 1 <<  0;
        /// Secondary device attributes (DA2).
        const DA2                  = 1 <<  1;
        /// Tertiary device attributes (DA3).
        const DA3                  = 1 <<  2;
        /// Terminal name + version (XTVERSION).
        const XTVERSION            = 1 <<  3;
        /// Kitty keyboard protocol flags (`CSI ? u`).
        const KITTY_KEYBOARD       = 1 <<  4;
        /// Kitty graphics protocol (APC query).
        const KITTY_GRAPHICS       = 1 <<  5;
        /// modifyOtherKeys query (`CSI ? 4 m`).
        const MODIFY_OTHER_KEYS    = 1 <<  6;
        /// Synchronized output mode (DEC 2026).
        const SYNCHRONIZED_OUTPUT  = 1 <<  7;
        /// Unicode core mode (DEC 2027).
        const UNICODE_CORE         = 1 <<  8;
        /// Light/dark color scheme notifications (DEC 2031).
        const THEME_REPORTING      = 1 <<  9;
        /// In-band resize notifications (DEC 2048).
        const IN_BAND_RESIZE       = 1 << 10;
        /// Bracketed paste (DEC 2004).
        const BRACKETED_PASTE      = 1 << 11;
        /// Focus in/out reporting (DEC 1004).
        const FOCUS_EVENTS         = 1 << 12;
        /// SGR mouse encoding (DEC 1006).
        const MOUSE_SGR            = 1 << 13;
        /// SGR-pixel mouse encoding (DEC 1016).
        const MOUSE_SGR_PIXELS     = 1 << 14;
        /// Extended ("styled") underlines via terminfo capability
        /// `Smulx`: support for `CSI 4 : n m` to draw curly /
        /// dotted / dashed underlines, including colored underlines
        /// via `CSI 58`.
        ///
        /// This is the only termcap query with broad cross-terminal
        /// agreement. OSC 8 hyperlink support has no standard
        /// termcap probe and is not exposed here.
        const STYLED_UNDERLINE     = 1 << 15;
        /// OSC 52 clipboard access via termcap `Ms`.
        const CLIPBOARD            = 1 << 16;
        /// Cell pixel size (`CSI 16 t`).
        const CELL_PIXEL_SIZE      = 1 << 17;
        /// Window pixel size (`CSI 14 t`).
        const WINDOW_PIXEL_SIZE    = 1 << 18;
        /// Default foreground color (OSC 10 `?`).
        const FOREGROUND_COLOR     = 1 << 19;
        /// Default background color (OSC 11 `?`).
        const BACKGROUND_COLOR     = 1 << 20;
        /// Default cursor color (OSC 12 `?`).
        const CURSOR_COLOR         = 1 << 21;
    }
}

impl Feature {
    /// Write the byte sequence that queries every feature in `self`
    /// to `w`.
    ///
    /// The bytes are emitted in a deterministic order: DECRQM mode
    /// queries first (they share a uniform request shape and reply
    /// shape), then device attributes, identity, keyboard, graphics,
    /// geometry, termcap, and color queries last; DA1 is emitted last
    /// so its reply naturally terminates the probe round-trip.
    ///
    /// Errors propagate from `w` directly. Tests can pass any
    /// `Vec<u8>` since `Vec<u8>` implements [`Write`].
    pub fn write_probe<W: Write>(self, w: &mut W) -> io::Result<()> {
        use crate::ansi::{
            background, ctrl, graphics, kitty, mode as mode_helpers, termcap, winop, xterm,
        };

        // --- DECRQM mode queries -------------------------------------
        let modes: &[(Self, Mode)] = &[
            (Self::SYNCHRONIZED_OUTPUT, Mode::SYNCHRONIZED_OUTPUT),
            (Self::UNICODE_CORE, Mode::UNICODE_CORE),
            (Self::THEME_REPORTING, Mode::LIGHT_DARK),
            (Self::IN_BAND_RESIZE, Mode::IN_BAND_RESIZE),
            (Self::BRACKETED_PASTE, Mode::BRACKETED_PASTE),
            (Self::FOCUS_EVENTS, Mode::FOCUS),
            (Self::MOUSE_SGR, Mode::MOUSE_SGR),
            (Self::MOUSE_SGR_PIXELS, Mode::MOUSE_SGR_PIXEL),
        ];
        for (bit, m) in modes {
            if self.contains(*bit) {
                mode_helpers::write_request_mode(w, *m)?;
            }
        }

        // --- Device attributes + identity ---------------------------
        if self.contains(Self::DA2) {
            w.write_all(ctrl::REQUEST_SECONDARY_DA)?;
        }
        if self.contains(Self::DA3) {
            w.write_all(ctrl::REQUEST_TERTIARY_DA)?;
        }
        if self.contains(Self::XTVERSION) {
            w.write_all(ctrl::REQUEST_XTVERSION)?;
        }

        // --- Keyboard ------------------------------------------------
        if self.contains(Self::KITTY_KEYBOARD) {
            w.write_all(kitty::REQUEST_KITTY_KEYBOARD)?;
        }
        if self.contains(Self::MODIFY_OTHER_KEYS) {
            w.write_all(xterm::QUERY_MODIFY_OTHER_KEYS)?;
        }

        // --- Graphics ------------------------------------------------
        if self.contains(Self::KITTY_GRAPHICS) {
            // Standard probe: query a 1x1 in-memory image. Terminals
            // that don't support the protocol stay silent.
            graphics::write_kitty_graphics(w, &["a=q", "t=d", "i=1", "s=1", "v=1"], &[])?;
        }

        // --- Geometry ------------------------------------------------
        if self.contains(Self::CELL_PIXEL_SIZE) {
            winop::write_window_op(w, winop::op::REQUEST_CELL_SIZE, &[])?;
        }
        if self.contains(Self::WINDOW_PIXEL_SIZE) {
            winop::write_window_op(w, winop::op::REQUEST_WINDOW_SIZE, &[])?;
        }

        // --- Termcap (single XTGETTCAP with all requested names) ----
        let mut caps: Vec<&str> = Vec::new();
        if self.contains(Self::STYLED_UNDERLINE) {
            caps.push("Smulx");
        }
        if self.contains(Self::CLIPBOARD) {
            caps.push("Ms");
        }
        if !caps.is_empty() {
            termcap::write_xtgettcap(w, &caps)?;
        }

        // --- Color queries ------------------------------------------
        if self.contains(Self::FOREGROUND_COLOR) {
            w.write_all(background::REQUEST_FOREGROUND_COLOR)?;
        }
        if self.contains(Self::BACKGROUND_COLOR) {
            w.write_all(background::REQUEST_BACKGROUND_COLOR)?;
        }
        if self.contains(Self::CURSOR_COLOR) {
            w.write_all(background::REQUEST_CURSOR_COLOR)?;
        }

        // --- DA1 last (acts as a natural probe terminator) ----------
        if self.contains(Self::DA1) {
            w.write_all(ctrl::REQUEST_PRIMARY_DA)?;
        }

        Ok(())
    }

    /// Build a recommended probe set from environment hints.
    ///
    /// Returns `Self::all()` minus probes that are known to misbehave
    /// on the terminal identified by `env`. Examples:
    ///
    /// * `TERM` of `dumb` (or unset / empty) returns the empty set —
    ///   nothing is safe to probe.
    /// * `TERM_PROGRAM == "Apple_Terminal"` returns the empty set:
    ///   Apple Terminal echoes most queries back as visible garbage
    ///   instead of replying (notably XTVERSION) and does not
    ///   reliably answer DECRQM, so no probe is safe.
    pub fn from_env(env: &Env) -> Self {
        let term = env.get("TERM").unwrap_or_default();
        if term.is_empty() || term == "dumb" {
            return Self::empty();
        }

        if env.get("TERM_PROGRAM").as_deref() == Some("Apple_Terminal") {
            return Self::empty();
        }

        Self::all()
    }
}

impl Default for Feature {
    /// Equivalent to `Self::from_env(&Env::from_process())`.
    fn default() -> Self {
        Self::from_env(&Env::from_process())
    }
}

/// Snapshot of terminal features and identity discovered from reply
/// events. See module documentation for the intended workflow.
#[derive(Debug, Clone)]
pub struct Capabilities {
    // --- Identity --------------------------------------------------------
    /// XTVERSION reply payload (typically `"name(version)"`).
    pub name_version: Option<String>,
    /// Primary device attributes (DA1) reply.
    pub da1: Vec<Option<u32>>,
    /// Secondary device attributes (DA2) reply.
    pub da2: Vec<Option<u32>>,
    /// Tertiary device attributes (DA3) reply (terminal ID string).
    pub da3: Option<String>,

    // --- Derived from DA1 -----------------------------------------------
    /// Sixel graphics — DA1 attribute `4`.
    pub sixel: Option<bool>,
    /// ReGIS graphics — DA1 attribute `3`.
    pub regis: Option<bool>,

    // --- Keyboard --------------------------------------------------------
    /// Kitty keyboard protocol — current flag set from `CSI ? u` reply.
    pub kitty_keyboard: Option<KittyFlags>,
    /// modifyOtherKeys level from `CSI > 4 ; n m` reply.
    pub modify_other_keys: Option<ModifyOtherKeysMode>,

    // --- Graphics --------------------------------------------------------
    /// Kitty graphics protocol — observed when a kitty graphics reply
    /// arrives. There is no negative reply, so this stays `None` on
    /// terminals that don't support it.
    pub kitty_graphics: Option<bool>,
    /// iTerm2 inline image protocol (OSC 1337 `File=`). There is no
    /// standard probe; support is inferred from the XTVERSION reply
    /// when the reported terminal name matches a known implementer
    /// (iTerm2, WezTerm, rio). Stays `None` until an XTVERSION reply
    /// arrives.
    pub iterm2_graphics: Option<bool>,

    // --- DEC private modes (DECRQM replies) -----------------------------
    /// Synchronized output (DEC mode 2026).
    pub synchronized_output: Option<bool>,
    /// Unicode core mode (DEC mode 2027).
    pub unicode_core: Option<bool>,
    /// Light/dark color scheme notifications (DEC mode 2031).
    pub theme_reporting: Option<bool>,
    /// In-band resize notifications (DEC mode 2048).
    pub in_band_resize: Option<bool>,
    /// Bracketed paste (DEC mode 2004).
    pub bracketed_paste: Option<bool>,
    /// Focus in/out reporting (DEC mode 1004).
    pub focus_events: Option<bool>,
    /// SGR mouse encoding (DEC mode 1006).
    pub mouse_sgr: Option<bool>,
    /// SGR-pixel mouse encoding (DEC mode 1016).
    pub mouse_sgr_pixels: Option<bool>,

    // --- Termcap-derived (XTGETTCAP replies) ----------------------------
    /// Extended ("styled") underline support — terminfo capability `Smulx`.
    pub styled_underline: Option<bool>,
    /// OSC 52 clipboard access (termcap `Ms`).
    pub clipboard: Option<bool>,

    // --- Geometry --------------------------------------------------------
    /// Cell pixel size from `CSI 16 t` reply.
    pub cell_pixel_size: Option<(u16, u16)>,
    /// Window pixel size from `CSI 14 t` reply.
    pub window_pixel_size: Option<(u16, u16)>,

    // --- Live state ------------------------------------------------------
    /// Default foreground color from OSC 10 reply.
    pub foreground: Option<Color>,
    /// Default background color from OSC 11 reply.
    pub background: Option<Color>,
    /// Default cursor color from OSC 12 reply.
    pub cursor_color: Option<Color>,
    /// Last reported color scheme from DEC mode 2031 reports
    /// (`CSI ? 997 ; {1|2} n`). Updated on every
    /// [`Event::DarkColorScheme`] / [`Event::LightColorScheme`].
    pub color_scheme: Option<crate::event::ColorScheme>,
}

impl Capabilities {
    /// Build a snapshot seeded from the environment.
    ///
    /// Entries that can be inferred without a terminal round-trip are
    /// populated up front; everything else stays at its defaults until
    /// a reply event is fed through [`Self::update`].
    ///
    /// Currently the only env-derived inference is iTerm2 inline image
    /// protocol support: set to `Some(true)` when `TERM_PROGRAM` names
    /// a known implementer (iTerm2, WezTerm, rio) or `LC_TERMINAL`
    /// contains `iTerm`; otherwise `Some(false)`.
    pub fn from_env(env: &Env) -> Self {
        Self {
            iterm2_graphics: Some(detect_iterm2_graphics_env(env)),
            ..Self::blank()
        }
    }

    /// Internal "all defaults" constructor used by [`Self::from_env`]
    /// and by tests that want a deterministic starting point.
    fn blank() -> Self {
        Self {
            name_version: None,
            da1: Vec::new(),
            da2: Vec::new(),
            da3: None,
            sixel: None,
            regis: None,
            kitty_keyboard: None,
            modify_other_keys: None,
            kitty_graphics: None,
            iterm2_graphics: None,
            synchronized_output: None,
            unicode_core: None,
            theme_reporting: None,
            in_band_resize: None,
            bracketed_paste: None,
            focus_events: None,
            mouse_sgr: None,
            mouse_sgr_pixels: None,
            styled_underline: None,
            clipboard: None,
            cell_pixel_size: None,
            window_pixel_size: None,
            foreground: None,
            background: None,
            cursor_color: None,
            color_scheme: None,
        }
    }

    /// Ingest one event. Returns `true` when `ev` was a known
    /// capability reply and the snapshot was updated, `false`
    /// otherwise. Safe to call for every event without filtering.
    pub fn update(&mut self, ev: &Event) -> bool {
        match ev {
            Event::PrimaryDeviceAttributes(params) => {
                self.da1 = params.clone();
                self.sixel = Some(params.contains(&Some(4)));
                self.regis = Some(params.contains(&Some(3)));
                true
            }
            Event::SecondaryDeviceAttributes(params) => {
                self.da2 = params.clone();
                true
            }
            Event::TertiaryDeviceAttributes(s) => {
                self.da3 = Some(s.clone());
                true
            }
            Event::TerminalVersion(s) => {
                if detect_iterm2_graphics_xtversion(s) {
                    self.iterm2_graphics = Some(true);
                }
                self.name_version = Some(s.clone());
                true
            }
            Event::KeyboardEnhancements { flags } => {
                self.kitty_keyboard = Some(KittyFlags::from_bits_truncate(*flags));
                true
            }
            Event::ModifyOtherKeys(m) => {
                self.modify_other_keys = Some(*m);
                true
            }
            Event::KittyGraphics { .. } => {
                self.kitty_graphics = Some(true);
                true
            }
            Event::ModeReport { mode, setting } => self.update_mode(*mode, *setting),
            Event::Termcap(payload) => self.update_termcap(payload),
            Event::CellPixelSize { width, height } => {
                self.cell_pixel_size = Some((*width, *height));
                true
            }
            Event::WindowPixelSize { width, height } => {
                self.window_pixel_size = Some((*width, *height));
                true
            }
            Event::ForegroundColor(c) => {
                self.foreground = Some(*c);
                true
            }
            Event::BackgroundColor(c) => {
                self.background = Some(*c);
                true
            }
            Event::CursorColor(c) => {
                self.cursor_color = Some(*c);
                true
            }
            Event::DarkColorScheme => {
                self.color_scheme = Some(crate::event::ColorScheme::Dark);
                true
            }
            Event::LightColorScheme => {
                self.color_scheme = Some(crate::event::ColorScheme::Light);
                true
            }
            _ => false,
        }
    }

    fn update_mode(&mut self, mode: Mode, setting: ModeSetting) -> bool {
        // `NotRecognized` (0) → terminal explicitly doesn't know the
        // mode → Some(false). Any other value means the mode is
        // recognized (set, reset, or permanently set/reset).
        let supported = !matches!(setting, ModeSetting::NotRecognized);
        let slot: &mut Option<bool> = match mode {
            Mode::BRACKETED_PASTE => &mut self.bracketed_paste,
            Mode::SYNCHRONIZED_OUTPUT => &mut self.synchronized_output,
            Mode::UNICODE_CORE => &mut self.unicode_core,
            Mode::LIGHT_DARK => &mut self.theme_reporting,
            Mode::IN_BAND_RESIZE => &mut self.in_band_resize,
            Mode::FOCUS => &mut self.focus_events,
            Mode::MOUSE_SGR => &mut self.mouse_sgr,
            Mode::MOUSE_SGR_PIXEL => &mut self.mouse_sgr_pixels,
            _ => return false,
        };
        *slot = Some(supported);
        true
    }

    fn update_termcap(&mut self, payload: &str) -> bool {
        // XTGETTCAP reply payload is `<hex-name>=<hex-value>` or just
        // `<hex-name>` for an unrecognized cap. We only care about
        // recognized entries here. Decode the hex name and match
        // against the caps we track.
        let name_hex = payload.split('=').next().unwrap_or("");
        let value_present = payload.contains('=');
        let Some(name) = decode_hex_ascii(name_hex) else {
            return false;
        };
        let slot: &mut Option<bool> = match name.as_str() {
            "Smulx" => &mut self.styled_underline,
            "Ms" => &mut self.clipboard,
            _ => return false,
        };
        *slot = Some(value_present);
        true
    }
}

impl Default for Capabilities {
    /// Equivalent to `Self::from_env(&Env::from_process())`.
    fn default() -> Self {
        Self::from_env(&Env::from_process())
    }
}

/// Decode a hex-encoded ASCII string (XTGETTCAP name half).
/// Returns `None` if the input is not an even number of ASCII hex
/// digits.
fn decode_hex_ascii(s: &str) -> Option<String> {
    if !s.is_ascii() || !s.len().is_multiple_of(2) {
        return None;
    }
    let mut out = String::with_capacity(s.len() / 2);
    for chunk in s.as_bytes().chunks_exact(2) {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8 as char);
    }
    Some(out)
}

/// Infer iTerm2 inline image protocol support from an XTVERSION reply.
/// XTVERSION payloads are typically of the form `name(version)` (e.g.
/// `"iTerm2(3.5.0)"`, `"WezTerm 20240203-110809-5046fc22"`,
/// `"rio 0.1.18"`). A case-insensitive substring match against the set
/// of known implementers is sufficient — these terminals all advertise
/// stable, distinctive product names.
fn detect_iterm2_graphics_xtversion(xtversion: &str) -> bool {
    let lower = xtversion.to_ascii_lowercase();
    ["iterm", "wezterm", "rio"]
        .iter()
        .any(|needle| lower.contains(needle))
}

/// Infer iTerm2 inline image protocol support from environment
/// variables, before any terminal round-trip. Mirrors the XTVERSION
/// detection: matches the same set of implementers via the standard
/// terminal-identifying env vars (`TERM_PROGRAM`, plus `LC_TERMINAL`
/// for older iTerm releases).
fn detect_iterm2_graphics_env(env: &Env) -> bool {
    let term_program = env
        .get("TERM_PROGRAM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ["iterm", "wezterm", "rio"]
        .iter()
        .any(|needle| term_program.contains(needle))
    {
        return true;
    }
    let lc_terminal = env
        .get("LC_TERMINAL")
        .unwrap_or_default()
        .to_ascii_lowercase();
    lc_terminal.contains("iterm")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_with_empty_env_is_all_none_except_iterm2() {
        let c = Capabilities::from_env(&Env::empty());
        assert!(c.sixel.is_none());
        assert!(c.bracketed_paste.is_none());
        assert!(c.kitty_keyboard.is_none());
        assert!(c.da1.is_empty());
        // iterm2_graphics is the only field seeded from the env at
        // construction time; with an empty env the answer is "no".
        assert_eq!(c.iterm2_graphics, Some(false));
    }

    #[test]
    fn new_infers_iterm2_graphics_from_term_program() {
        for value in ["iTerm.app", "WezTerm", "rio"] {
            let env = Env::from_pairs([("TERM_PROGRAM", value)]);
            assert_eq!(
                Capabilities::from_env(&env).iterm2_graphics,
                Some(true),
                "expected Some(true) for TERM_PROGRAM={value:?}"
            );
        }
    }

    #[test]
    fn new_infers_iterm2_graphics_from_lc_terminal() {
        let env = Env::from_pairs([("LC_TERMINAL", "iTerm2")]);
        assert_eq!(Capabilities::from_env(&env).iterm2_graphics, Some(true));
    }

    #[test]
    fn new_does_not_infer_iterm2_graphics_for_other_terms() {
        for value in ["Apple_Terminal", "Hyper", "ghostty", "kitty"] {
            let env = Env::from_pairs([("TERM_PROGRAM", value)]);
            assert_eq!(
                Capabilities::from_env(&env).iterm2_graphics,
                Some(false),
                "expected Some(false) for TERM_PROGRAM={value:?}"
            );
        }
    }

    #[test]
    fn xtversion_does_not_downgrade_env_inferred_iterm2_graphics() {
        let env = Env::from_pairs([("TERM_PROGRAM", "WezTerm")]);
        let mut c = Capabilities::from_env(&env);
        // An XTVERSION reply that doesn't mention WezTerm/iTerm/rio
        // must not downgrade the env-positive signal.
        c.update(&Event::TerminalVersion("xterm(395)".to_string()));
        assert_eq!(c.iterm2_graphics, Some(true));
    }

    #[test]
    fn xtversion_upgrades_iterm2_graphics_when_env_silent() {
        let mut c = Capabilities::from_env(&Env::empty());
        assert_eq!(c.iterm2_graphics, Some(false));
        c.update(&Event::TerminalVersion("iTerm2(3.5.0)".to_string()));
        assert_eq!(c.iterm2_graphics, Some(true));
    }

    #[test]
    fn da1_sets_sixel_and_regis() {
        let mut c = Capabilities::from_env(&Env::empty());
        let ev = Event::PrimaryDeviceAttributes(vec![Some(64), Some(4), Some(22)]);
        assert!(c.update(&ev));
        assert_eq!(c.sixel, Some(true));
        assert_eq!(c.regis, Some(false));
        assert_eq!(c.da1, vec![Some(64), Some(4), Some(22)]);
    }

    #[test]
    fn da1_without_sixel_marks_false() {
        let mut c = Capabilities::from_env(&Env::empty());
        c.update(&Event::PrimaryDeviceAttributes(vec![Some(1), Some(2)]));
        assert_eq!(c.sixel, Some(false));
        assert_eq!(c.regis, Some(false));
    }

    #[test]
    fn xtversion_stores_raw_payload() {
        let mut c = Capabilities::from_env(&Env::empty());
        c.update(&Event::TerminalVersion("xterm(395)".to_string()));
        assert_eq!(c.name_version.as_deref(), Some("xterm(395)"));
        assert_eq!(c.iterm2_graphics, Some(false));
    }

    #[test]
    fn xtversion_infers_iterm2_graphics_for_known_implementers() {
        for name in [
            "iTerm2(3.5.0)",
            "WezTerm 20240203-110809-5046fc22",
            "rio 0.1.18",
        ] {
            let mut c = Capabilities::from_env(&Env::empty());
            c.update(&Event::TerminalVersion(name.to_string()));
            assert_eq!(
                c.iterm2_graphics,
                Some(true),
                "expected iterm2_graphics=Some(true) for {name:?}"
            );
        }
    }

    #[test]
    fn xtversion_infers_no_iterm2_graphics_for_others() {
        for name in ["xterm(395)", "ghostty 1.0.0", "kitty 0.32.0", "foot 1.16"] {
            let mut c = Capabilities::from_env(&Env::empty());
            c.update(&Event::TerminalVersion(name.to_string()));
            assert_eq!(
                c.iterm2_graphics,
                Some(false),
                "expected iterm2_graphics=Some(false) for {name:?}"
            );
        }
    }

    #[test]
    fn keyboard_enhancements_unpacks_flags() {
        let mut c = Capabilities::from_env(&Env::empty());
        c.update(&Event::KeyboardEnhancements { flags: 0b101 });
        assert_eq!(
            c.kitty_keyboard,
            Some(KittyFlags::DISAMBIGUATE_ESCAPE_CODES | KittyFlags::REPORT_ALTERNATE_KEYS)
        );
    }

    #[test]
    fn modify_other_keys_stored() {
        let mut c = Capabilities::from_env(&Env::empty());
        c.update(&Event::ModifyOtherKeys(ModifyOtherKeysMode::Mode2));
        assert_eq!(c.modify_other_keys, Some(ModifyOtherKeysMode::Mode2));
    }

    #[test]
    fn kitty_graphics_marks_supported() {
        let mut c = Capabilities::from_env(&Env::empty());
        c.update(&Event::KittyGraphics {
            options: vec![],
            payload: vec![],
        });
        assert_eq!(c.kitty_graphics, Some(true));
    }

    #[test]
    fn mode_report_set_maps_to_supported() {
        let mut c = Capabilities::from_env(&Env::empty());
        c.update(&Event::ModeReport {
            mode: Mode::SYNCHRONIZED_OUTPUT,
            setting: ModeSetting::Set,
        });
        assert_eq!(c.synchronized_output, Some(true));
    }

    #[test]
    fn mode_report_not_recognized_maps_to_unsupported() {
        let mut c = Capabilities::from_env(&Env::empty());
        c.update(&Event::ModeReport {
            mode: Mode::IN_BAND_RESIZE,
            setting: ModeSetting::NotRecognized,
        });
        assert_eq!(c.in_band_resize, Some(false));
    }

    #[test]
    fn mode_report_reset_still_means_supported() {
        let mut c = Capabilities::from_env(&Env::empty());
        c.update(&Event::ModeReport {
            mode: Mode::BRACKETED_PASTE,
            setting: ModeSetting::Reset,
        });
        assert_eq!(c.bracketed_paste, Some(true));
    }

    #[test]
    fn mode_report_unknown_mode_ignored() {
        let mut c = Capabilities::from_env(&Env::empty());
        let updated = c.update(&Event::ModeReport {
            mode: Mode::Dec(9999),
            setting: ModeSetting::Set,
        });
        assert!(!updated);
    }

    #[test]
    fn termcap_smulx_marks_styled_underline_supported() {
        // "Smulx" -> hex "536D756C78"; value can be anything (we only
        // check for presence of '=').
        let mut c = Capabilities::from_env(&Env::empty());
        assert!(c.update(&Event::Termcap("536D756C78=31".to_string())));
        assert_eq!(c.styled_underline, Some(true));
    }

    #[test]
    fn termcap_smulx_without_value_marks_unsupported() {
        let mut c = Capabilities::from_env(&Env::empty());
        c.update(&Event::Termcap("536D756C78".to_string()));
        assert_eq!(c.styled_underline, Some(false));
    }

    #[test]
    fn termcap_ms_marks_clipboard() {
        // "Ms" -> "4d73"
        let mut c = Capabilities::from_env(&Env::empty());
        c.update(&Event::Termcap("4d73=313b32".to_string()));
        assert_eq!(c.clipboard, Some(true));
    }

    #[test]
    fn cell_and_window_pixel_size_stored() {
        let mut c = Capabilities::from_env(&Env::empty());
        c.update(&Event::CellPixelSize {
            width: 10,
            height: 20,
        });
        c.update(&Event::WindowPixelSize {
            width: 800,
            height: 600,
        });
        assert_eq!(c.cell_pixel_size, Some((10, 20)));
        assert_eq!(c.window_pixel_size, Some((800, 600)));
    }

    #[test]
    fn unrelated_event_returns_false() {
        let mut c = Capabilities::from_env(&Env::empty());
        let updated = c.update(&Event::FocusIn);
        assert!(!updated);
    }

    // -- Feature::probe --------------------------------------------------

    /// Helper: collect the bytes `Feature::write_probe` would emit
    /// into a `Vec<u8>` so existing test assertions stay readable.
    fn probe(f: Feature) -> Vec<u8> {
        let mut buf = Vec::new();
        f.write_probe(&mut buf).unwrap();
        buf
    }

    #[test]
    fn probe_empty_is_empty() {
        assert!(probe(Feature::empty()).is_empty());
    }

    #[test]
    fn probe_da1_emits_request() {
        let bytes = probe(Feature::DA1);
        assert_eq!(bytes, b"\x1b[c");
    }

    #[test]
    fn probe_da1_is_last_when_combined() {
        let bytes = probe(Feature::DA1 | Feature::DA2 | Feature::BRACKETED_PASTE);
        // DA1 (b"\x1b[c") must end the buffer so it acts as the
        // probe terminator.
        assert!(bytes.ends_with(b"\x1b[c"));
        // DA2 must appear before DA1.
        let da1_pos = bytes.windows(3).rposition(|w| w == b"\x1b[c").unwrap();
        let da2_pos = bytes.windows(4).position(|w| w == b"\x1b[>c").unwrap();
        assert!(da2_pos < da1_pos);
    }

    #[test]
    fn probe_modes_use_decrqm_shape() {
        let bytes = probe(Feature::BRACKETED_PASTE);
        assert_eq!(bytes, b"\x1b[?2004$p");
    }

    #[test]
    fn probe_kitty_keyboard() {
        let bytes = probe(Feature::KITTY_KEYBOARD);
        assert_eq!(bytes, b"\x1b[?u");
    }

    #[test]
    fn probe_termcap_groups_caps_into_one_xtgettcap() {
        let bytes = probe(Feature::STYLED_UNDERLINE | Feature::CLIPBOARD);
        // "Smulx" -> "536D756C78", "Ms" -> "4D73", joined by ';'.
        assert_eq!(bytes, b"\x1bP+q536D756C78;4D73\x1b\\");
    }

    #[test]
    fn probe_pixel_size_uses_window_ops() {
        let bytes = probe(Feature::CELL_PIXEL_SIZE | Feature::WINDOW_PIXEL_SIZE);
        // CSI 16 t (cell) then CSI 14 t (window) in declared order.
        assert_eq!(bytes, b"\x1b[16t\x1b[14t");
    }

    #[test]
    fn probe_kitty_graphics_uses_apc_query() {
        let bytes = probe(Feature::KITTY_GRAPHICS);
        assert!(bytes.starts_with(b"\x1b_G"));
        assert!(bytes.ends_with(b"\x1b\\"));
        assert!(bytes.windows(3).any(|w| w == b"a=q"));
    }

    #[test]
    fn probe_color_queries() {
        let bytes =
            probe(Feature::FOREGROUND_COLOR | Feature::BACKGROUND_COLOR | Feature::CURSOR_COLOR);
        assert_eq!(bytes, b"\x1b]10;?\x07\x1b]11;?\x07\x1b]12;?\x07");
    }

    #[test]
    fn feature_all_contains_every_bit() {
        let all = Feature::all();
        assert!(all.contains(Feature::DA1));
        assert!(all.contains(Feature::KITTY_GRAPHICS));
        assert!(all.contains(Feature::CURSOR_COLOR));
    }

    // -- Feature::from_env -----------------------------------------------

    fn env_with(pairs: &[(&str, &str)]) -> Env {
        Env::from_pairs(pairs.iter().map(|(k, v)| (*k, *v)))
    }

    #[test]
    fn from_env_dumb_term_yields_empty() {
        let e = env_with(&[("TERM", "dumb")]);
        assert_eq!(Feature::from_env(&e), Feature::empty());
    }

    #[test]
    fn from_env_missing_term_yields_empty() {
        let e = Env::empty();
        assert_eq!(Feature::from_env(&e), Feature::empty());
    }

    #[test]
    fn from_env_known_term_yields_all() {
        let e = env_with(&[("TERM", "xterm-256color")]);
        assert_eq!(Feature::from_env(&e), Feature::all());
    }

    #[test]
    fn from_env_apple_terminal_is_empty() {
        let e = env_with(&[
            ("TERM", "xterm-256color"),
            ("TERM_PROGRAM", "Apple_Terminal"),
        ]);
        assert_eq!(Feature::from_env(&e), Feature::empty());
    }

    #[test]
    fn from_env_other_term_program_yields_all() {
        let e = env_with(&[("TERM", "xterm-256color"), ("TERM_PROGRAM", "kitty")]);
        assert_eq!(Feature::from_env(&e), Feature::all());
    }
}
