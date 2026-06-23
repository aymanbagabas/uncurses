//! Terminal capability flags used by the renderer.
//!
//! ## Purpose
//!
//! [`Optimizations`] is not a feature wishlist; it is the renderer's
//! contract for which byte sequences are safe to emit for the current
//! terminal and line discipline. Each flag unlocks a family of shorter
//! sequences. When a flag is absent, the renderer falls back to more
//! conservative cursor movement or explicit cell writes.
//!
//! ## Detection
//!
//! The built-in detector maps `$TERM` families to conservative baseline
//! sets. Unknown, empty, and `dumb` terminals use [`Optimizations::none`].
//! A missing `$TERM` uses [`Optimizations::default`] so in-memory tests
//! and sinks without environment information keep a useful baseline.
//!
//! ## Line-discipline flags
//!
//! [`Optimizations::TABS`], [`Optimizations::BS`], and
//! [`Optimizations::ONLCR`] describe behavior that often depends on
//! terminal mode as much as terminal type. Override them when raw/cooked
//! mode or output processing differs from the detector's assumptions.

use bitflags::bitflags;

use crate::terminal::Env;

bitflags! {
    /// Terminal capabilities the renderer may use for shorter output.
    ///
    /// # Usage
    ///
    /// Start from a detector result such as [`Optimizations::from_env`]
    /// or a baseline such as [`Optimizations::xterm`], then use the
    /// `with_*` methods to toggle assumptions confirmed by probing or
    /// terminal-mode setup.
    ///
    /// Flag names follow the short capability names used by `infocmp`
    /// where they exist. `BS` and `ONLCR` are not terminfo caps; they
    /// describe control-character and output-processing behavior.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Optimizations: u32 {
        /// Terminal supports ECH (Erase Characters, `CSI Ps X`) for
        /// clearing a run on the current row.
        const ECH    = 1 <<  0;
        /// Terminal supports REP (Repeat preceding character,
        /// `CSI Ps b`) for compact repeated ASCII glyphs.
        const REP    = 1 <<  1;
        /// Terminal supports ICH (Insert Characters, `CSI Ps @`) for
        /// opening cells within a row.
        const ICH    = 1 <<  2;
        /// Terminal supports DCH (Delete Characters, `CSI Ps P`) for
        /// removing cells within a row.
        const DCH    = 1 <<  3;
        /// Terminal supports scroll regions (DECSTBM; terminfo `csr`).
        const CSR    = 1 <<  4;
        /// Terminal supports SU/SD (Scroll Up/Down, `CSI Ps S` /
        /// `CSI Ps T`) for moving full scroll regions.
        const SU_SD  = 1 <<  5;
        /// Terminal supports IL/DL (Insert/Delete Line, `CSI Ps L` /
        /// `CSI Ps M`) for line-level scroll fallbacks.
        const IL_DL  = 1 <<  6;
        /// Terminal supports BCE (Background Color Erase): erase
        /// operations paint with the active background color.
        const BCE    = 1 <<  7;
        /// Terminal supports CHA (Cursor Horizontal Absolute,
        /// `CSI Ps G`) for absolute column moves.
        const CHA    = 1 <<  8;
        /// Terminal supports HPA (Horizontal Position Absolute,
        /// `CSI Ps \``) for absolute column moves.
        const HPA    = 1 <<  9;
        /// Terminal supports VPA (Vertical Position Absolute,
        /// `CSI Ps d`) for absolute row moves.
        const VPA    = 1 << 10;
        /// Literal tab bytes move to configured hardware tab stops.
        const TABS   = 1 << 11;
        /// Terminal supports CBT (Cursor Backward Tab, `CSI Ps Z`).
        const CBT    = 1 << 12;
        /// Terminal supports CHT (Cursor Horizontal Tab, `CSI Ps I`).
        const CHT    = 1 << 13;
        /// Terminal supports BS (the backspace control character,
        /// `\x08`) for cursor-left-by-one.
        const BS     = 1 << 14;
        /// Whether the terminal currently maps `\n` to `\r\n`
        /// (termios ONLCR). In raw mode this is unset and `\n` only
        /// moves the cursor down without resetting the column.
        const ONLCR  = 1 << 15;
    }
}

impl Default for Optimizations {
    /// The default is [`Optimizations::xterm`] — the modern baseline
    /// for the overwhelming majority of terminals reachable from a
    /// generic `TERM=xterm-256color` session.
    fn default() -> Self {
        Self::xterm()
    }
}

impl Optimizations {
    /// Return the most conservative useful capability set.
    ///
    /// Every escape-sequence optimization is disabled; only literal
    /// hardware tabs and backspace remain enabled. Use this for unknown
    /// or genuinely capability-limited terminals when direct cell output
    /// is safer than specialized control sequences.
    pub const fn none() -> Self {
        Self::TABS.union(Self::BS)
    }

    /// Return the modern full-feature baseline.
    ///
    /// Enables every renderer optimization except [`Self::ONLCR`], since
    /// raw mode is the default assumption for terminal applications.
    pub const fn modern() -> Self {
        Self::ECH
            .union(Self::REP)
            .union(Self::ICH)
            .union(Self::DCH)
            .union(Self::CSR)
            .union(Self::SU_SD)
            .union(Self::IL_DL)
            .union(Self::BCE)
            .union(Self::CHA)
            .union(Self::HPA)
            .union(Self::VPA)
            .union(Self::TABS)
            .union(Self::CBT)
            .union(Self::CHT)
            .union(Self::BS)
    }

    /// Return the xterm-compatible conservative baseline.
    ///
    /// Compared to [`Self::modern`], `HPA`,
    /// `CHT`, and `REP` are off:
    /// - `HPA`: konsole and several xterm-compatible terminals lack
    ///   HPA; xterm-256color terminfo defines HPA via the same
    ///   sequence as CHA, so CHA is the safer choice.
    /// - `CHT`: forward-tab support is historically inconsistent
    ///   across xterm-compatible emulators.
    /// - `REP`: REP is not universally implemented across the
    ///   xterm-compatible family.
    pub const fn xterm() -> Self {
        Self::modern().difference(Self::HPA.union(Self::CHT).union(Self::REP))
    }

    /// Return the VT100/VT102 baseline.
    ///
    /// Predates the xterm extensions for
    /// absolute positioning (CHA/HPA/VPA), ECH, REP, BCE, SU/SD, and
    /// CBT, but supports DECSTBM, hardware tabs, BS, and on the
    /// VT102 the ICH/DCH/IL/DL editing pairs.
    pub const fn vt100() -> Self {
        Self::ICH
            .union(Self::DCH)
            .union(Self::CSR)
            .union(Self::IL_DL)
            .union(Self::TABS)
            .union(Self::BS)
    }

    /// Return the Linux console baseline.
    ///
    /// The kernel's terminal driver
    /// implements a narrow subset of ECMA-48 — only absolute
    /// positioning (CHA/HPA/VPA), ECH, and ICH on top of the
    /// hardware tab stops and BS handled by termios.
    /// See `console_codes(4)`.
    pub const fn linux() -> Self {
        Self::ECH
            .union(Self::ICH)
            .union(Self::CHA)
            .union(Self::HPA)
            .union(Self::VPA)
            .union(Self::TABS)
            .union(Self::BS)
    }

    /// Return the GNU screen baseline, derived from
    /// `infocmp -x1 screen-256color`. screen multiplexes onto the
    /// host terminal and only re-advertises a conservative subset:
    /// no `BCE`, `ECH`, `REP`, `CHA`, or `CHT`.
    pub const fn screen() -> Self {
        Self::ICH
            .union(Self::DCH)
            .union(Self::CSR)
            .union(Self::SU_SD)
            .union(Self::IL_DL)
            .union(Self::HPA)
            .union(Self::VPA)
            .union(Self::TABS)
            .union(Self::CBT)
            .union(Self::BS)
    }

    /// Return `self` with hardware tab support (`TABS`) toggled.
    ///
    /// Disable when the
    /// receiving terminal is in cooked mode without `TAB0` set on
    /// `c_oflag` and `\t` would otherwise be expanded to spaces.
    #[must_use]
    pub const fn with_tabs(self, enabled: bool) -> Self {
        self.with_flag(Self::TABS, enabled)
    }

    /// Return `self` with backspace-character support (`BS`) toggled.
    ///
    /// Disable when the
    /// receiving terminal does not interpret `\x08` as cursor-left
    /// by one cell.
    #[must_use]
    pub const fn with_bs(self, enabled: bool) -> Self {
        self.with_flag(Self::BS, enabled)
    }

    /// Return `self` with the `\n` → `\r\n` assumption (`ONLCR`)
    /// toggled.
    ///
    /// Enable when the terminal is in cooked mode with `ONLCR` set so a
    /// newline both advances a row and resets the column.
    #[must_use]
    pub const fn with_onlcr(self, enabled: bool) -> Self {
        self.with_flag(Self::ONLCR, enabled)
    }

    /// Return `self` with erase-character (`ECH`) support toggled.
    #[must_use]
    pub const fn with_ech(self, enabled: bool) -> Self {
        self.with_flag(Self::ECH, enabled)
    }

    /// Return `self` with repeat-character (`REP`) support toggled.
    #[must_use]
    pub const fn with_rep(self, enabled: bool) -> Self {
        self.with_flag(Self::REP, enabled)
    }

    /// Return `self` with insert-character (`ICH`) support toggled.
    #[must_use]
    pub const fn with_ich(self, enabled: bool) -> Self {
        self.with_flag(Self::ICH, enabled)
    }

    /// Return `self` with delete-character (`DCH`) support toggled.
    #[must_use]
    pub const fn with_dch(self, enabled: bool) -> Self {
        self.with_flag(Self::DCH, enabled)
    }

    /// Return `self` with DECSTBM scroll-region (`CSR`) support toggled.
    #[must_use]
    pub const fn with_csr(self, enabled: bool) -> Self {
        self.with_flag(Self::CSR, enabled)
    }

    /// Return `self` with scroll-up/scroll-down (`SU_SD`) support toggled.
    #[must_use]
    pub const fn with_su_sd(self, enabled: bool) -> Self {
        self.with_flag(Self::SU_SD, enabled)
    }

    /// Return `self` with insert/delete-line (`IL_DL`) support toggled.
    #[must_use]
    pub const fn with_il_dl(self, enabled: bool) -> Self {
        self.with_flag(Self::IL_DL, enabled)
    }

    /// Return `self` with background-color-erase (`BCE`) support toggled.
    #[must_use]
    pub const fn with_bce(self, enabled: bool) -> Self {
        self.with_flag(Self::BCE, enabled)
    }

    /// Return `self` with cursor-horizontal-absolute (`CHA`) support toggled.
    #[must_use]
    pub const fn with_cha(self, enabled: bool) -> Self {
        self.with_flag(Self::CHA, enabled)
    }

    /// Return `self` with horizontal-position-absolute (`HPA`) support toggled.
    #[must_use]
    pub const fn with_hpa(self, enabled: bool) -> Self {
        self.with_flag(Self::HPA, enabled)
    }

    /// Return `self` with vertical-position-absolute (`VPA`) support toggled.
    #[must_use]
    pub const fn with_vpa(self, enabled: bool) -> Self {
        self.with_flag(Self::VPA, enabled)
    }

    /// Return `self` with cursor-backward-tab (`CBT`) support toggled.
    #[must_use]
    pub const fn with_cbt(self, enabled: bool) -> Self {
        self.with_flag(Self::CBT, enabled)
    }

    /// Return `self` with cursor-horizontal-tab (`CHT`) support toggled.
    #[must_use]
    pub const fn with_cht(self, enabled: bool) -> Self {
        self.with_flag(Self::CHT, enabled)
    }

    /// Const-friendly helper used by the `with_*` builders.
    const fn with_flag(self, flag: Self, enabled: bool) -> Self {
        if enabled {
            self.union(flag)
        } else {
            self.difference(flag)
        }
    }

    /// Derive an optimization set from a `TERM` value.
    ///
    /// # Parameters
    ///
    /// - `term`: terminal name, usually from `$TERM`.
    ///
    /// # Returns
    ///
    /// A baseline capability set for the terminal family. Unknown,
    /// empty, and `dumb` values return [`Self::none`].
    pub fn from_term(term: &str) -> Self {
        let head = term.split('-').next().unwrap_or("");
        // xterm-<vendor> reassignment when the vendor is a known modern
        // terminal advertising xterm compatibility.
        if head == "xterm"
            && let Some(rest) = term.strip_prefix("xterm-")
        {
            let vendor = rest.split('-').next().unwrap_or("");
            if matches!(vendor, "ghostty" | "kitty" | "rio") {
                return Self::modern();
            }
        }
        match head {
            "" | "dumb" => Self::none(),
            // Modern terminals that advertise the full xterm-era cap
            // set. Alacritty falls into this bucket too; the detector
            // uses the shared modern baseline rather than maintaining a
            // vendor-specific single-flag variant here.
            "alacritty" | "contour" | "foot" | "ghostty" | "kitty" | "rio" | "st" | "tmux"
            | "wezterm" => Self::modern(),
            "xterm" => Self::xterm(),
            "screen" => Self::screen(),
            "linux" => Self::linux(),
            _ => Self::none(),
        }
    }

    /// Derive an optimization set from an [`Env`].
    ///
    /// Routes `$TERM` through [`Self::from_term`] when it is set, and
    /// falls back to [`Self::default`] when `$TERM` is unset entirely.
    /// This keeps callers with no environment information (CI harnesses,
    /// embedded sinks, tests) on the xterm baseline rather than
    /// collapsing to [`Self::none`].
    pub fn from_env(env: &Env) -> Self {
        match env.get("TERM") {
            Some(term) => Self::from_term(&term),
            None => Self::default(),
        }
    }

    /// Report whether `env` names a terminal known to implement DECST8C,
    /// the `ESC [ ? 5 W` sequence that resets tab stops to one every
    /// eight columns in a single, cursor-safe write.
    ///
    /// The allowlist covers Ghostty, kitty, Rio, Alacritty, iTerm2, and
    /// Windows Terminal. Terminals outside it fall back to the portable
    /// TBC-then-HTS reset, so a false negative only costs a few extra
    /// bytes, never correctness. This inspects the environment only and
    /// never probes the terminal.
    pub(crate) fn supports_decst8c(env: &Env) -> bool {
        // Windows Terminal announces itself with a session token rather
        // than through $TERM.
        if env.has("WT_SESSION") {
            return true;
        }
        // kitty, Alacritty, and iTerm2 export a session/window id that
        // survives login shells and multiplexers rewriting $TERM.
        if env.has("KITTY_WINDOW_ID")
            || env.has("ALACRITTY_WINDOW_ID")
            || env.has("ITERM_SESSION_ID")
        {
            return true;
        }
        // iTerm2 sets $LC_TERMINAL, which also propagates across ssh.
        if env
            .get("LC_TERMINAL")
            .is_some_and(|t| t.eq_ignore_ascii_case("iterm2"))
        {
            return true;
        }
        // $TERM_PROGRAM outlives some xterm-compatible $TERM rewrites.
        if let Some(program) = env.get("TERM_PROGRAM")
            && matches!(
                program.to_ascii_lowercase().as_str(),
                "ghostty" | "rio" | "iterm.app"
            )
        {
            return true;
        }
        let Some(term) = env.get("TERM") else {
            return false;
        };
        // Match the bare vendor head and the xterm-<vendor> promotion
        // form, mirroring how `from_term` resolves these terminals.
        let head = term.split('-').next().unwrap_or("");
        if matches!(head, "alacritty" | "ghostty" | "kitty" | "rio") {
            return true;
        }
        if let Some(rest) = term.strip_prefix("xterm-") {
            let vendor = rest.split('-').next().unwrap_or("");
            return matches!(vendor, "ghostty" | "kitty" | "rio");
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(pairs: &[(&str, &str)]) -> Env {
        Env::from_pairs(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())))
    }

    #[test]
    fn supports_decst8c_matches_allowlisted_terms() {
        for term in [
            "alacritty",
            "ghostty",
            "kitty",
            "rio",
            "xterm-kitty",
            "xterm-ghostty",
        ] {
            assert!(
                Optimizations::supports_decst8c(&env_with(&[("TERM", term)])),
                "expected DECST8C support for TERM={term}",
            );
        }
    }

    #[test]
    fn supports_decst8c_rejects_plain_xterm_and_unknown() {
        for term in [
            "xterm-256color",
            "screen-256color",
            "tmux",
            "vt100",
            "dumb",
            "",
        ] {
            assert!(
                !Optimizations::supports_decst8c(&env_with(&[("TERM", term)])),
                "did not expect DECST8C support for TERM={term}",
            );
        }
    }

    #[test]
    fn supports_decst8c_detects_terminals_via_env_tokens() {
        // Windows Terminal and window-id exporters are recognized even when
        // $TERM is rewritten to a generic value.
        assert!(Optimizations::supports_decst8c(&env_with(&[
            ("TERM", "xterm-256color"),
            ("WT_SESSION", "abc"),
        ])));
        assert!(Optimizations::supports_decst8c(&env_with(&[
            ("TERM", "xterm-256color"),
            ("KITTY_WINDOW_ID", "1"),
        ])));
        assert!(Optimizations::supports_decst8c(&env_with(&[
            ("TERM", "xterm-256color"),
            ("ALACRITTY_WINDOW_ID", "1"),
        ])));
        assert!(Optimizations::supports_decst8c(&env_with(&[
            ("TERM", "screen"),
            ("TERM_PROGRAM", "ghostty"),
        ])));
        // iTerm2 keeps $TERM generic and identifies itself through its own
        // tokens, including $LC_TERMINAL which survives ssh.
        assert!(Optimizations::supports_decst8c(&env_with(&[
            ("TERM", "xterm-256color"),
            ("TERM_PROGRAM", "iTerm.app"),
        ])));
        assert!(Optimizations::supports_decst8c(&env_with(&[
            ("TERM", "xterm-256color"),
            ("ITERM_SESSION_ID", "w0t0p0:abc"),
        ])));
        assert!(Optimizations::supports_decst8c(&env_with(&[
            ("TERM", "xterm-256color"),
            ("LC_TERMINAL", "iTerm2"),
        ])));
    }

    #[test]
    fn default_is_xterm() {
        assert_eq!(Optimizations::default(), Optimizations::xterm());
    }

    #[test]
    fn none_disables_escape_caps_only() {
        let o = Optimizations::none();
        assert!(!o.intersects(
            Optimizations::ECH
                | Optimizations::REP
                | Optimizations::ICH
                | Optimizations::DCH
                | Optimizations::CSR
                | Optimizations::SU_SD
                | Optimizations::IL_DL
                | Optimizations::BCE
                | Optimizations::CHA
                | Optimizations::HPA
                | Optimizations::VPA
                | Optimizations::CBT
                | Optimizations::CHT
                | Optimizations::ONLCR,
        ));
        // Termios-gated stays on.
        assert!(o.contains(Optimizations::TABS));
        assert!(o.contains(Optimizations::BS));
    }

    #[test]
    fn modern_enables_everything_except_onlcr() {
        let o = Optimizations::modern();
        let everything_but_onlcr = Optimizations::all().difference(Optimizations::ONLCR);
        assert_eq!(o, everything_but_onlcr);
    }

    #[test]
    fn xterm_drops_hpa_cht_rep() {
        let o = Optimizations::xterm();
        assert!(!o.contains(Optimizations::HPA));
        assert!(!o.contains(Optimizations::CHT));
        assert!(!o.contains(Optimizations::REP));
        let expected = Optimizations::modern()
            .difference(Optimizations::HPA | Optimizations::CHT | Optimizations::REP);
        assert_eq!(o, expected);
    }

    #[test]
    fn vt100_predates_xterm_extensions() {
        let o = Optimizations::vt100();
        // No xterm-era absolute positioning or extensions.
        let missing = Optimizations::CHA
            | Optimizations::HPA
            | Optimizations::VPA
            | Optimizations::ECH
            | Optimizations::REP
            | Optimizations::SU_SD
            | Optimizations::BCE
            | Optimizations::CBT
            | Optimizations::CHT
            | Optimizations::ONLCR;
        assert!(!o.intersects(missing));
        // VT100 era margins + VT102 editing pairs.
        let present = Optimizations::CSR
            | Optimizations::ICH
            | Optimizations::DCH
            | Optimizations::IL_DL
            | Optimizations::TABS
            | Optimizations::BS;
        assert!(o.contains(present));
    }

    #[test]
    fn linux_matches_console_codes_4() {
        let o = Optimizations::linux();
        let present = Optimizations::ECH
            | Optimizations::ICH
            | Optimizations::CHA
            | Optimizations::HPA
            | Optimizations::VPA
            | Optimizations::TABS
            | Optimizations::BS;
        assert_eq!(o, present);
    }

    #[test]
    fn screen_matches_infocmp_x1_screen_256color() {
        let o = Optimizations::screen();
        let present = Optimizations::ICH
            | Optimizations::DCH
            | Optimizations::CSR
            | Optimizations::SU_SD
            | Optimizations::IL_DL
            | Optimizations::HPA
            | Optimizations::VPA
            | Optimizations::TABS
            | Optimizations::CBT
            | Optimizations::BS;
        assert_eq!(o, present);
        assert!(!o.contains(Optimizations::BCE));
        assert!(!o.contains(Optimizations::ECH));
        assert!(!o.contains(Optimizations::REP));
        assert!(!o.contains(Optimizations::CHA));
        assert!(!o.contains(Optimizations::CHT));
    }

    #[test]
    fn from_term_kitty() {
        let o = Optimizations::from_term("kitty");
        assert!(o.contains(Optimizations::REP | Optimizations::HPA | Optimizations::VPA));
    }

    #[test]
    fn from_term_xterm_excludes_hpa_cht_rep() {
        let o = Optimizations::from_term("xterm-256color");
        assert!(!o.contains(Optimizations::HPA));
        assert!(!o.contains(Optimizations::CHT));
        assert!(!o.contains(Optimizations::REP));
        assert!(o.contains(Optimizations::CHA));
    }

    #[test]
    fn from_term_xterm_kitty_promotes() {
        let o = Optimizations::from_term("xterm-kitty");
        assert!(o.contains(Optimizations::HPA | Optimizations::REP));
    }

    #[test]
    fn from_term_xterm_ghostty_promotes() {
        let o = Optimizations::from_term("xterm-ghostty");
        assert!(o.contains(Optimizations::HPA | Optimizations::REP));
    }

    #[test]
    fn from_term_xterm_rio_promotes() {
        let o = Optimizations::from_term("xterm-rio");
        assert!(o.contains(Optimizations::HPA | Optimizations::REP));
    }

    #[test]
    fn from_term_linux_console() {
        assert_eq!(Optimizations::from_term("linux"), Optimizations::linux());
    }

    #[test]
    fn from_term_dumb_is_none() {
        assert_eq!(Optimizations::from_term("dumb"), Optimizations::none());
    }

    #[test]
    fn from_term_empty_is_none() {
        assert_eq!(Optimizations::from_term(""), Optimizations::none());
    }

    #[test]
    fn from_term_unknown_falls_back_to_none() {
        assert_eq!(
            Optimizations::from_term("madeupterm-256color"),
            Optimizations::none(),
        );
    }

    #[test]
    fn xterm_term_enables_cha() {
        let o = Optimizations::from_term("xterm-256color");
        assert!(o.contains(Optimizations::CHA));
        assert!(!o.contains(Optimizations::HPA));
    }

    #[test]
    fn linux_term_supports_vpa_hpa_not_rep() {
        let o = Optimizations::from_term("linux");
        assert!(o.contains(Optimizations::VPA | Optimizations::HPA));
        assert!(!o.contains(Optimizations::REP));
    }

    #[test]
    fn alacritty_term_has_explicit_caps() {
        let o = Optimizations::from_term("alacritty");
        assert!(o.contains(Optimizations::CHA | Optimizations::ECH | Optimizations::REP));
    }

    #[test]
    fn screen_term_uses_screen_profile() {
        assert_eq!(
            Optimizations::from_term("screen-256color"),
            Optimizations::screen(),
        );
    }

    #[test]
    fn tmux_term_supports_vpa() {
        assert!(Optimizations::from_term("tmux-256color").contains(Optimizations::VPA));
    }

    #[test]
    fn from_term_modern_families_all_enable_rep() {
        for term in [
            "contour",
            "foot",
            "ghostty",
            "kitty",
            "rio",
            "st",
            "tmux",
            "wezterm",
            "alacritty",
        ] {
            let o = Optimizations::from_term(term);
            assert!(
                o.contains(Optimizations::REP | Optimizations::HPA),
                "{term} should enable REP and HPA"
            );
        }
    }

    #[test]
    fn with_tabs_toggles_tabs() {
        let on = Optimizations::none().with_tabs(true);
        let off = Optimizations::none().with_tabs(false);
        assert!(on.contains(Optimizations::TABS));
        assert!(!off.contains(Optimizations::TABS));
    }

    #[test]
    fn with_bs_toggles_bs() {
        let on = Optimizations::none().with_bs(true);
        let off = Optimizations::none().with_bs(false);
        assert!(on.contains(Optimizations::BS));
        assert!(!off.contains(Optimizations::BS));
    }

    #[test]
    fn with_onlcr_toggles_onlcr() {
        let on = Optimizations::none().with_onlcr(true);
        let off = Optimizations::modern().with_onlcr(false);
        assert!(on.contains(Optimizations::ONLCR));
        assert!(!off.contains(Optimizations::ONLCR));
    }

    #[test]
    fn with_builders_compose() {
        let o = Optimizations::xterm()
            .with_rep(true)
            .with_hpa(true)
            .with_cha(false);
        assert!(o.contains(Optimizations::REP | Optimizations::HPA));
        assert!(!o.contains(Optimizations::CHA));
    }

    #[test]
    fn from_env_uses_term_when_set() {
        let env = Env::from_pairs([("TERM", "xterm-kitty")]);
        assert_eq!(Optimizations::from_env(&env), Optimizations::modern());
    }

    #[test]
    fn from_env_dumb_collapses_to_none() {
        let env = Env::from_pairs([("TERM", "dumb")]);
        assert_eq!(Optimizations::from_env(&env), Optimizations::none());
    }

    #[test]
    fn from_env_empty_term_collapses_to_none() {
        let env = Env::from_pairs([("TERM", "")]);
        assert_eq!(Optimizations::from_env(&env), Optimizations::none());
    }

    #[test]
    fn from_env_missing_term_falls_back_to_default() {
        let env = Env::new();
        assert_eq!(Optimizations::from_env(&env), Optimizations::default());
    }

    #[test]
    fn with_builders_are_idempotent() {
        let a = Optimizations::modern().with_tabs(true).with_tabs(true);
        let b = Optimizations::modern().with_tabs(true);
        assert_eq!(a, b);
        let c = Optimizations::modern().with_rep(false).with_rep(false);
        let d = Optimizations::modern().with_rep(false);
        assert_eq!(c, d);
    }
}
