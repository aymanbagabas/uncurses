//! The packed cell the renderer stores, and the arena that gives its ids
//! meaning.
//!
//! A [`Cell`](crate::cell::Cell) owns its content and style outright, which
//! is what applications want but not what a full-screen diff wants: three
//! grids of them is a lot of memory to walk, and comparing two means
//! comparing strings. The renderer therefore stores [`Ref`], which is the
//! same cell as three interned ids in eight bytes, so a frame diff is an
//! integer compare.
//!
//! Ids mean nothing on their own. They are issued by one [`Arena`](arena::Arena)
//! and are only resolvable, or comparable, against that same arena. The
//! renderer's front, back, and tracked buffers all share one, which is what
//! lets the diff work; nothing outside this module ever sees an id.

pub(crate) mod arena;
pub(crate) mod local;

use crate::cell::{Cell, Content, Kind, Style};
use arena::Arena;

/// Bit position of the packed [`Kind`] within [`Ref::content`].
const KIND_SHIFT: u32 = 30;
/// Mask selecting the packed [`Kind`] bits.
const KIND_MASK: u32 = 0b11 << KIND_SHIFT;
/// Mask selecting the interned grapheme id.
const CONTENT_MASK: u32 = !KIND_MASK;

/// A grid cell as stored: three interned ids in eight bytes.
///
/// Ids mean nothing on their own. They are issued by one [`Arena`] and are
/// only resolvable, or comparable, against that same arena. Because equal
/// ids mean equal values, the renderer diffs a frame by comparing `Ref`s
/// directly and never inspects a color or a grapheme.
///
/// Surfaces convert to and from [`Cell`] at their public boundary, which is
/// what keeps id provenance inside the crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Ref {
    /// Interned grapheme id in the low 30 bits, [`Kind`] in the top two.
    /// Content id `0` is the empty content used by wide-cell continuation
    /// placeholders; see [`arena`] for the encoding.
    pub content: arena::GraphemeId,
    /// Interned style id. Equal ids mean equal styles, so cell comparison
    /// never inspects colors or attributes.
    pub style: arena::StyleId,
    /// Interned hyperlink id, or [`arena::EMPTY_LINK`] when the cell carries
    /// no link.
    pub link: arena::LinkId,
}

impl Default for Ref {
    fn default() -> Self {
        Self::BLANK
    }
}

impl Ref {
    /// A blank narrow cell with default style.
    ///
    /// Blank content is a single space, which is its own id in every arena,
    /// as are the default style and the absent link. That makes `BLANK`
    /// valid against any arena and lets it stay a constant.
    pub(crate) const BLANK: Ref = Ref {
        content: b' ' as u32 | ((Kind::Narrow as u32) << KIND_SHIFT),
        style: arena::EMPTY_STYLE,
        link: arena::EMPTY_LINK,
    };

    /// A single-column cell holding one scalar, with the default style.
    ///
    /// Valid against any arena: a scalar is its own grapheme id.
    #[cfg(any(test, uncurses_bench))]
    pub(crate) const fn narrow(ch: char) -> Self {
        Ref {
            content: (ch as u32) | ((Kind::Narrow as u32) << KIND_SHIFT),
            style: arena::EMPTY_STYLE,
            link: arena::EMPTY_LINK,
        }
    }

    /// The primary of a two-column scalar, with the default style.
    ///
    /// Valid against any arena, for the same reason as [`Ref::narrow`].
    #[cfg(any(test, uncurses_bench))]
    pub(crate) const fn wide(ch: char) -> Self {
        Ref {
            content: (ch as u32) | ((Kind::Wide as u32) << KIND_SHIFT),
            style: arena::EMPTY_STYLE,
            link: arena::EMPTY_LINK,
        }
    }

    /// The right-column placeholder for a [`Ref::wide`] primary.
    pub(crate) const fn continuation() -> Self {
        Ref {
            // A space, matching `BLANK`: the column is never drawn because
            // `Kind` says so, and keeping content out of the encoding means
            // no id is ever zero.
            content: b' ' as u32 | ((Kind::Continuation as u32) << KIND_SHIFT),
            style: arena::EMPTY_STYLE,
            link: arena::EMPTY_LINK,
        }
    }

    /// Assemble a cell from ids one arena has already issued.
    ///
    /// The ids must all come from the arena this cell will be stored in.
    pub(crate) const fn from_ids(
        content: arena::GraphemeId,
        kind: Kind,
        style: arena::StyleId,
        link: arena::LinkId,
    ) -> Self {
        Ref {
            content: (content & CONTENT_MASK) | ((kind as u32) << KIND_SHIFT),
            style,
            link,
        }
    }

    /// Return the cell's structural role.
    #[inline]
    pub(crate) fn kind(&self) -> Kind {
        match (self.content & KIND_MASK) >> KIND_SHIFT {
            0 => Kind::Narrow,
            1 => Kind::Wide,
            _ => Kind::Continuation,
        }
    }

    /// Test whether this is the primary of a two-column grapheme.
    #[inline]
    pub(crate) fn is_wide(&self) -> bool {
        matches!(self.kind(), Kind::Wide)
    }

    /// Test whether this is a wide-character continuation placeholder.
    #[inline]
    pub(crate) fn is_continuation(&self) -> bool {
        matches!(self.kind(), Kind::Continuation)
    }

    /// Test whether this cell renders as blank space.
    ///
    /// Style is not considered; see [`Cell::is_blank`].
    #[inline]
    pub(crate) fn is_blank(&self) -> bool {
        let id = self.content_id();
        id == 0 || id == b' ' as u32 || self.is_continuation()
    }

    /// Column footprint of this cell on the grid.
    ///
    /// Row walkers generally advance by `width().max(1)` so they make
    /// progress even when landing on a continuation.
    #[inline]
    pub(crate) fn width(&self) -> u8 {
        self.kind().width()
    }

    /// Interned grapheme id with the [`Kind`] bits stripped, as the arena
    /// knows it.
    #[inline]
    pub(crate) fn content_id(&self) -> arena::GraphemeId {
        self.content & CONTENT_MASK
    }
    /// Resolve this cell's ids against the arena that issued them.
    ///
    /// # Panics
    ///
    /// Never panics. Resolving against the wrong arena yields the wrong
    /// content rather than failing.
    pub(crate) fn resolve(&self, arena: &dyn Arena) -> Cell {
        let kind = self.kind();
        let id = self.content_id();
        let content = match char::from_u32(id) {
            Some(c) => Content::Char(c),
            None => Content::from(arena.grapheme(id)),
        };
        Cell {
            content,
            style: Style {
                style: arena.style(self.style),
                link: match arena.link(self.link) {
                    l if l.is_empty() => None,
                    l => Some(std::sync::Arc::new(l.clone())),
                },
            },
            kind,
        }
    }

    /// Resolve just the grapheme text.
    #[cfg(test)]
    pub(crate) fn text(&self, arena: &dyn Arena) -> String {
        self.resolve(arena).content.to_string()
    }
}

/// Conveniences for tests, which build cells against the process arena --
/// the same one [`Buffer::new`](crate::buffer::Buffer::new) uses.
#[cfg(test)]
impl Ref {
    pub(crate) fn with_style(self, sgr: impl Into<crate::style::Style>) -> Self {
        Ref {
            style: arena::global_ref().intern_style(&sgr.into()),
            ..self
        }
    }

    pub(crate) fn with_link(self, url: impl AsRef<str>, params: impl AsRef<str>) -> Self {
        Ref {
            link: arena::global_ref().intern_link(url.as_ref(), params.as_ref()),
            ..self
        }
    }

    pub(crate) fn style(&self) -> crate::style::Style {
        arena::global_ref().style(self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Style as Sgr;

    fn global() -> &'static local::LocalArena {
        arena::global_ref()
    }

    #[test]
    fn narrow_ref_holds_its_scalar() {
        let c = Ref::narrow('A');
        assert_eq!(char::from_u32(c.content_id()), Some('A'));
        assert_eq!(c.kind(), Kind::Narrow);
        assert_eq!(c.width(), 1);
    }

    #[test]
    fn wide_ref_holds_its_scalar() {
        let c = Ref::wide('中');
        assert_eq!(char::from_u32(c.content_id()), Some('中'));
        assert!(c.is_wide());
        assert_eq!(c.width(), 2);
    }

    #[test]
    fn continuation_ref_is_zero_width() {
        let c = Ref::continuation();
        assert!(c.is_continuation());
        assert_eq!(c.width(), 0);
    }

    #[test]
    fn content_classifies_like_the_arena() {
        assert_eq!(Content::from(""), Content::Char(' '));
        assert_eq!(Content::from("a"), Content::Char('a'));
        assert_eq!(Content::from("中"), Content::Char('中'));
        assert!(matches!(Content::from("e\u{0301}"), Content::Cluster(_)));
    }

    #[test]
    fn cell_derives_kind_from_width() {
        assert!(Cell::new("a", Sgr::default()).is_narrow());
        assert!(Cell::new("中", Sgr::default()).is_wide());
        assert!(Cell::new("", Sgr::default()).is_narrow());
    }

    #[test]
    fn round_trip_preserves_content_style_and_kind() {
        let arena = global();
        for cell in [
            Cell::new("a", Sgr::default().bold()),
            Cell::new("中", Sgr::default()),
            Cell::new("👩‍🚀", Sgr::default()),
            Cell::new("e\u{0301}", Sgr::default().underline()),
            Cell::continuation(),
            Cell::default(),
        ] {
            let back = cell.intern(arena).resolve(arena);
            assert_eq!(back, cell, "round trip changed {cell:?}");
        }
    }

    #[test]
    fn round_trip_preserves_a_hyperlink() {
        let arena = global();
        let cell = Cell::new("x", Style::new().link("https://example.com", "id=1"));
        assert_eq!(cell.intern(arena).resolve(arena), cell);
    }

    #[test]
    fn interning_equal_cells_yields_equal_refs() {
        // The whole point of the packed form: the renderer diffs by id.
        let arena = global();
        let a = Cell::new("👩‍🚀", Sgr::default().bold());
        let b = Cell::new("👩‍🚀", Sgr::default().bold());
        assert_eq!(a.intern(arena), b.intern(arena));
        assert_ne!(
            a.intern(arena),
            Cell::new("👩‍🚀", Sgr::default()).intern(arena)
        );
    }

    #[test]
    fn blank_ref_matches_a_blank_cell() {
        let arena = global();
        assert_eq!(Ref::BLANK, Cell::new(" ", Sgr::default()).intern(arena));
        assert!(Ref::BLANK.is_blank());
        assert!(Ref::BLANK.resolve(arena).is_blank());
    }

    #[test]
    fn a_continuation_is_blank_and_zero_width() {
        // Its column belongs to the wide primary on its left, so `Kind` is
        // what matters; the content is a space so no id is ever zero.
        let arena = global();
        let cont = Ref::continuation().resolve(arena);
        assert!(cont.is_blank());
        assert_eq!(cont.width(), 0);
        assert_eq!(Ref::continuation().text(arena), " ");
    }
}

#[cfg(test)]
mod layout {
    use super::*;

    #[test]
    fn a_stored_cell_is_eight_bytes() {
        // The packed form is the reason the arena exists. If it grows, a
        // full-screen grid grows with it and the trade stops paying.
        assert_eq!(size_of::<Ref>(), 8);
        assert_eq!(align_of::<Ref>(), 4);
    }
}

#[cfg(test)]
mod sizes {
    use super::*;
    #[test]
    fn report() {
        println!("Ref            = {}", size_of::<Ref>());
        println!("Cell (fat)     = {}", size_of::<Cell>());
        println!("Content        = {}", size_of::<Content>());
        println!("cell::Style    = {}", size_of::<Style>());
        println!("style::Style   = {}", size_of::<crate::style::Style>());
        println!(
            "Option<Link>   = {}",
            size_of::<Option<crate::style::Link>>()
        );
        println!(
            "Opt<Arc<Link>> = {}",
            size_of::<Option<std::sync::Arc<crate::style::Link>>>()
        );
    }
}

impl crate::buffer::GridCell for Ref {
    #[inline]
    fn width(&self) -> u8 {
        Ref::width(self)
    }
    #[inline]
    fn is_wide(&self) -> bool {
        Ref::is_wide(self)
    }
    #[inline]
    fn is_continuation(&self) -> bool {
        Ref::is_continuation(self)
    }
    #[inline]
    fn blank() -> Self {
        Ref::BLANK
    }
    #[inline]
    fn continuation() -> Self {
        Ref::continuation()
    }
    #[inline]
    fn blank_like(&self) -> Self {
        Ref {
            style: self.style,
            ..Ref::BLANK
        }
    }
    #[inline]
    fn continuation_like(&self) -> Self {
        Ref {
            style: self.style,
            ..Ref::continuation()
        }
    }
}

impl Cell {
    /// Pack this cell into ids issued by `arena`.
    pub(crate) fn intern(&self, arena: &dyn Arena) -> Ref {
        // Every id in a plain cell is identity-mapped: a scalar is its own
        // grapheme id, the empty style is 0, and "no link" is 0. That covers
        // most of what an application draws, and it needs no arena at all.
        if self.style.link.is_none() && self.style.style.is_empty() {
            let content = match &self.content {
                Content::Char(c) => *c as arena::GraphemeId,
                Content::Cluster(s) => arena.intern_grapheme(s),
            };
            return Ref::from_ids(content, self.kind, arena::EMPTY_STYLE, arena::EMPTY_LINK);
        }
        let content = match &self.content {
            Content::Char(c) => *c as arena::GraphemeId,
            Content::Cluster(s) => arena.intern_grapheme(s),
        };
        Ref::from_ids(
            content,
            self.kind,
            arena.intern_style(&self.style.style),
            match &self.style.link {
                Some(l) => arena.intern_link(&l.url, &l.params),
                None => arena::EMPTY_LINK,
            },
        )
    }
}
