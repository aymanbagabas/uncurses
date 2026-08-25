//! A reclaiming [`Arena`] that owns its entries.
//!
//! This is both the process default (see [`global`](super::arena::global))
//! and what an application reaches for when a buffer should not outlive its
//! interned data. A buffer showing untrusted content wants unbounded distinct
//! values over a program's life while paying only for what the grid currently
//! shows, and only reclamation gives that.
//!
//! `LocalArena` reclaims by sweeping.
//! [`sweep`](super::arena::sweep) hands it the live cells of every buffer on
//! this arena; it keeps the entries those cells reference, drops the rest,
//! and reports how ids moved so the caller can rewrite them.
//!
//! Interning is also capped at [`MAX_ENTRIES`], so a program that never
//! compacts still cannot be driven out of memory by a stream of novel
//! graphemes, styles, or hyperlinks.
//!
//! ## Why a sweep rather than reference counts
//!
//! A sweep derives liveness from the grid itself, so it cannot drift however
//! a cell was written. Counting references would instead require observing
//! every write, and a count that drifts low frees an entry still on screen:
//! silent corruption rather than a leak.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use super::arena::{
    Arena, EMPTY_LINK, EMPTY_STYLE, GraphemeId, LinkId, MAX_ENTRIES, MAX_GRAPHEME_CODEPOINTS,
    StyleId,
};
use crate::style::Link;
use crate::style::Style;

/// First id that indexes the grapheme table.
///
/// Every id below this is a Unicode scalar and *is* its own codepoint, which
/// is what lets a scalar cell be valid against any arena. Only clusters of
/// two or more scalars need a table entry.
const FIRST_INTERNED: GraphemeId = 0x11_0000;

/// Grapheme id handed back once the table is full: a visible, always-valid
/// stand-in that costs no allocation.
const GRAPHEME_EXHAUSTED: GraphemeId = b'?' as GraphemeId;

/// The 128 ASCII characters in one string, so a single ASCII scalar resolves
/// to a subslice rather than to a map entry behind a lock.
///
/// Non-ASCII scalars still take the map. They are rare enough in practice
/// that the extra table has not been worth it; the same trick would extend
/// to Latin-1 if a workload ever showed otherwise.
static ASCII: LazyLock<Box<str>> = LazyLock::new(|| (0u8..0x80).map(char::from).collect());

/// Entries are boxed so their contents keep a fixed address as the vectors
/// grow. Only a sweep drops them.
#[derive(Debug, Default)]
// The boxes are load-bearing, not redundant: `grapheme` and `link` hand out
// references into these entries, and an unboxed `Vec` would move them on
// reallocation and dangle those references.
#[allow(clippy::vec_box)]
struct Tables {
    graphemes: Vec<Box<str>>,
    grapheme_ids: HashMap<Box<str>, GraphemeId>,
    /// Encoded scalars, filled on demand so `grapheme` can hand back a
    /// borrow for a codepoint that was never interned.
    scalars: HashMap<GraphemeId, Box<str>>,
    styles: Vec<Style>,
    style_ids: HashMap<Style, StyleId>,
    links: Vec<Box<Link>>,
    link_ids: HashMap<Link, LinkId>,
}

/// An [`Arena`] that owns its entries and can reclaim them.
///
/// Pass one to [`Buffer::new_in`](crate::buffer::Buffer::new_in) so the
/// buffer's interned data dies with the buffer, and call
/// [`sweep`](super::arena::sweep) to drop entries the grid no longer shows.
#[derive(Debug)]
pub struct LocalArena {
    tables: RwLock<Tables>,
}

impl Default for LocalArena {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalArena {
    /// Create an empty arena.
    ///
    /// # Returns
    ///
    /// An arena pre-seeded so id `0` resolves to the empty style and the
    /// empty link. Every arena agrees on those, so a default-styled,
    /// unlinked cell is valid against all of them.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn new() -> Self {
        let mut t = Tables::default();
        t.styles.push(Style::EMPTY);
        t.style_ids.insert(Style::EMPTY, 0);
        t.links.push(Box::new(Link {
            url: String::new(),
            params: String::new(),
        }));
        Self {
            tables: RwLock::new(t),
        }
    }
}

impl Arena for LocalArena {
    fn intern_grapheme(&self, text: &str) -> GraphemeId {
        if text.is_empty() {
            return 0;
        }
        // A lone scalar is its own id.
        let mut chars = text.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && c != '\0'
        {
            return c as GraphemeId;
        }
        // Bound the entry: a cluster may carry unlimited combining marks.
        let text = match text.char_indices().nth(MAX_GRAPHEME_CODEPOINTS) {
            Some((cut, _)) => &text[..cut],
            None => text,
        };
        if let Some(&id) = self.tables.read().unwrap().grapheme_ids.get(text) {
            return id;
        }
        let mut t = self.tables.write().unwrap();
        if let Some(&id) = t.grapheme_ids.get(text) {
            return id;
        }
        if t.graphemes.len() as u32 >= MAX_ENTRIES {
            return GRAPHEME_EXHAUSTED;
        }
        let id = FIRST_INTERNED + t.graphemes.len() as GraphemeId;
        let owned: Box<str> = text.into();
        t.graphemes.push(owned.clone());
        t.grapheme_ids.insert(owned, id);
        id
    }

    fn grapheme(&self, id: GraphemeId) -> &str {
        if id == 0 {
            return "";
        }
        // ASCII is the bulk of terminal content, and every byte of it is a
        // one-byte substring of `ASCII`. Resolving from there keeps the
        // common cell off the lock and out of the map entirely.
        if id < 0x80 {
            let i = id as usize;
            return &ASCII[i..i + 1];
        }
        if id < FIRST_INTERNED {
            let Some(c) = char::from_u32(id) else {
                return "";
            };
            if let Some(text) = self.tables.read().unwrap().scalars.get(&id) {
                // SAFETY: boxed, so its address is fixed; scalars are never
                // dropped before the arena itself.
                return unsafe { &*(text.as_ref() as *const str) };
            }
            let mut t = self.tables.write().unwrap();
            let entry = t
                .scalars
                .entry(id)
                .or_insert_with(|| c.to_string().into_boxed_str());
            return unsafe { &*(entry.as_ref() as *const str) };
        }
        let t = self.tables.read().unwrap();
        match t.graphemes.get((id - FIRST_INTERNED) as usize) {
            // SAFETY: the text lives in a `Box<str>` on the heap, so its
            // address is fixed as the vector grows and is invalidated only by
            // `sweep`. `sweep` is reached through `Buffer::compact`, which
            // takes `&mut Buffer`, while every borrow handed out here flows
            // from `&Buffer` -- so the borrow checker already forbids holding
            // one across a sweep.
            Some(text) => unsafe { &*(text.as_ref() as *const str) },
            None => "",
        }
    }

    fn intern_style(&self, style: &Style) -> StyleId {
        // The empty style is pre-seeded at id 0 in every arena, so the most
        // common style of all resolves without touching the lock.
        if style.is_empty() {
            return EMPTY_STYLE;
        }
        if let Some(&id) = self.tables.read().unwrap().style_ids.get(style) {
            return id;
        }
        let mut t = self.tables.write().unwrap();
        if let Some(&id) = t.style_ids.get(style) {
            return id;
        }
        if t.styles.len() as u32 >= MAX_ENTRIES {
            return EMPTY_STYLE;
        }
        let id = t.styles.len() as StyleId;
        t.styles.push(*style);
        t.style_ids.insert(*style, id);
        id
    }

    fn style(&self, id: StyleId) -> Style {
        self.tables
            .read()
            .unwrap()
            .styles
            .get(id as usize)
            .copied()
            .unwrap_or(Style::EMPTY)
    }

    fn intern_link(&self, url: &str, params: &str) -> LinkId {
        if url.is_empty() {
            return EMPTY_LINK;
        }
        let probe = Link {
            url: url.to_owned(),
            params: params.to_owned(),
        };
        if let Some(&id) = self.tables.read().unwrap().link_ids.get(&probe) {
            return id;
        }
        let mut t = self.tables.write().unwrap();
        if let Some(&id) = t.link_ids.get(&probe) {
            return id;
        }
        if t.links.len() as u32 >= MAX_ENTRIES {
            return EMPTY_LINK;
        }
        let id = t.links.len() as LinkId;
        t.links.push(Box::new(probe.clone()));
        t.link_ids.insert(probe, id);
        id
    }

    fn link(&self, id: LinkId) -> &Link {
        let t = self.tables.read().unwrap();
        match t.links.get(id as usize) {
            // SAFETY: as in `grapheme`, the value is boxed so its address is
            // fixed, and only `sweep` drops it.
            Some(link) => unsafe { &*(link.as_ref() as *const Link) },
            None => &BLANK_LINK,
        }
    }
}

static BLANK_LINK: Link = Link {
    url: String::new(),
    params: String::new(),
};
