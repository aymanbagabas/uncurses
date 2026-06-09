//! Text sizing protocol addon.
//!
//! Builds and emits the OSC 66 escape sequence used by terminals
//! that support per-run text sizing: each sequence declares a
//! block of cells and a piece of text the terminal renders into
//! that block at the requested scale and alignment.
//!
//! ```text
//! \x1b]66;<key>=<value>:<key>=<value>;<text>\x1b\\
//! ```
//!
//! Capability detection is the host's responsibility — this
//! crate emits unconditionally. Terminals that don't implement
//! the protocol ignore the OSC sequence; the [`uncurses`]
//! regions API has already stamped `Skip` placeholders over the
//! footprint so the cell grid stays consistent regardless.
//!
//! ## Quick start
//!
//! ```no_run
//! use uncurses::screen::{RegionId, Screen};
//! use uncurses_textsize::TextSizing;
//!
//! let mut screen = Screen::new(std::io::stdout());
//! let label = TextSizing::new("🐈").scale(2).width(2);
//! label.paint(&mut screen, RegionId(1), (10, 5))?;
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! Footprint is `(s * max(w, 1)) × s` cells. The metadata is
//! validated and clamped to the protocol's allowed ranges at
//! build time so [`TextSizing::encode`] is infallible.
//!
//! Per-OSC chunking is the caller's responsibility: the protocol
//! caps each sequence at 4096 bytes of payload text. See
//! [`MAX_TEXT_BYTES`].

#![forbid(unsafe_code)]

use std::io;

use compact_str::CompactString;
use uncurses::Rect;
use uncurses::screen::{RegionId, Screen};
use uncurses::text::{WidthMode, grapheme_cells};

/// Maximum number of UTF-8 text bytes per OSC 66 sequence as
/// defined by the protocol. Callers passing longer strings must
/// split the input themselves; [`TextSizing::new`] truncates at
/// this boundary on a UTF-8 character boundary.
pub const MAX_TEXT_BYTES: usize = 4096;

/// Vertical alignment for fractionally-scaled text.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VAlign {
    #[default]
    Top,
    Bottom,
    Center,
}

impl VAlign {
    fn as_param(self) -> u8 {
        match self {
            VAlign::Top => 0,
            VAlign::Bottom => 1,
            VAlign::Center => 2,
        }
    }
}

/// Horizontal alignment for fractionally-scaled text.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HAlign {
    #[default]
    Left,
    Right,
    Center,
}

impl HAlign {
    fn as_param(self) -> u8 {
        match self {
            HAlign::Left => 0,
            HAlign::Right => 1,
            HAlign::Center => 2,
        }
    }
}

/// A single text-sizing run: text plus the protocol metadata
/// that determines its cell footprint and visual scale.
///
/// Build with [`TextSizing::new`] then chain modifiers. Out-of-
/// range parameters are clamped silently to the protocol's
/// allowed ranges.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextSizing {
    text: CompactString,
    s: u8,
    w: u8,
    n: u8,
    d: u8,
    v: VAlign,
    h: HAlign,
}

impl TextSizing {
    /// Build a run carrying `text`. The text is truncated at the
    /// nearest UTF-8 boundary if it exceeds [`MAX_TEXT_BYTES`].
    pub fn new(text: impl Into<CompactString>) -> Self {
        let mut text = text.into();
        if text.len() > MAX_TEXT_BYTES {
            let cut = floor_char_boundary(text.as_str(), MAX_TEXT_BYTES);
            text.truncate(cut);
        }
        Self {
            text,
            s: 1,
            w: 0,
            n: 0,
            d: 0,
            v: VAlign::Top,
            h: HAlign::Left,
        }
    }

    /// Set the overall scale (1..=7). Out-of-range values clamp.
    pub fn scale(mut self, s: u8) -> Self {
        self.s = s.clamp(1, 7);
        self
    }

    /// Set the declared width in scaled cells (0..=7). Out-of-
    /// range values clamp. `0` lets the terminal compute the
    /// width from the text itself.
    pub fn width(mut self, w: u8) -> Self {
        self.w = w.min(7);
        self
    }

    /// Set the fractional scale `n/d` (each 0..=15). When
    /// `d <= n` or either is out of range, the fraction is
    /// dropped (no fractional scaling).
    pub fn fraction(mut self, n: u8, d: u8) -> Self {
        let n = n.min(15);
        let d = d.min(15);
        if d == 0 || n >= d {
            self.n = 0;
            self.d = 0;
        } else {
            self.n = n;
            self.d = d;
        }
        self
    }

    /// Set the alignment used when rendering fractionally-scaled
    /// text inside the cell block.
    pub fn align(mut self, h: HAlign, v: VAlign) -> Self {
        self.h = h;
        self.v = v;
        self
    }

    /// The text payload of this run.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Cell footprint of this run as `(width, height)`.
    ///
    /// Width is `s * w` when `w` is non-zero (the run reserves an
    /// explicitly declared block), or `s * text_display_width`
    /// when `w == 0` (the terminal sizes the block from the text
    /// itself, like normal cell-grid output but with each cell
    /// scaled `s × s`). Height is always `s`.
    ///
    /// Display width is measured in cluster-aware mode with the
    /// East-Asian Ambiguous policy off. [`Self::paint`] re-measures
    /// using the screen's own width mode and policy so the
    /// reserved cells match the terminal's own segmentation.
    pub fn footprint(&self) -> (u16, u16) {
        self.footprint_with(WidthMode::Grapheme, false)
    }

    fn footprint_with(&self, mode: WidthMode, eaw_wide: bool) -> (u16, u16) {
        let s = self.s as u16;
        let width = if self.w == 0 {
            let cells: u16 = grapheme_cells(self.text.as_str(), mode, eaw_wide)
                .map(|(_, w)| w as u16)
                .sum();
            cells.saturating_mul(s).max(s)
        } else {
            s.saturating_mul(self.w as u16)
        };
        (width, s)
    }

    /// Encode this run as the wire-format OSC 66 byte sequence.
    /// The terminator is `ESC \` (ST). The returned bytes do not
    /// reposition the cursor — [`Self::paint`] wraps the sequence
    /// with explicit cursor positioning so the run always begins
    /// and ends at its anchor cell.
    pub fn encode(&self) -> Vec<u8> {
        let mut metadata = String::new();
        push_kv(&mut metadata, "s", self.s, 1);
        push_kv(&mut metadata, "w", self.w, 0);
        if self.d > 0 {
            push_kv(&mut metadata, "n", self.n, 0);
            push_kv(&mut metadata, "d", self.d, 0);
            push_kv(&mut metadata, "v", self.v.as_param(), 0);
            push_kv(&mut metadata, "h", self.h.as_param(), 0);
        }

        let mut out = Vec::with_capacity(self.text.len() + metadata.len() + 8);
        out.extend_from_slice(b"\x1b]66;");
        out.extend_from_slice(metadata.as_bytes());
        out.push(b';');
        out.extend_from_slice(self.text.as_bytes());
        out.extend_from_slice(b"\x1b\\");
        out
    }

    /// Stamp this run as a paint region on `screen` under `id`.
    /// The screen reserves the cell footprint with `Skip`
    /// placeholders and emits the encoded payload after the cell
    /// diff on every render. Repeat calls with the same `id`
    /// replace the prior registration.
    pub fn paint<W: io::Write>(
        &self,
        screen: &mut Screen<W>,
        id: RegionId,
        pos: (u16, u16),
    ) -> io::Result<()> {
        let (width, height) = self.footprint_with(screen.width_mode(), screen.eaw_wide());
        let area = Rect {
            x: pos.0,
            y: pos.1,
            width,
            height,
        };
        screen.set_region(id, area, self.encode().into());
        Ok(())
    }
}

/// Append `key=value` to `metadata` only when `value` differs
/// from `default`, separating from prior entries with `:`.
fn push_kv(metadata: &mut String, key: &str, value: u8, default: u8) {
    if value == default {
        return;
    }
    if !metadata.is_empty() {
        metadata.push(':');
    }
    metadata.push_str(key);
    metadata.push('=');
    metadata.push_str(&value.to_string());
}

/// Largest index `i <= max_len` that is on a UTF-8 character
/// boundary in `s`.
fn floor_char_boundary(s: &str, max_len: usize) -> usize {
    if max_len >= s.len() {
        return s.len();
    }
    let mut i = max_len;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn defaults_emit_only_text() {
        let bytes = TextSizing::new("hi").encode();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.starts_with("\x1b]66;"), "header: {s:?}");
        assert!(s.ends_with("\x1b\\"), "trailer: {s:?}");
        // Default s=1 and w=0 are omitted from metadata.
        assert!(s.contains("\x1b]66;;hi"), "default metadata empty: {s:?}");
    }

    #[test]
    fn scale_and_width_appear_in_metadata() {
        let bytes = TextSizing::new("X").scale(2).width(3).encode();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.contains("s=2"), "missing s=2: {s:?}");
        assert!(s.contains("w=3"), "missing w=3: {s:?}");
        assert!(s.contains(";X\x1b\\"), "text body: {s:?}");
    }

    #[test]
    fn fraction_appears_only_when_valid() {
        let kept = TextSizing::new("X").fraction(1, 2).encode();
        let kept = std::str::from_utf8(&kept).unwrap();
        assert!(kept.contains("n=1"), "kept fraction: {kept:?}");
        assert!(kept.contains("d=2"), "kept fraction: {kept:?}");

        // d <= n drops both.
        let dropped = TextSizing::new("X").fraction(3, 2).encode();
        let dropped = std::str::from_utf8(&dropped).unwrap();
        assert!(!dropped.contains("n="), "dropped: {dropped:?}");
        assert!(!dropped.contains("d="), "dropped: {dropped:?}");

        // d == 0 drops both.
        let zero = TextSizing::new("X").fraction(0, 0).encode();
        let zero = std::str::from_utf8(&zero).unwrap();
        assert!(!zero.contains("n="));
        assert!(!zero.contains("d="));
    }

    #[test]
    fn alignment_only_appears_with_fraction() {
        let no_frac = TextSizing::new("X")
            .align(HAlign::Center, VAlign::Center)
            .encode();
        let no_frac = std::str::from_utf8(&no_frac).unwrap();
        assert!(!no_frac.contains("h="), "no fraction => no h: {no_frac:?}");
        assert!(!no_frac.contains("v="), "no fraction => no v: {no_frac:?}");

        let with_frac = TextSizing::new("X")
            .fraction(1, 2)
            .align(HAlign::Right, VAlign::Bottom)
            .encode();
        let with_frac = std::str::from_utf8(&with_frac).unwrap();
        assert!(with_frac.contains("h=1"), "h=Right: {with_frac:?}");
        assert!(with_frac.contains("v=1"), "v=Bottom: {with_frac:?}");
    }

    #[test]
    fn footprint_uses_text_display_width_when_w_is_zero() {
        // Single ASCII cell, scale 1.
        assert_eq!(TextSizing::new("a").footprint(), (1, 1));
        // Five ASCII cells × scale 2 = 10 wide × 2 tall.
        assert_eq!(TextSizing::new("hello").scale(2).footprint(), (10, 2));
        // East-Asian wide cluster counts as 2 cells.
        assert_eq!(TextSizing::new("中").footprint(), (2, 1));
        assert_eq!(TextSizing::new("中").scale(2).footprint(), (4, 2));
        // Empty text still reserves at least s cells (the OSC byte
        // sequence still emits a one-cell wide visual placeholder).
        assert_eq!(TextSizing::new("").scale(2).footprint(), (2, 2));
    }

    #[test]
    fn footprint_uses_declared_width_when_w_is_nonzero() {
        // Declared width takes precedence over text length.
        assert_eq!(TextSizing::new("a").scale(2).width(3).footprint(), (6, 2));
        assert_eq!(TextSizing::new("hello").width(4).footprint(), (4, 1));
    }

    #[test]
    fn out_of_range_values_clamp() {
        let bytes = TextSizing::new("X").scale(99).width(99).encode();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.contains("s=7"), "scale clamped to 7: {s:?}");
        assert!(s.contains("w=7"), "width clamped to 7: {s:?}");
    }

    #[test]
    fn long_text_truncates_on_char_boundary() {
        // 4097 ASCII bytes — gets cut to 4096.
        let big: String = "a".repeat(MAX_TEXT_BYTES + 1);
        let sizing = TextSizing::new(big);
        assert_eq!(sizing.text().len(), MAX_TEXT_BYTES);

        // Multi-byte char straddling the limit drops the full char.
        let mut s = "a".repeat(MAX_TEXT_BYTES - 1);
        s.push('é'); // 2 bytes
        let sizing = TextSizing::new(s);
        assert_eq!(sizing.text().len(), MAX_TEXT_BYTES - 1);
    }

    #[test]
    fn paint_registers_a_region_with_the_encoded_payload() {
        let mut screen = Screen::new(Vec::<u8>::new()).with_size(20, 4);
        let sizing = TextSizing::new("X").scale(2).width(2);
        sizing.paint(&mut screen, RegionId(1), (3, 1)).unwrap();
        screen.render().unwrap();
        screen.flush().unwrap();

        let written = String::from_utf8_lossy(screen.writer()).into_owned();
        assert!(written.contains("\x1b]66;"), "payload emitted: {written:?}");
        assert!(written.contains("s=2"));
        assert!(written.contains("w=2"));
        assert!(written.contains(";X\x1b\\"));
    }
}
