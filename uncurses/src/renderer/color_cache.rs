//! Memoized color-profile conversion used by the renderer.
//!
//! [`crate::color::Profile::convert`] is pure but performs a
//! nearest-palette search for the `Ansi` and `Ansi256` variants. Real
//! frames reuse a handful of source colors many times, so caching the
//! converted result keyed by the source RGB triple avoids re-running
//! the search.
//!
//! Private to the renderer — public APIs (Screen, Renderer) still
//! speak [`Profile`] directly.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;

use rustc_hash::FxBuildHasher;

use crate::color::{Color, Profile};
use crate::style::Style;

/// Profile + per-instance memo of nearest-palette conversions.
#[derive(Debug, Default)]
pub(super) struct ColorCache {
    profile: Profile,
    /// Interior-mutable so `convert` only needs `&self`, mirroring
    /// [`Profile::convert`]'s by-value signature. Uses a fast
    /// non-cryptographic hasher — the 3-byte RGB key doesn't need
    /// cryptographic strength and the conversion lookup is hot
    /// enough during styled output that the per-hit hash cost
    /// matters.
    cache: RefCell<HashMap<(u8, u8, u8), Color, FxBuildHasher>>,
}

impl ColorCache {
    pub(super) fn new(profile: Profile) -> Self {
        Self {
            profile,
            cache: RefCell::new(HashMap::default()),
        }
    }

    /// The wrapped profile.
    pub(super) fn profile(&self) -> Profile {
        self.profile
    }

    /// Replace the wrapped profile. Clears the cache when the profile
    /// actually changes — cached answers are profile-specific.
    pub(super) fn set_profile(&mut self, profile: Profile) {
        if self.profile != profile {
            self.profile = profile;
            self.cache.get_mut().clear();
        }
    }

    /// Memoized counterpart to [`Profile::convert`].
    pub(super) fn convert(&self, color: Color) -> Option<Color> {
        match self.profile {
            Profile::Disabled | Profile::Ascii => None,
            Profile::TrueColor => Some(color),
            Profile::Ansi | Profile::Ansi256 => {
                let rgb = color.to_rgb();
                if let Some(c) = self.cache.borrow().get(&rgb) {
                    return Some(*c);
                }
                let out = self.profile.convert(color)?;
                self.cache.borrow_mut().insert(rgb, out);
                Some(out)
            }
        }
    }

    /// Cached counterpart of [`crate::style::diff::convert_style`] —
    /// downsample a style under the wrapped profile, hitting the cache
    /// for each colored field. Hyperlinks are dropped entirely under
    /// [`Profile::Disabled`] so piped / non-TTY output stays free of
    /// OSC 8 sequences.
    ///
    /// Returns a borrowed reference whenever possible to avoid the
    /// per-cell rebuild on the hot pen-update path:
    /// - `TrueColor`: pass-through (no downsampling needed).
    /// - `Disabled`: owned empty style (no SGR, no link).
    /// - `Ascii` / `Ansi` / `Ansi256`: build an owned, converted copy.
    pub(super) fn convert_style<'a>(&self, style: &'a Style) -> Cow<'a, Style> {
        match self.profile {
            Profile::TrueColor => Cow::Borrowed(style),
            Profile::Disabled => Cow::Owned(Style::default()),
            Profile::Ascii => Cow::Owned(Style {
                fg: None,
                bg: None,
                underline_color: None,
                ..style.clone()
            }),
            Profile::Ansi | Profile::Ansi256 => Cow::Owned(Style {
                fg: style.fg.and_then(|c| self.convert(c)),
                bg: style.bg.and_then(|c| self.convert(c)),
                underline_color: style.underline_color.and_then(|c| self.convert(c)),
                ..style.clone()
            }),
        }
    }
}

/// Helper for use in tests / introspection — not used by the renderer
/// proper.
#[cfg(test)]
impl ColorCache {
    pub(super) fn cache_len(&self) -> usize {
        self.cache.borrow().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truecolor_is_passthrough_uncached() {
        let c = ColorCache::new(Profile::TrueColor);
        let red = Color::Rgb(255, 0, 0);
        assert_eq!(c.convert(red), Some(red));
        assert_eq!(c.cache_len(), 0);
    }

    #[test]
    fn disabled_and_ascii_return_none() {
        for p in [Profile::Disabled, Profile::Ascii] {
            let c = ColorCache::new(p);
            assert_eq!(c.convert(Color::Rgb(1, 2, 3)), None);
            assert_eq!(c.cache_len(), 0);
        }
    }

    #[test]
    fn ansi256_results_match_uncached_and_are_memoized() {
        let c = ColorCache::new(Profile::Ansi256);
        let red = Color::Rgb(255, 0, 0);
        let want = Profile::Ansi256.convert(red);
        assert_eq!(c.convert(red), want);
        assert_eq!(c.cache_len(), 1);
        assert_eq!(c.convert(red), want);
        assert_eq!(c.cache_len(), 1);
    }

    #[test]
    fn cache_keyed_by_rgb_not_color_variant() {
        let c = ColorCache::new(Profile::Ansi);
        let idx = Color::Indexed(9);
        let (r, g, b) = idx.to_rgb();
        let rgb = Color::Rgb(r, g, b);
        assert_eq!(c.convert(idx), c.convert(rgb));
        assert_eq!(c.cache_len(), 1);
    }

    #[test]
    fn set_profile_clears_cache_on_change() {
        let mut c = ColorCache::new(Profile::Ansi256);
        c.convert(Color::Rgb(10, 20, 30));
        assert_eq!(c.cache_len(), 1);
        c.set_profile(Profile::Ansi256);
        assert_eq!(c.cache_len(), 1, "no-op set keeps cache");
        c.set_profile(Profile::Ansi);
        assert_eq!(c.cache_len(), 0, "profile change drops cache");
    }
}
