//! Terminal capability snapshot.
//!
//! [`Capabilities`] aggregates feature support and identity
//! information learned from reply events. Build one with
//! [`Capabilities::new`] and feed every incoming [`Event`] through
//! [`Capabilities::update`]; relevant replies populate the matching
//! fields. Fields stay `None` until a positive or negative reply
//! arrives — treat `None` as "do not enable".

use crate::ansi::kitty::KittyFlags;
use crate::ansi::mode::{Mode, ModeSetting};
use crate::color::Color;
use crate::event::{Event, ModifyOtherKeysMode};
use crate::terminal::Env;

use bitflags::bitflags;

bitflags! {
    /// Features that [`Feature::probe`] knows how to query.
    ///
    /// Each bit corresponds to a single capability or identity reply
    /// the terminal can produce. Callers compose a set based on what
    /// they want to ask about, write the bytes returned by
    /// [`Feature::probe`] to the terminal, and feed the resulting
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
    /// Build the byte sequence that queries every feature in `self`.
    ///
    /// The bytes are written in a deterministic order: DECRQM mode
    /// queries first (they share a uniform request shape and reply
    /// shape), then device attributes, identity, keyboard, graphics,
    /// geometry, termcap, and color queries last.
    ///
    /// This is a pure function — it does not touch state or perform
    /// I/O. Callers write the returned bytes to their terminal output
    /// stream themselves.
    pub fn probe(self) -> Vec<u8> {
        use crate::ansi::{
            background, ctrl, graphics, kitty, mode as mode_helpers, termcap, winop, xterm,
        };

        let mut out = Vec::new();

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
                mode_helpers::write_request_mode(&mut out, *m).unwrap();
            }
        }

        // --- Device attributes + identity ---------------------------
        if self.contains(Self::DA2) {
            out.extend_from_slice(ctrl::REQUEST_SECONDARY_DA);
        }
        if self.contains(Self::DA3) {
            out.extend_from_slice(ctrl::REQUEST_TERTIARY_DA);
        }
        if self.contains(Self::XTVERSION) {
            out.extend_from_slice(ctrl::REQUEST_XTVERSION);
        }

        // --- Keyboard ------------------------------------------------
        if self.contains(Self::KITTY_KEYBOARD) {
            out.extend_from_slice(kitty::REQUEST_KITTY_KEYBOARD);
        }
        if self.contains(Self::MODIFY_OTHER_KEYS) {
            out.extend_from_slice(xterm::QUERY_MODIFY_OTHER_KEYS);
        }

        // --- Graphics ------------------------------------------------
        if self.contains(Self::KITTY_GRAPHICS) {
            // Standard probe: query a 1x1 in-memory image. Terminals
            // that don't support the protocol stay silent.
            graphics::write_kitty_graphics(&mut out, &["a=q", "t=d", "i=1", "s=1", "v=1"], &[])
                .unwrap();
        }

        // --- Geometry ------------------------------------------------
        if self.contains(Self::CELL_PIXEL_SIZE) {
            winop::write_window_op(&mut out, winop::op::REQUEST_CELL_SIZE, &[]).unwrap();
        }
        if self.contains(Self::WINDOW_PIXEL_SIZE) {
            winop::write_window_op(&mut out, winop::op::REQUEST_WINDOW_SIZE, &[]).unwrap();
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
            termcap::write_xtgettcap(&mut out, &caps).unwrap();
        }

        // --- Color queries ------------------------------------------
        if self.contains(Self::FOREGROUND_COLOR) {
            out.extend_from_slice(background::REQUEST_FOREGROUND_COLOR);
        }
        if self.contains(Self::BACKGROUND_COLOR) {
            out.extend_from_slice(background::REQUEST_BACKGROUND_COLOR);
        }
        if self.contains(Self::CURSOR_COLOR) {
            out.extend_from_slice(background::REQUEST_CURSOR_COLOR);
        }

        // --- DA1 last (acts as a natural probe terminator) ----------
        if self.contains(Self::DA1) {
            out.extend_from_slice(ctrl::REQUEST_PRIMARY_DA);
        }

        out
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
#[derive(Debug, Default, Clone)]
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
    /// Build an empty snapshot.
    pub fn new() -> Self {
        Self::default()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_all_none() {
        let c = Capabilities::new();
        assert!(c.sixel.is_none());
        assert!(c.bracketed_paste.is_none());
        assert!(c.kitty_keyboard.is_none());
        assert!(c.da1.is_empty());
    }

    #[test]
    fn da1_sets_sixel_and_regis() {
        let mut c = Capabilities::new();
        let ev = Event::PrimaryDeviceAttributes(vec![Some(64), Some(4), Some(22)]);
        assert!(c.update(&ev));
        assert_eq!(c.sixel, Some(true));
        assert_eq!(c.regis, Some(false));
        assert_eq!(c.da1, vec![Some(64), Some(4), Some(22)]);
    }

    #[test]
    fn da1_without_sixel_marks_false() {
        let mut c = Capabilities::new();
        c.update(&Event::PrimaryDeviceAttributes(vec![Some(1), Some(2)]));
        assert_eq!(c.sixel, Some(false));
        assert_eq!(c.regis, Some(false));
    }

    #[test]
    fn xtversion_stores_raw_payload() {
        let mut c = Capabilities::new();
        c.update(&Event::TerminalVersion("xterm(395)".to_string()));
        assert_eq!(c.name_version.as_deref(), Some("xterm(395)"));
    }

    #[test]
    fn keyboard_enhancements_unpacks_flags() {
        let mut c = Capabilities::new();
        c.update(&Event::KeyboardEnhancements { flags: 0b101 });
        assert_eq!(
            c.kitty_keyboard,
            Some(KittyFlags::DISAMBIGUATE_ESCAPE_CODES | KittyFlags::REPORT_ALTERNATE_KEYS)
        );
    }

    #[test]
    fn modify_other_keys_stored() {
        let mut c = Capabilities::new();
        c.update(&Event::ModifyOtherKeys(ModifyOtherKeysMode::Mode2));
        assert_eq!(c.modify_other_keys, Some(ModifyOtherKeysMode::Mode2));
    }

    #[test]
    fn kitty_graphics_marks_supported() {
        let mut c = Capabilities::new();
        c.update(&Event::KittyGraphics {
            options: vec![],
            payload: vec![],
        });
        assert_eq!(c.kitty_graphics, Some(true));
    }

    #[test]
    fn mode_report_set_maps_to_supported() {
        let mut c = Capabilities::new();
        c.update(&Event::ModeReport {
            mode: Mode::SYNCHRONIZED_OUTPUT,
            setting: ModeSetting::Set,
        });
        assert_eq!(c.synchronized_output, Some(true));
    }

    #[test]
    fn mode_report_not_recognized_maps_to_unsupported() {
        let mut c = Capabilities::new();
        c.update(&Event::ModeReport {
            mode: Mode::IN_BAND_RESIZE,
            setting: ModeSetting::NotRecognized,
        });
        assert_eq!(c.in_band_resize, Some(false));
    }

    #[test]
    fn mode_report_reset_still_means_supported() {
        let mut c = Capabilities::new();
        c.update(&Event::ModeReport {
            mode: Mode::BRACKETED_PASTE,
            setting: ModeSetting::Reset,
        });
        assert_eq!(c.bracketed_paste, Some(true));
    }

    #[test]
    fn mode_report_unknown_mode_ignored() {
        let mut c = Capabilities::new();
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
        let mut c = Capabilities::new();
        assert!(c.update(&Event::Termcap("536D756C78=31".to_string())));
        assert_eq!(c.styled_underline, Some(true));
    }

    #[test]
    fn termcap_smulx_without_value_marks_unsupported() {
        let mut c = Capabilities::new();
        c.update(&Event::Termcap("536D756C78".to_string()));
        assert_eq!(c.styled_underline, Some(false));
    }

    #[test]
    fn termcap_ms_marks_clipboard() {
        // "Ms" -> "4d73"
        let mut c = Capabilities::new();
        c.update(&Event::Termcap("4d73=313b32".to_string()));
        assert_eq!(c.clipboard, Some(true));
    }

    #[test]
    fn cell_and_window_pixel_size_stored() {
        let mut c = Capabilities::new();
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
        let mut c = Capabilities::new();
        let updated = c.update(&Event::FocusIn);
        assert!(!updated);
    }

    // -- Feature::probe --------------------------------------------------

    #[test]
    fn probe_empty_is_empty() {
        assert!(Feature::empty().probe().is_empty());
    }

    #[test]
    fn probe_da1_emits_request() {
        let bytes = Feature::DA1.probe();
        assert_eq!(bytes, b"\x1b[c");
    }

    #[test]
    fn probe_da1_is_last_when_combined() {
        let bytes = Feature::DA1
            .union(Feature::DA2)
            .union(Feature::BRACKETED_PASTE)
            .probe();
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
        let bytes = Feature::BRACKETED_PASTE.probe();
        assert_eq!(bytes, b"\x1b[?2004$p");
    }

    #[test]
    fn probe_kitty_keyboard() {
        let bytes = Feature::KITTY_KEYBOARD.probe();
        assert_eq!(bytes, b"\x1b[?u");
    }

    #[test]
    fn probe_termcap_groups_caps_into_one_xtgettcap() {
        let bytes = (Feature::STYLED_UNDERLINE | Feature::CLIPBOARD).probe();
        // "Smulx" -> "536D756C78", "Ms" -> "4D73", joined by ';'.
        assert_eq!(bytes, b"\x1bP+q536D756C78;4D73\x1b\\");
    }

    #[test]
    fn probe_pixel_size_uses_window_ops() {
        let bytes = (Feature::CELL_PIXEL_SIZE | Feature::WINDOW_PIXEL_SIZE).probe();
        // CSI 16 t (cell) then CSI 14 t (window) in declared order.
        assert_eq!(bytes, b"\x1b[16t\x1b[14t");
    }

    #[test]
    fn probe_kitty_graphics_uses_apc_query() {
        let bytes = Feature::KITTY_GRAPHICS.probe();
        assert!(bytes.starts_with(b"\x1b_G"));
        assert!(bytes.ends_with(b"\x1b\\"));
        assert!(bytes.windows(3).any(|w| w == b"a=q"));
    }

    #[test]
    fn probe_color_queries() {
        let bytes =
            (Feature::FOREGROUND_COLOR | Feature::BACKGROUND_COLOR | Feature::CURSOR_COLOR).probe();
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
