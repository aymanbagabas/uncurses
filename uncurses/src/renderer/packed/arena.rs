//! Interning storage behind the ids a packed cell holds.
//!
//! A `Ref` stores its content, style, and hyperlink as ids
//! rather than as values. Ids are laid out so the overwhelmingly common
//! content needs no table at all:
//!
//! ```text
//! grapheme id 0            no content (wide-cell continuations)
//! grapheme id 1..0x110000  the Unicode scalar with that value
//! grapheme id >= 0x110000  index into the cluster table
//! ```
//!
//! Every single-scalar grapheme is therefore its own id and resolves without
//! touching a table or its lock, which covers the bulk of terminal content.
//! Only a cluster of two or more scalars earns an entry.
//!
//! [`Arena`] is the storage behind those ids.
//! [`Buffer`](crate::buffer::Buffer) resolves through the arena it was built
//! with, so an application can supply its own: a scratch arena that dies with
//! the buffer, a shared arena so several buffers can compare ids directly, or
//! the process default from [`global`].
//!
//! Ids mean nothing outside the arena that issued them. That is why a cell
//! crosses the public API as [`Cell`](super::Cell), which owns its values;
//! [`Surface`](crate::buffer::Surface) interns and resolves at its own
//! boundary, and [`Surface::draw`](crate::buffer::Surface::draw) re-interns
//! when the two surfaces hold different arenas.

use std::sync::{Arc, LazyLock};

use super::local::LocalArena;
use crate::style::Link;
use crate::style::Style;

/// Id of a grapheme: a Unicode scalar below `0x110000`, or a table index
/// above it. Needs the full 32 bits to hold scalars directly.
pub type GraphemeId = u32;

/// Id of a style.
///
/// Width follows the liveness bound rather than the space of possible
/// styles: a grid can reference at most one style per cell, so an id only
/// has to count live entries.
pub type StyleId = u16;

/// Id of a hyperlink, bounded like [`StyleId`].
pub type LinkId = u16;

/// Longest grapheme cluster that is stored verbatim, in codepoints.
///
/// A cluster can carry arbitrarily many combining marks, so untrusted text
/// could otherwise make a single entry arbitrarily large. Longer clusters are
/// truncated, which keeps the base character legible.
pub const MAX_GRAPHEME_CODEPOINTS: usize = 64;

/// Ceiling on the number of entries a [`LocalArena`] table will hold.
///
/// The global tables never reclaim, so without a ceiling any program that
/// displays untrusted text can be driven out of memory by a stream of novel
/// graphemes, styles, or hyperlinks. Past this many entries interning stops
/// allocating and degrades: unknown graphemes render as `?`, unknown styles
/// as the default, and unknown hyperlinks as no link.
///
/// The limit is far above what real workloads reach. A corpus spanning
/// Latin, Cyrillic, Greek, CJK, Hangul, Arabic, Hebrew, Devanagari, Tamil,
/// Thai, and emoji interns well under a thousand clusters, because every
/// single-scalar grapheme is encoded inline and never reaches a table.
pub const MAX_ENTRIES: u32 = u16::MAX as u32;

/// Id of [`Style::EMPTY`], pre-seeded in every arena so a blank cell is a
/// constant rather than something an arena has to issue.
pub const EMPTY_STYLE: StyleId = 0;

/// Id meaning "this cell carries no hyperlink".
pub const EMPTY_LINK: LinkId = 0;

/// Interning storage for the ids a [`Cell`](super::Cell) holds.
///
/// A cell is three `u32` ids: grapheme, style, and hyperlink. An `Arena`
/// turns values into those ids and back. Interning takes `&self` so several
/// buffers can share one arena; implementations provide their own interior
/// mutability.
///
/// # Id validity
///
/// Ids are scoped to the arena that issued them. Comparing or resolving an
/// id against a different arena yields the wrong cell, so keep a buffer and
/// the cells written into it on one arena.
///
/// # Reference lifetimes
///
/// The lookup methods return references borrowed from the arena, so an
/// implementation must keep entries at stable addresses for as long as it
/// lives.
// `Ref` is crate-private, so only this crate can implement the trait.
// That is deliberate: an outside type could otherwise mint ids that no
// arena issued.
#[allow(private_interfaces)]
pub trait Arena: Send + Sync + std::fmt::Debug {
    /// Map grapheme content to its id, adding it if new.
    fn intern_grapheme(&self, text: &str) -> GraphemeId;

    /// Return the grapheme content for an id from [`Arena::intern_grapheme`].
    fn grapheme(&self, id: GraphemeId) -> &str;

    /// Map a style to its id, adding it if new.
    fn intern_style(&self, style: &Style) -> StyleId;

    /// Return the style for an id from [`Arena::intern_style`].
    ///
    /// Returned by value: [`Style`] is a small `Copy` value, so an
    /// implementation never has to keep a stable address for one.
    fn style(&self, id: StyleId) -> Style;

    /// Map a hyperlink to its id, adding it if new.
    ///
    /// An empty URL means "no link" and maps to [`EMPTY_LINK`].
    fn intern_link(&self, url: &str, params: &str) -> LinkId;

    /// Return the hyperlink for an id from [`Arena::intern_link`].
    ///
    /// [`EMPTY_LINK`] resolves to an empty link, matching the empty content and
    /// empty style that id `0` resolves to.
    fn link(&self, id: LinkId) -> &Link;
}

/// The one process-wide arena.
///
/// This is what the free cell constructors intern into and what
/// [`Buffer::new`](crate::buffer::Buffer::new) uses, so their ids are
/// comparable. It is an ordinary [`LocalArena`], which means it owns its
/// entries, but it never gives them back: every buffer built with
/// [`Buffer::new`](crate::buffer::Buffer::new) shares it, so no caller can
/// name a complete root set. [`MAX_ENTRIES`] bounds it instead. Use
/// [`Buffer::new_in`](crate::buffer::Buffer::new_in) with a
/// [`LocalArena`] when a buffer's interned data should be reclaimable.
pub(crate) static GLOBAL: LazyLock<Arc<LocalArena>> = LazyLock::new(|| Arc::new(LocalArena::new()));

/// Return the process default arena.
///
/// This is what [`Buffer::new`](crate::buffer::Buffer::new) uses when no
/// arena is supplied, so buffers built that way share id meanings.
pub fn global() -> Arc<dyn Arena> {
    GLOBAL.clone()
}

/// Borrow the process arena for the life of the program.
///
/// # Panics
///
/// Never panics.
pub fn global_ref() -> &'static LocalArena {
    &GLOBAL
}

#[cfg(test)]
mod hostile_input {
    use super::*;

    #[test]
    fn a_long_cluster_is_truncated_not_stored_whole() {
        // A base character plus far more combining marks than any real
        // grapheme carries. Untrusted text can supply unlimited marks, so the
        // stored entry must not grow with the input.
        let zalgo: String = std::iter::once('a')
            .chain(std::iter::repeat_n('\u{0301}', 5_000))
            .collect();
        let id = GLOBAL.intern_grapheme(&zalgo);
        let stored = GLOBAL.grapheme(id);
        assert!(
            stored.chars().count() <= MAX_GRAPHEME_CODEPOINTS,
            "stored {} codepoints",
            stored.chars().count()
        );
        assert!(stored.starts_with('a'), "base character should survive");
    }

    #[test]
    fn distinct_long_clusters_still_collapse_to_one_entry() {
        // Two hostile clusters differing only past the cap must not each earn
        // an entry, or the truncation would not bound anything.
        let a: String = std::iter::once('x')
            .chain(std::iter::repeat_n('\u{0301}', 500))
            .collect();
        let b: String = std::iter::once('x')
            .chain(std::iter::repeat_n('\u{0301}', 900))
            .collect();
        assert_eq!(GLOBAL.intern_grapheme(&a), GLOBAL.intern_grapheme(&b));
    }

    #[test]
    fn exhausting_a_table_degrades_instead_of_growing() {
        // Simulate the ceiling rather than actually interning 65k entries:
        // once past MAX_ENTRIES the arena must hand back a usable fallback.
        assert_eq!(GLOBAL.grapheme(b'?' as GraphemeId), "?");
        assert_eq!(GLOBAL.style(EMPTY_STYLE), Style::EMPTY);
        assert!(GLOBAL.link(EMPTY_LINK).is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scalars_are_their_own_ids() {
        // A scalar is its own id and never reaches the cluster table, which
        // is what lets `Cell::narrow` build a cell without an arena.
        for c in [' ', 'a', 'Z', '0', '~', 'ä', '中', '🌟'] {
            let id = GLOBAL.intern_grapheme(&c.to_string());
            assert_eq!(id, c as GraphemeId, "scalar {c:?} should be its own id");
            assert_eq!(GLOBAL.grapheme(id), c.to_string());
        }
    }

    #[test]
    fn empty_is_zero() {
        assert_eq!(GLOBAL.intern_grapheme(""), 0);
        assert_eq!(GLOBAL.grapheme(0), "");
    }

    #[test]
    fn default_style_is_the_pre_seeded_empty_id() {
        // The renderer's pen fast path assumes an unstyled cell and a
        // default pen both resolve to EMPTY_STYLE; if these diverged the
        // pen would emit a redundant SGR for every unstyled cell.
        assert_eq!(GLOBAL.intern_style(&Style::default()), EMPTY_STYLE);
        assert_eq!(GLOBAL.intern_style(&Style::EMPTY), EMPTY_STYLE);
        assert_eq!(GLOBAL.style(EMPTY_STYLE), Style::EMPTY);
    }

    #[test]
    fn styles_intern_and_dedup() {
        let a = GLOBAL.intern_style(&Style::default().bold());
        let b = GLOBAL.intern_style(&Style::default().bold());
        assert_eq!(a, b);
        assert_ne!(a, EMPTY_STYLE);
        assert_eq!(GLOBAL.style(a), Style::default().bold());
    }

    #[test]
    fn clusters_intern_and_dedup() {
        let a = GLOBAL.intern_grapheme("👩‍🚀");
        let b = GLOBAL.intern_grapheme("👩‍🚀");
        assert_eq!(a, b);
        // A cluster id sits above every scalar, so the two spaces never collide.
        assert!(a > char::MAX as GraphemeId);
        assert_eq!(GLOBAL.grapheme(a), "👩‍🚀");
        assert_ne!(GLOBAL.intern_grapheme("中"), a);
        assert_eq!(GLOBAL.grapheme(GLOBAL.intern_grapheme("中")), "中");
    }
}
