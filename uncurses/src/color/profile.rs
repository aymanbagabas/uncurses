//! Terminal color profile detection and color downsampling.
//!
//! Detection consults a number of environment variables and TERM heuristics:
//!
//! * `NO_COLOR` — disables color entirely (text decoration still allowed).
//!   See <https://no-color.org/>.
//! * `CLICOLOR` / `CLICOLOR_FORCE` — opt in to / force colors.
//!   See <https://bixense.com/clicolors/>.
//! * `COLORTERM=truecolor|24bit|yes|true` — upgrade to TrueColor.
//! * `TERM` — terminal-database name; `dumb` disables, `*-256color` →
//!   ANSI256, `*-direct` → TrueColor, and a few known-good terminals are
//!   recognized by name.
//! * `WT_SESSION` — Windows Terminal advertises itself this way → TrueColor.
//! * `GOOGLE_CLOUD_SHELL` → TrueColor.
//! * `CI` — CI runners render ANSI color in logs → TrueColor.

use super::Color;
use crate::terminal::Env;

/// Terminal color capability profile.
///
/// Ordered: variants compare as `Disabled < Ascii < Ansi < Ansi256 < TrueColor`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Profile {
    /// No styling output at all — used when the output is not a TTY or
    /// when `TERM` is empty / `dumb`, where SGR escape sequences would
    /// be rendered as literal text.
    Disabled,
    /// ASCII only — no color, decoration allowed.
    Ascii,
    /// 16 ANSI colors.
    Ansi,
    /// 256 xterm colors.
    Ansi256,
    /// 24-bit true color.
    #[default]
    TrueColor,
}

impl Profile {
    /// Downsample a color to fit this profile.
    pub fn convert(self, color: Color) -> Option<Color> {
        use super::convert::*;
        match self {
            Profile::Disabled | Profile::Ascii => None,
            Profile::Ansi => Some(rgb_to_ansi16(color.to_rgb())),
            Profile::Ansi256 => Some(rgb_to_ansi256(color.to_rgb())),
            Profile::TrueColor => Some(color),
        }
    }

    /// Detect the profile from the current process environment, assuming
    /// the output is a TTY.
    ///
    /// For testability, see [`Profile::detect_from`].
    pub fn detect() -> Self {
        let env = Env::from_process();
        Self::detect_from(&env, true)
    }

    /// Detect the profile from an explicit environment snapshot.
    ///
    /// `is_tty` should be true if the output stream is a terminal. When
    /// false, or when `TERM` indicates no styling capability (empty or
    /// `dumb`), the result is clamped to [`Profile::Disabled`]
    /// unless `CLICOLOR_FORCE` (or `TTY_FORCE`) overrides it.
    pub fn detect_from(env: &Env, is_tty: bool) -> Self {
        let is_tty = is_tty || env.bool("TTY_FORCE");
        let term = env.get("TERM").unwrap_or_default();

        // `env_color_profile` is responsible for translating the
        // environment to a profile, including the empty-or-`dumb` TERM
        // case: on Unix that means Disabled, on Windows it falls back
        // to a platform-specific probe (e.g. WT_SESSION → TrueColor)
        // because Windows shells routinely leave TERM unset. The only
        // unconditional clamp here is non-TTY output.
        let envp = env_color_profile(env, &term);
        let mut p = if !is_tty { Profile::Disabled } else { envp };

        // NO_COLOR: clamp to Ascii (decoration still allowed).
        if env.bool("NO_COLOR") && is_tty {
            if p > Profile::Ascii {
                p = Profile::Ascii;
            }
            return p;
        }

        // CLICOLOR_FORCE: at least Ansi, take max of env-derived.
        if env.bool("CLICOLOR_FORCE") {
            if p < Profile::Ansi {
                p = Profile::Ansi;
            }
            if envp > p {
                p = envp;
            }
            return p;
        }

        let is_dumb = term.is_empty() || term == DUMB_TERM;
        // CLICOLOR: bump non-dumb TTY to at least Ansi.
        if env.bool("CLICOLOR") && is_tty && !is_dumb && p < Profile::Ansi {
            p = Profile::Ansi;
        }

        p
    }
}

const DUMB_TERM: &str = "dumb";

/// Environment-driven profile inference. Knows nothing about TTY-ness.
fn env_color_profile(env: &Env, term: &str) -> Profile {
    let mut p = if term == DUMB_TERM {
        // An explicit `dumb` terminal opts out of styling everywhere.
        Profile::Disabled
    } else if term.is_empty() {
        // On Windows, the lack of TERM is normal — Windows Terminal and
        // cmd.exe don't set it. Defer to a Windows-specific fallback when
        // we know we're on Windows; otherwise treat as Disabled.
        #[cfg(windows)]
        {
            windows_color_profile(env).unwrap_or(Profile::Disabled)
        }
        #[cfg(not(windows))]
        {
            let _ = env;
            Profile::Disabled
        }
    } else {
        Profile::Ansi
    };

    // Known-good terminals: full TrueColor.
    if KNOWN_TRUECOLOR_TERMS.iter().any(|t| term.contains(t)) {
        return Profile::TrueColor;
    }

    if term.starts_with("tmux") || term.starts_with("screen") {
        if p < Profile::Ansi256 {
            p = Profile::Ansi256;
        }
    } else if term.starts_with("xterm") && p < Profile::Ansi {
        p = Profile::Ansi;
    }

    // Windows Terminal session variable — set even when TERM isn't.
    if env.has("WT_SESSION") {
        return Profile::TrueColor;
    }

    if env.bool("GOOGLE_CLOUD_SHELL") {
        return Profile::TrueColor;
    }

    // CI runners advertise themselves with CI=true and render ANSI
    // color in their logs even though TERM is usually unset or `dumb`.
    if env.bool("CI") {
        return Profile::TrueColor;
    }

    // COLORTERM upgrades to TrueColor, except inside screen which
    // doesn't propagate it. Modern tmux (3.2+) forwards COLORTERM to
    // its panes, so we honour it there.
    if colorterm_says_truecolor(env) && !term.starts_with("screen") {
        return Profile::TrueColor;
    }

    if term.ends_with("256color") && p < Profile::Ansi256 {
        p = Profile::Ansi256;
    }

    if term.ends_with("direct") {
        return Profile::TrueColor;
    }

    p
}

/// Terminals known to support TrueColor regardless of `TERM` suffix.
const KNOWN_TRUECOLOR_TERMS: &[&str] = &[
    "alacritty",
    "contour",
    "foot",
    "ghostty",
    "kitty",
    "rio",
    "st",
    "wezterm",
];

fn colorterm_says_truecolor(env: &Env) -> bool {
    let v = env
        .get("COLORTERM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(v.as_str(), "truecolor" | "24bit" | "yes" | "true")
}

#[cfg(windows)]
fn windows_color_profile(env: &Env) -> Option<Profile> {
    // Windows 10+ conhost and Windows Terminal both support virtual
    // terminal sequences. WT_SESSION pins TrueColor; otherwise assume
    // ANSI256 (conhost's legacy floor) — TrueColor support arrived in
    // build 14931 and is universally available on supported Windows
    // versions, but we stay conservative without further probing.
    if env.has("WT_SESSION") {
        return Some(Profile::TrueColor);
    }
    Some(Profile::Ansi256)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::from_pairs(pairs.iter().map(|(k, v)| (*k, *v)))
    }

    #[test]
    fn default_is_truecolor() {
        assert_eq!(Profile::default(), Profile::TrueColor);
    }

    #[test]
    fn ordering_is_capability_ascending() {
        assert!(Profile::Disabled < Profile::Ascii);
        assert!(Profile::Ascii < Profile::Ansi);
        assert!(Profile::Ansi < Profile::Ansi256);
        assert!(Profile::Ansi256 < Profile::TrueColor);
    }

    #[test]
    fn no_tty_clamps_to_notty() {
        let e = env(&[("TERM", "xterm-256color")]);
        assert_eq!(Profile::detect_from(&e, false), Profile::Disabled);
    }

    #[test]
    fn dumb_term_is_notty() {
        let e = env(&[("TERM", "dumb")]);
        assert_eq!(Profile::detect_from(&e, true), Profile::Disabled);
    }

    #[test]
    fn no_color_clamps_to_ascii() {
        let e = env(&[("TERM", "xterm-256color"), ("NO_COLOR", "1")]);
        assert_eq!(Profile::detect_from(&e, true), Profile::Ascii);
    }

    #[test]
    fn no_color_does_not_apply_off_tty() {
        // off-tty + no_color: still Disabled (no_color clamp only applies when isatty).
        let e = env(&[("TERM", "xterm-256color"), ("NO_COLOR", "1")]);
        assert_eq!(Profile::detect_from(&e, false), Profile::Disabled);
    }

    #[test]
    fn clicolor_force_overrides_notty() {
        let e = env(&[("TERM", "dumb"), ("CLICOLOR_FORCE", "1")]);
        // CLICOLOR_FORCE guarantees at least Ansi; the platform/env floor may
        // be higher (e.g. Ansi256 on Windows conhost).
        assert!(Profile::detect_from(&e, true) >= Profile::Ansi);
    }

    #[test]
    fn clicolor_bumps_to_ansi_on_tty() {
        // No TERM at all on a unix TTY would normally be Disabled.
        let e = env(&[("CLICOLOR", "1"), ("TERM", "screen")]);
        let p = Profile::detect_from(&e, true);
        assert!(p >= Profile::Ansi);
    }

    #[test]
    fn colorterm_truecolor_upgrades() {
        let e = env(&[("TERM", "xterm"), ("COLORTERM", "truecolor")]);
        assert_eq!(Profile::detect_from(&e, true), Profile::TrueColor);
    }

    #[test]
    fn colorterm_24bit_upgrades() {
        let e = env(&[("TERM", "xterm"), ("COLORTERM", "24bit")]);
        assert_eq!(Profile::detect_from(&e, true), Profile::TrueColor);
    }

    #[test]
    fn colorterm_does_not_upgrade_inside_screen() {
        let e = env(&[("TERM", "screen-256color"), ("COLORTERM", "truecolor")]);
        // screen does not forward COLORTERM-derived TrueColor.
        let p = Profile::detect_from(&e, true);
        assert!(p < Profile::TrueColor);
    }

    #[test]
    fn colorterm_upgrades_inside_tmux() {
        let e = env(&[("TERM", "tmux-256color"), ("COLORTERM", "truecolor")]);
        // Modern tmux forwards COLORTERM, so the upgrade applies.
        assert_eq!(Profile::detect_from(&e, true), Profile::TrueColor);
    }

    #[test]
    fn known_terminal_is_truecolor() {
        for name in [
            "alacritty",
            "wezterm",
            "ghostty",
            "kitty-direct",
            "xterm-kitty",
        ] {
            let e = env(&[("TERM", name)]);
            assert_eq!(
                Profile::detect_from(&e, true),
                Profile::TrueColor,
                "TERM={name} should detect as TrueColor",
            );
        }
    }

    #[test]
    fn term_256color_suffix_is_ansi256() {
        let e = env(&[("TERM", "xterm-256color")]);
        assert_eq!(Profile::detect_from(&e, true), Profile::Ansi256);
    }

    #[test]
    fn term_direct_suffix_is_truecolor() {
        let e = env(&[("TERM", "tmux-direct")]);
        assert_eq!(Profile::detect_from(&e, true), Profile::TrueColor);
    }

    #[test]
    fn screen_floor_is_ansi256() {
        let e = env(&[("TERM", "screen")]);
        assert_eq!(Profile::detect_from(&e, true), Profile::Ansi256);
    }

    #[test]
    fn tmux_floor_is_ansi256() {
        let e = env(&[("TERM", "tmux")]);
        assert_eq!(Profile::detect_from(&e, true), Profile::Ansi256);
    }

    #[test]
    fn wt_session_implies_truecolor() {
        let e = env(&[("TERM", "xterm"), ("WT_SESSION", "1234")]);
        assert_eq!(Profile::detect_from(&e, true), Profile::TrueColor);
    }

    #[cfg(windows)]
    #[test]
    fn wt_session_without_term_is_truecolor_on_windows() {
        // Windows shells (PowerShell, cmd, Windows Terminal) routinely
        // leave TERM unset. WT_SESSION must still pin TrueColor.
        let e = env(&[("WT_SESSION", "1234")]);
        assert_eq!(Profile::detect_from(&e, true), Profile::TrueColor);
    }

    #[cfg(windows)]
    #[test]
    fn empty_term_falls_back_to_ansi256_on_windows() {
        // Without WT_SESSION, conhost still supports VT sequences on
        // supported Windows versions; floor is Ansi256.
        let e = env(&[]);
        assert_eq!(Profile::detect_from(&e, true), Profile::Ansi256);
    }

    #[test]
    fn google_cloud_shell_implies_truecolor() {
        let e = env(&[("GOOGLE_CLOUD_SHELL", "true"), ("TERM", "xterm")]);
        assert_eq!(Profile::detect_from(&e, true), Profile::TrueColor);
    }

    #[test]
    fn ci_implies_truecolor() {
        let e = env(&[("CI", "true"), ("TERM", "dumb")]);
        assert_eq!(Profile::detect_from(&e, true), Profile::TrueColor);
    }

    #[test]
    fn xterm_plain_is_ansi() {
        let e = env(&[("TERM", "xterm")]);
        assert_eq!(Profile::detect_from(&e, true), Profile::Ansi);
    }

    #[test]
    fn env_bool_parses_truthy_values() {
        let cases = [
            ("1", true),
            ("0", false),
            ("true", true),
            ("True", true),
            ("TRUE", true),
            ("t", true),
            ("T", true),
            ("false", false),
            ("False", false),
            ("FALSE", false),
            ("", false),
            ("yes", false),
            ("garbage", false),
        ];
        for (v, want) in cases {
            let e = env(&[("X", v)]);
            assert_eq!(e.bool("X"), want, "bool({v:?})");
        }
    }

    #[test]
    fn convert_to_each_profile() {
        let red = Color::Rgb(255, 0, 0);
        assert!(Profile::Disabled.convert(red).is_none());
        assert!(Profile::Ascii.convert(red).is_none());
        assert!(Profile::Ansi.convert(red).is_some());
        assert!(Profile::Ansi256.convert(red).is_some());
        assert_eq!(Profile::TrueColor.convert(red), Some(red));
    }
}
