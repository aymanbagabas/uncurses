//! Terminal graphics capability detection.

/// Graphics support detected from terminal replies and environment hints.
///
/// Use [`detect`](Self::detect) with the program's capabilities and environment.
/// Unknown capabilities stay false. Applications can override these flags.
/// Sixel also requires cursor-relative drawing and character erasure of graphics
/// in the active terminal mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImageCapabilities {
    /// Kitty graphics support.
    ///
    /// Automatic selection assumes Unicode placeholder support when the
    /// destination dimensions and color profile permit it.
    /// Select `Protocol::Kitty` for native-only terminals.
    pub kitty: bool,
    pub iterm2: bool,
    pub sixel: bool,
}

impl From<&uncurses::program::Capabilities> for ImageCapabilities {
    /// Read Kitty, Sixel, and XTVERSION support hints from terminal replies.
    /// Use [`Self::detect`] to include environment detection.
    fn from(capabilities: &uncurses::program::Capabilities) -> Self {
        let named = terminal_protocols(capabilities.terminal_name().unwrap_or(""));
        Self {
            kitty: capabilities.kitty_graphics() || named.kitty,
            sixel: capabilities
                .primary_device_attributes()
                .is_some_and(supports_sixel),
            iterm2: named.iterm2,
        }
    }
}

impl ImageCapabilities {
    /// Combine terminal replies with the supplied environment.
    ///
    /// Pass `program.capabilities()` and `program.env()`. Call again after
    /// capability replies arrive. This method performs no terminal I/O.
    ///
    /// Kitty support comes from a graphics reply or terminal identity hints.
    /// Sixel support comes from
    /// capability `4` after a VT220-or-later DA1 service class (`62` through `65`).
    /// The service class itself is not a capability.
    ///
    /// XTVERSION, `TERM_PROGRAM`, and `LC_TERMINAL` identify these families:
    ///
    /// | Family | Kitty | iTerm2 |
    /// |---|---|---|
    /// | kitty, Ghostty | Yes | No |
    /// | Rio, WezTerm, Konsole | Yes | Yes |
    /// | iTerm2 / iTerm.app, mintty | No | Yes |
    ///
    /// Names are case-insensitive. A version after whitespace or `(` is ignored.
    /// `TERM` recognizes the same families, with an optional `xterm-` prefix
    /// and a suffix such as `-256color`.
    /// Nonempty `KITTY_WINDOW_ID`, `ITERM_SESSION_ID`, and `WEZTERM_EXECUTABLE`
    /// identify kitty, iTerm2, and WezTerm respectively.
    ///
    /// Environment hints identify the terminal, not its current settings or
    /// multiplexer transport. Override flags when that environment differs
    /// from the active connection. Sixel mode requirements still apply.
    ///
    /// ```
    /// use uncurses::program::Capabilities;
    /// use uncurses::terminal::EnvList;
    /// use uncurses_image::ImageCapabilities;
    ///
    /// let env = EnvList::from_pairs([("TERM_PROGRAM", "iTerm.app")]);
    /// let detected = ImageCapabilities::detect(&Capabilities::default(), &env);
    /// assert!(detected.iterm2);
    /// ```
    pub fn detect(
        capabilities: &uncurses::program::Capabilities,
        env: &dyn uncurses::terminal::Env,
    ) -> Self {
        let mut detected = Self::from(capabilities);
        let wezterm = env.has("WEZTERM_EXECUTABLE");
        let kitty = env.has("KITTY_WINDOW_ID");
        detected.kitty |= kitty || wezterm;
        detected.iterm2 |= env.has("ITERM_SESSION_ID") || wezterm;
        for key in ["TERM_PROGRAM", "LC_TERMINAL", "TERM"] {
            let Some(name) = env.get(key) else { continue };
            let name = name.to_ascii_lowercase();
            let family = if key == "TERM" {
                name.strip_prefix("xterm-")
                    .unwrap_or(&name)
                    .split('-')
                    .next()
                    .unwrap_or("")
            } else {
                &name
            };
            let hints = terminal_protocols(family);
            detected.kitty |= hints.kitty;
            detected.iterm2 |= hints.iterm2;
        }
        detected
    }
}

fn terminal_protocols(name: &str) -> ImageCapabilities {
    let family = name
        .trim()
        .split(|c: char| c.is_whitespace() || c == '(')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    ImageCapabilities {
        kitty: matches!(
            family.as_str(),
            "kitty" | "ghostty" | "wezterm" | "rio" | "konsole"
        ),
        iterm2: matches!(
            family.as_str(),
            "iterm.app" | "iterm2" | "wezterm" | "rio" | "konsole" | "mintty"
        ),
        sixel: false,
    }
}

fn supports_sixel(attributes: &[Option<u32>]) -> bool {
    matches!(
        attributes.split_first(),
        Some((Some(62..=65), features)) if features.contains(&Some(4))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixel_detection_interprets_da1_capabilities_after_the_service_class() {
        for class in [62, 63, 64, 65] {
            assert!(supports_sixel(&[Some(class), Some(1), Some(4), Some(22)]));
            assert!(!supports_sixel(&[Some(class), Some(1), Some(22)]));
        }
        for attributes in [
            vec![],
            vec![Some(4)],
            vec![Some(4), Some(4)],
            vec![Some(1), Some(2), Some(4)],
            vec![None, Some(4)],
            vec![Some(64), None, Some(14)],
        ] {
            assert!(!supports_sixel(&attributes), "{attributes:?}");
        }
        assert!(supports_sixel(&[Some(64), None, Some(4)]));
    }

    #[test]
    fn image_detection_uses_the_supplied_environment() {
        use uncurses::program::Capabilities;
        use uncurses::terminal::EnvList;

        let capabilities = Capabilities::default();
        assert_eq!(
            ImageCapabilities::from(&capabilities),
            ImageCapabilities::default()
        );
        assert_eq!(
            ImageCapabilities::detect(&capabilities, &EnvList::new()),
            ImageCapabilities::default(),
        );
        for (key, value, kitty, iterm2) in [
            ("TERM_PROGRAM", "iTerm.app", false, true),
            ("TERM_PROGRAM", "WezTerm", true, true),
            ("TERM_PROGRAM", "rio", true, true),
            ("TERM_PROGRAM", "mintty", false, true),
            ("TERM_PROGRAM", "Ghostty", true, false),
            ("TERM_PROGRAM", "Konsole", true, true),
            ("LC_TERMINAL", "iTerm2", false, true),
            ("LC_TERMINAL", "ITERM2", false, true),
            ("ITERM_SESSION_ID", "session", false, true),
            ("KITTY_WINDOW_ID", "0", true, false),
            ("WEZTERM_EXECUTABLE", "/usr/bin/wezterm", true, true),
            ("TERM", "wezterm", true, true),
            ("TERM", "rio", true, true),
            ("TERM", "xterm-rio", true, true),
            ("TERM", "xterm-rio-256color", true, true),
            ("TERM", "xterm-kitty", true, false),
            ("TERM", "xterm-ghostty", true, false),
            ("TERM", "mintty-256color", false, true),
        ] {
            let env = EnvList::from_pairs([(key, value)]);
            let detected = ImageCapabilities::detect(&capabilities, &env);
            assert_eq!(
                detected,
                ImageCapabilities {
                    kitty,
                    iterm2,
                    sixel: false
                },
                "{key}={value}"
            );
        }
        for (key, value) in [
            ("TERM_PROGRAM", "Apple_Terminal"),
            ("TERM_PROGRAM", "not-WezTerm"),
            ("TERM_PROGRAM", ""),
            ("LC_TERMINAL", "unknown"),
            ("ITERM_SESSION_ID", ""),
            ("KITTY_WINDOW_ID", ""),
            ("WEZTERM_EXECUTABLE", ""),
            ("TERM", "xterm-256color"),
            ("TERM", "xterm-rioish"),
            ("TERM", "screen"),
            ("TERM", "tmux-256color"),
            ("COLORTERM", "truecolor"),
        ] {
            let env = EnvList::from_pairs([(key, value)]);
            assert_eq!(
                ImageCapabilities::detect(&capabilities, &env),
                ImageCapabilities::default(),
                "{key}={value}",
            );
        }
    }

    #[test]
    fn terminal_names_identify_image_protocols_without_matching_substrings() {
        for (name, kitty, iterm2) in [
            ("kitty(0.42.0)", true, false),
            ("Ghostty 1.2.0", true, false),
            ("WezTerm 20240203-110809-5046fc22", true, true),
            ("rio v0.3.0", true, true),
            ("Konsole 24.12.0", true, true),
            ("iTerm2 3.6.0", false, true),
            ("mintty 3.8.0", false, true),
            ("  RIO\t0.3.0 ", true, true),
            ("XTerm(400)", false, false),
            ("foot 1.20.0", false, false),
            ("not-kitty 1.0", false, false),
            ("kittyish", false, false),
            ("", false, false),
        ] {
            assert_eq!(
                terminal_protocols(name),
                ImageCapabilities {
                    kitty,
                    iterm2,
                    sixel: false
                },
                "{name}"
            );
        }
    }
}
