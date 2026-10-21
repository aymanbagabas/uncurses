//! Symbol families, set operations, and selector parsing.

use std::collections::BTreeSet;
use std::io;
use std::ops::{BitOr, Sub};
use std::str::FromStr;
use std::sync::Arc;

use bitflags::bitflags;
use uncurses::text::WidthMode;

use super::catalog;
use crate::invalid;

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub(super) struct Tags: u32 {
        /// An empty cell.
        const SPACE = 1 << 0;
        /// A full block.
        const SOLID = 1 << 1;
        /// Light, medium, and dark shade patterns.
        const STIPPLE = 1 << 2;
        /// Unicode block elements, including halves, quadrants, and shades.
        const BLOCK = 1 << 3;
        /// Box borders and junctions.
        const BORDER = 1 << 4;
        /// Diagonal lines and crosses.
        const DIAGONAL = 1 << 5;
        /// Isolated dots and bullets.
        const DOT = 1 << 6;
        /// All 2-by-2 quadrant patterns.
        const QUADRANT = 1 << 7;
        /// Left and right halves.
        const LEFT_RIGHT = 1 << 8;
        /// Upper and lower halves. This selection alone uses two color samples.
        const UPPER_LOWER = 1 << 9;
        /// Both half-block families.
        const HALF = Self::LEFT_RIGHT.bits() | Self::UPPER_LOWER.bits();
        /// All 256 Braille patterns.
        const BRAILLE = 1 << 10;
        /// Mathematical operators and technical marks.
        const TECHNICAL = 1 << 11;
        /// Squares, circles, triangles, and diamonds.
        const GEOMETRIC = 1 << 12;
        /// Printable ASCII.
        const ASCII = 1 << 13;
        /// Latin, Greek, and wide hiragana letters.
        const ALPHA = 1 << 14;
        /// ASCII digits.
        const DIGIT = 1 << 15;
        /// Letters and digits.
        const ALNUM = Self::ALPHA.bits() | Self::DIGIT.bits();
        /// ASCII and printable Latin-1.
        const LATIN = 1 << 16;
        /// All 2-by-3 sextant patterns.
        const SEXTANT = 1 << 17;
        /// Diagonal blocks and triangular wedges.
        const WEDGE = 1 << 18;
        /// All 2-by-4 octant patterns. Requires Unicode 16 mosaic support.
        const OCTANT = 1 << 19;
        /// Legacy mosaics, wedges, eighth-block stripes, and shade patterns.
        const LEGACY = 1 << 20;
        /// All one-cell glyphs in the built-in catalog.
        const NARROW = 1 << 21;
        /// Two-cell glyphs under the selected text-width policy.
        const WIDE = 1 << 22;
        const INVERTED = 1 << 23;
        /// Common blocks and borders.
        const DEFAULT = Self::BLOCK.bits() | Self::BORDER.bits() | Self::DIAGONAL.bits();
        /// Every built-in family, including wide glyphs.
        const ALL = (1 << 24) - 1;
    }
}

/// A selection of built-in glyphs for image approximation.
///
/// Use `|` for union and `-` for exclusion. These operations act on glyphs,
/// including glyphs shared by several families. For example,
/// `SymbolSet::BORDER - SymbolSet::DIAGONAL` removes diagonals from borders.
/// [`DEFAULT`](Self::DEFAULT) uses common blocks and borders.
/// [`ALL`](Self::ALL) also selects letters, wide kana, and recent Unicode
/// mosaics. Confirm font support before selecting those families.
///
/// `NARROW` and `WIDE` follow the text-width policy used for rendering.
/// Unions and exclusions retain that policy-dependent selection.
///
/// Masks approximate glyphs, rather than the active terminal font. Geometric
/// masks follow Unicode shapes. Letter masks use the public-domain font data
/// provided by the MIT-licensed `font8x8` crate. This is an independent renderer,
/// not a translation of Chafa's tables.
///
/// Parse comma-separated classes with `+` and `-` for ordered set operations.
/// A leading sign starts from the default set. Select individual characters
/// with `[abc]`, `u2580`, or `u2580..u259f`. Inside brackets, escape `]` or `\`
/// with `\`. Explicit selections must match at least one built-in glyph.
///
/// ```
/// use uncurses_image::{Protocol, SymbolSet};
///
/// let symbols = (SymbolSet::BLOCK | SymbolSet::BORDER) - SymbolSet::DIAGONAL;
/// assert_eq!(symbols, "block+border-diagonal".parse::<SymbolSet>()?);
/// let protocol = Protocol::Symbols(symbols);
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct SymbolSet(Selection);

#[derive(Clone, Debug)]
enum Selection {
    Class(Tags),
    Glyphs(Arc<BTreeSet<char>>),
    Union(Arc<(SymbolSet, SymbolSet)>),
    Difference(Arc<(SymbolSet, SymbolSet)>),
}

macro_rules! families {
    ($(#[doc = $doc:literal] $name:ident => $selector:literal),* $(,)?) => {
        impl SymbolSet {
            $(#[doc = $doc]
            pub const $name: Self = Self(Selection::Class(Tags::$name));)*

            fn named(name: &str) -> Option<Self> {
                match name {
                    $($selector => Some(Self::$name),)*
                    "none" => Some(Self::NONE),
                    "quad" => Some(Self::QUADRANT),
                    _ => None,
                }
            }
        }
    };
}

families! {
    /// Empty cells.
    SPACE => "space",
    /// Full blocks.
    SOLID => "solid",
    /// Shade patterns.
    STIPPLE => "stipple",
    /// Block elements, including halves, quadrants, and shades.
    BLOCK => "block",
    /// Box borders and junctions.
    BORDER => "border",
    /// Diagonal strokes and crosses.
    DIAGONAL => "diagonal",
    /// Isolated dots and bullets.
    DOT => "dot",
    /// All 2-by-2 quadrant patterns.
    QUADRANT => "quadrant",
    /// Left and right halves.
    LEFT_RIGHT => "left_right",
    /// Upper and lower halves. One-cell glyphs can use the two-sample fast path.
    UPPER_LOWER => "upper_lower",
    /// Both half-block families.
    HALF => "half",
    /// Complementary glyphs whose simpler counterpart also exists in the catalog.
    INVERTED => "inverted",
    /// All 256 Braille patterns.
    BRAILLE => "braille",
    /// Mathematical operators and technical marks.
    TECHNICAL => "technical",
    /// Squares, circles, triangles, and diamonds.
    GEOMETRIC => "geometric",
    /// Printable ASCII.
    ASCII => "ascii",
    /// Latin, Greek, and wide hiragana letters.
    ALPHA => "alpha",
    /// ASCII digits.
    DIGIT => "digit",
    /// Letters and digits.
    ALNUM => "alnum",
    /// ASCII and printable Latin-1.
    LATIN => "latin",
    /// All 2-by-3 sextant patterns.
    SEXTANT => "sextant",
    /// Diagonal blocks and triangular wedges.
    WEDGE => "wedge",
    /// All 2-by-4 octant patterns. Requires Unicode 16 mosaic support.
    OCTANT => "octant",
    /// Legacy mosaics, wedges, stripes, and shades.
    LEGACY => "legacy",
    /// Every one-cell glyph under the selected text-width policy.
    NARROW => "narrow",
    /// Every two-cell glyph under the selected policy. An unmatched column stays blank.
    WIDE => "wide",
    /// Common blocks and borders.
    DEFAULT => "default",
    /// Every built-in family, including wide glyphs.
    ALL => "all",
}

impl SymbolSet {
    /// An empty set. Add glyphs before rendering.
    pub const NONE: Self = Self(Selection::Class(Tags::empty()));

    /// Return whether this set contains a glyph with the default text-width policy.
    ///
    /// The default uses [`WidthMode::Wc`] and one-cell ambiguous characters.
    pub fn contains(&self, character: char) -> bool {
        self.contains_with_policy(character, WidthMode::Wc, false)
    }

    /// Return whether this set contains a glyph under the supplied text-width policy.
    pub fn contains_with_policy(&self, character: char, mode: WidthMode, eaw_wide: bool) -> bool {
        let glyphs = catalog::glyphs(mode, eaw_wide);
        glyphs
            .binary_search_by_key(&character, |g| g.character)
            .is_ok_and(|index| self.matches(&glyphs[index]))
    }

    /// Return whether this set selects zero glyphs under the default text-width policy.
    pub fn is_empty(&self) -> bool {
        self.is_empty_with_policy(WidthMode::Wc, false)
    }

    /// Return whether this set selects zero glyphs under the supplied text-width policy.
    pub fn is_empty_with_policy(&self, mode: WidthMode, eaw_wide: bool) -> bool {
        match &self.0 {
            Selection::Class(tags) => tags.is_empty(),
            Selection::Glyphs(chars) => chars.is_empty(),
            _ => !catalog::glyphs(mode, eaw_wide)
                .iter()
                .any(|g| self.matches(g)),
        }
    }

    pub(super) fn matches(&self, glyph: &catalog::Glyph) -> bool {
        match &self.0 {
            Selection::Class(tags) => glyph.tags.intersects(*tags),
            Selection::Glyphs(chars) => chars.contains(&glyph.character),
            Selection::Union(pair) => pair.0.matches(glyph) || pair.1.matches(glyph),
            Selection::Difference(pair) => pair.0.matches(glyph) && !pair.1.matches(glyph),
        }
    }
}

impl PartialEq for SymbolSet {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Selection::Class(a), Selection::Class(b)) if a == b => true,
            (Selection::Glyphs(a), Selection::Glyphs(b)) => a == b,
            _ => catalog::POLICIES.iter().all(|&(mode, eaw_wide)| {
                catalog::glyphs(mode, eaw_wide)
                    .iter()
                    .all(|glyph| self.matches(glyph) == other.matches(glyph))
            }),
        }
    }
}

impl Eq for SymbolSet {}

impl BitOr for SymbolSet {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        match (&self.0, &rhs.0) {
            (Selection::Class(a), Selection::Class(b)) => Self(Selection::Class(*a | *b)),
            _ => Self(Selection::Union(Arc::new((self, rhs)))),
        }
    }
}

impl Sub for SymbolSet {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self(Selection::Difference(Arc::new((self, rhs))))
    }
}

impl FromStr for SymbolSet {
    type Err = io::Error;

    fn from_str(input: &str) -> io::Result<Self> {
        let mut rest = input.trim();
        let mut result = if rest.starts_with(['+', '-']) {
            Self::DEFAULT
        } else {
            Self::NONE
        };
        if rest.is_empty() {
            return Err(invalid("expected a symbol selector"));
        }
        loop {
            let remove = rest.starts_with('-');
            rest = rest
                .strip_prefix(['+', '-', ','])
                .unwrap_or(rest)
                .trim_start();
            let selection;
            if let Some(literal) = rest.strip_prefix('[') {
                let mut chars = BTreeSet::new();
                let mut escaped = false;
                let mut end = None;
                for (at, ch) in literal.char_indices() {
                    if !escaped && ch == '\\' {
                        escaped = true;
                        continue;
                    }
                    if !escaped && ch == ']' {
                        end = Some(at + 2);
                        break;
                    }
                    if !Self::ALL.contains(ch) {
                        return Err(invalid("literal symbol has no built-in glyph"));
                    }
                    chars.insert(ch);
                    escaped = false;
                }
                rest = &rest[end.ok_or_else(|| invalid("unterminated symbol literal"))?..];
                selection = Self(Selection::Glyphs(Arc::new(chars)));
            } else {
                let end = rest.find(['+', '-', ',']).unwrap_or(rest.len());
                let token = rest[..end].trim();
                selection = if let Some(named) = Self::named(token) {
                    named
                } else if token.starts_with('u') {
                    let code = |s: &str| {
                        s.strip_prefix('u')
                            .and_then(|s| u32::from_str_radix(s, 16).ok())
                            .and_then(char::from_u32)
                            .ok_or_else(|| invalid("invalid symbol code point"))
                    };
                    let (first, last) = token.split_once("..").unwrap_or((token, token));
                    let (first, last) = (code(first)?, code(last)?);
                    let chars: BTreeSet<_> = catalog::GLYPHS
                        .iter()
                        .map(|g| g.character)
                        .filter(|c| (first..=last).contains(c))
                        .collect();
                    if chars.is_empty() {
                        return Err(invalid("symbol range contains no built-in glyphs"));
                    }
                    Self(Selection::Glyphs(Arc::new(chars)))
                } else {
                    return Err(invalid("unknown symbol class"));
                };
                rest = &rest[end..];
            }
            result = if remove {
                result - selection
            } else {
                result | selection
            };
            rest = rest.trim_start();
            if rest.is_empty() {
                return Ok(result);
            }
            if !rest.starts_with(['+', '-', ',']) {
                return Err(invalid("expected a symbol selector separator"));
            }
        }
    }
}

impl Default for SymbolSet {
    fn default() -> Self {
        Self::DEFAULT
    }
}
