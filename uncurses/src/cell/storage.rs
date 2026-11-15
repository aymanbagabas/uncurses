//! Private packed storage. Each owner pairs its cells with its own resources.
//!
//! A returned cell owns one reference to each resource it names. Moving that
//! cell transfers ownership. Copying it requires `retain`; discarding it requires
//! `release`. Operation holds use the same pair of methods and the same count.
//! Acquire incoming references before releasing outgoing references.
//!
//! IDs belong to one owner. Use `value_eq` across owners and `clone_cells` to
//! copy a collection into an independent owner. Reads borrow glyphs and style
//! parts; only `materialize` constructs an owned `Cell`.
//! The codec preserves the eight defined attributes and drops unknown bits.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use hashbrown::HashTable;

#[cfg(test)]
use super::Cell;
use crate::color::Color;
use crate::style::{AttrFlags, Link, Style, UnderlineStyle};

const EMPTY: u32 = 0x11_0000;
const COMPLEX: u32 = EMPTY + 1;
const GLYPH_MASK: u32 = 0xff_ffff;
const COLOR_MASK: u64 = (1 << 25) - 1;

/// A handle with ownership managed by `Resources`, rather than by `Drop`.
///
/// The fields stay private so callers cannot manufacture resource IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub(crate) struct PackedCell {
    glyph_width: u32,
    extras_id: u32,
    sgr: u64,
}

impl PackedCell {
    pub(crate) const BLANK: Self = Self {
        glyph_width: (1 << 24) | b' ' as u32,
        extras_id: 0,
        sgr: 0,
    };

    #[cfg(test)]
    pub(crate) const CONTINUATION: Self = Self {
        glyph_width: EMPTY,
        extras_id: 0,
        sgr: 0,
    };

    #[inline]
    pub(crate) fn width(self) -> u8 {
        (self.glyph_width >> 24) as u8
    }

    pub(crate) fn is_wide(self) -> bool {
        self.width() > 1
    }

    fn token(self) -> u32 {
        self.glyph_width & GLYPH_MASK
    }

    #[inline]
    pub(crate) fn is_blank(self) -> bool {
        self.width() == 1 && matches!(self.token(), EMPTY | 0x20)
    }

    pub(crate) fn is_continuation(self) -> bool {
        self.width() == 0
    }

    #[inline]
    pub(super) fn scalar(self) -> Option<char> {
        char::from_u32(self.token())
    }

    #[inline]
    pub(super) fn sgr_word(self) -> u64 {
        self.sgr
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Glyph<'a> {
    Scalar(char),
    Empty,
    Str(&'a str),
}

/// An owned payload occurs once in the pool, including its original `Arc<Link>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Extras {
    pub(crate) link: Option<Arc<Link>>,
    // Only selector 3 stores a color here.
    pub(crate) underline_color: Option<Color>,
}

#[derive(Clone, Copy, Hash)]
struct ExtrasRef<'a> {
    link: Option<&'a Arc<Link>>,
    underline_color: Option<Color>,
}

impl ExtrasRef<'_> {
    fn matches(self, other: &Extras) -> bool {
        self.link == other.link.as_ref() && self.underline_color == other.underline_color
    }

    fn to_owned(self) -> Extras {
        Extras {
            link: self.link.cloned(),
            underline_color: self.underline_color,
        }
    }
}

/// Decoded colors include the concrete, explicit underline color.
///
/// Changing `fg` or `bg` on these parts preserves that color. Encoding selects
/// its new representation relative to the replacement foreground/background.
/// Reading these parts neither allocates nor clones a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StyleParts<'a> {
    pub(crate) fg: Option<Color>,
    pub(crate) bg: Option<Color>,
    pub(crate) underline_color: Option<Color>,
    pub(crate) underline: UnderlineStyle,
    pub(crate) attrs: AttrFlags,
    pub(crate) link: Option<&'a Arc<Link>>,
}

impl<'a> StyleParts<'a> {
    pub(crate) fn from_style(style: &'a Style) -> Self {
        Self {
            fg: style.fg_color(),
            bg: style.bg_color(),
            underline_color: style.ul_color(),
            underline: style.underline_shape(),
            attrs: AttrFlags::from_bits_truncate(style.attributes().bits()),
            link: style.link_arc(),
        }
    }

    pub(crate) fn to_owned(self) -> Style {
        Style::from_parts(self)
    }

    fn encode(self) -> (u64, ExtrasRef<'a>) {
        let selector = match self.underline_color {
            None => 0,
            Some(color) if Some(color) == self.fg => 1,
            Some(color) if Some(color) == self.bg => 2,
            Some(_) => 3,
        };
        let sgr = color_word(self.fg)
            | (color_word(self.bg) << 25)
            | (u64::from(self.attrs.bits() & 0xff) << 50)
            | ((self.underline as u64) << 58)
            | (selector << 61);
        (
            sgr,
            ExtrasRef {
                link: self.link,
                underline_color: if selector == 3 {
                    self.underline_color
                } else {
                    None
                },
            },
        )
    }
}

#[inline]
pub(crate) fn color_word(color: Option<Color>) -> u64 {
    match color {
        None => 0,
        Some(Color::Indexed(index)) => u64::from(index) + 17,
        Some(Color::Rgb(r, g, b)) => {
            (1 << 24) | (u64::from(r) << 16) | (u64::from(g) << 8) | u64::from(b)
        }
        Some(named) => u64::from(named.named_index().unwrap()) + 1,
    }
}

pub(crate) fn color_value(word: u64) -> Option<Color> {
    match word & COLOR_MASK {
        0 => None,
        n @ 1..=16 => Color::from_named((n - 1) as u8),
        n @ 17..=272 => Some(Color::Indexed((n - 17) as u8)),
        n @ 0x1000000..=COLOR_MASK => Some(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)),
        _ => unreachable!("private color code"),
    }
}

pub(crate) fn underline_shape(word: u64) -> UnderlineStyle {
    match (word >> 58) & 7 {
        0 => UnderlineStyle::None,
        1 => UnderlineStyle::Single,
        2 => UnderlineStyle::Double,
        3 => UnderlineStyle::Curly,
        4 => UnderlineStyle::Dotted,
        5 => UnderlineStyle::Dashed,
        _ => unreachable!("private underline code"),
    }
}

#[derive(Clone)]
struct Entry<T> {
    value: T,
    refs: usize,
    hash: u64,
}

#[derive(Clone)]
struct Pool<T> {
    entries: Vec<Option<Entry<T>>>,
    index: HashTable<u32>,
    free: Vec<u32>,
}

impl<T> Default for Pool<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            index: HashTable::new(),
            free: Vec::new(),
        }
    }
}

impl<T> Pool<T> {
    /// Hash the borrowed input once. Allocate its payload only after a miss.
    fn acquire<Q: Hash + ?Sized>(
        &mut self,
        query: &Q,
        matches: impl Fn(&T) -> bool,
        make: impl FnOnce() -> T,
        max_id: u32,
    ) -> u32 {
        let mut hasher = rustc_hash::FxHasher::default();
        query.hash(&mut hasher);
        let hash = hasher.finish();
        if let Some(&id) = self.index.find(hash, |&id| matches(self.get(id))) {
            self.retain(id);
            return id;
        }
        let id = self.free.last().copied().unwrap_or_else(|| {
            let id = u32::try_from(self.entries.len()).expect("resource ID capacity exceeded");
            assert!(id <= max_id, "resource ID capacity exceeded");
            id
        });
        let entry = Entry {
            value: make(),
            refs: 1,
            hash,
        };
        if self.free.pop().is_some() {
            self.entries[id as usize] = Some(entry);
        } else {
            self.entries.push(Some(entry));
        }
        let entries = &self.entries;
        self.index
            .insert_unique(hash, id, |&id| entries[id as usize].as_ref().unwrap().hash);
        id
    }

    #[inline]
    fn get(&self, id: u32) -> &T {
        &self.entries[id as usize].as_ref().unwrap().value
    }

    fn retain(&mut self, id: u32) {
        let entry = self.entries[id as usize].as_mut().unwrap();
        entry.refs = entry.refs.checked_add(1).expect("resource count exceeded");
    }

    fn release(&mut self, id: u32) {
        let entry = self.entries[id as usize].as_mut().unwrap();
        assert!(entry.refs > 0);
        entry.refs -= 1;
        if entry.refs == 0 {
            self.index
                .find_entry(entry.hash, |&old| old == id)
                .unwrap()
                .remove();
            self.entries[id as usize] = None;
            self.free.push(id);
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct Resources {
    pools: Option<Box<ResourcePools>>,
}

#[derive(Clone, Default)]
pub(crate) struct ResourcePools {
    glyphs: Pool<Box<str>>,
    extras: Pool<Extras>,
}

// Buffer swaps move one pointer instead of both pool headers. Scalar-only
// buffers keep the headers unallocated, including the renderer's empty buffer.
impl std::ops::Deref for Resources {
    type Target = ResourcePools;

    fn deref(&self) -> &Self::Target {
        self.pools.as_deref().expect("resource payload exists")
    }
}

impl std::ops::DerefMut for Resources {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.pools.get_or_insert_with(Default::default)
    }
}

/// Each translation owns a hold until the operation ends.
#[derive(Default)]
pub(crate) struct ImportCache {
    glyphs: std::collections::HashMap<u32, u32>,
    extras: std::collections::HashMap<u32, u32>,
}

impl Resources {
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.pools
            .as_ref()
            .is_none_or(|pools| pools.glyphs.index.is_empty() && pools.extras.index.is_empty())
    }

    pub(crate) fn import_packed(
        &mut self,
        source: &Self,
        cell: PackedCell,
        cache: &mut ImportCache,
    ) -> PackedCell {
        let token = if cell.token() >= COMPLEX {
            let id = *cache
                .glyphs
                .entry(cell.token() - COMPLEX)
                .or_insert_with(|| {
                    let content = source.glyphs.get(cell.token() - COMPLEX).as_ref();
                    self.glyphs.acquire(
                        content,
                        |value| value.as_ref() == content,
                        || content.into(),
                        GLYPH_MASK - COMPLEX,
                    )
                });
            self.glyphs.retain(id);
            COMPLEX + id
        } else {
            cell.token()
        };
        let extras_id = if cell.extras_id != 0 {
            let id = *cache.extras.entry(cell.extras_id).or_insert_with(|| {
                let extras = source.extras.get(cell.extras_id - 1);
                self.acquire_extras(ExtrasRef {
                    link: extras.link.as_ref(),
                    underline_color: extras.underline_color,
                })
            });
            self.extras.retain(id - 1);
            id
        } else {
            0
        };
        PackedCell {
            glyph_width: (u32::from(cell.width()) << 24) | token,
            extras_id,
            sgr: cell.sgr,
        }
    }

    pub(crate) fn release_imports(&mut self, cache: ImportCache) {
        for (_, id) in cache.glyphs {
            self.glyphs.release(id);
        }
        for (_, id) in cache.extras {
            self.extras.release(id - 1);
        }
    }

    pub(crate) fn import(&mut self, cell: super::CellRef<'_>) -> PackedCell {
        if let Some(owned) = cell.owned_import_source()
            && owned.style.ul_color().is_none()
            && owned.style.link_arc().is_none()
        {
            let mut chars = owned.content().chars();
            let first = chars.next();
            if chars.next().is_none() {
                return PackedCell {
                    glyph_width: (u32::from(owned.width()) << 24)
                        | first.map_or(EMPTY, |scalar| scalar as u32),
                    extras_id: 0,
                    sgr: owned.style.sgr_word(),
                };
            }
        }
        if let Some(packed) = cell.packed_import_source()
            && packed.token() < COMPLEX
            && packed.extras_id == 0
        {
            // Scalar and empty cells carry values, not owner-local handles.
            return packed;
        }
        self.intern_parts(cell.glyph(), cell.width(), cell.parts())
    }
    /// Import exactly the content and explicit width of an existing cell.
    #[cfg(test)]
    pub(crate) fn intern(&mut self, cell: &Cell) -> PackedCell {
        self.intern_parts(
            Glyph::Str(cell.content()),
            cell.width(),
            StyleParts::from_style(&cell.style),
        )
    }

    pub(crate) fn intern_parts(
        &mut self,
        glyph: Glyph<'_>,
        width: u8,
        style: StyleParts<'_>,
    ) -> PackedCell {
        let token = match glyph {
            Glyph::Empty => EMPTY,
            Glyph::Scalar(scalar) => scalar as u32,
            Glyph::Str(content) => {
                // Normalize external borrowed input through the same scalar path.
                let mut chars = content.chars();
                match (chars.next(), chars.next()) {
                    (None, _) => EMPTY,
                    (Some(scalar), None) => scalar as u32,
                    _ => {
                        COMPLEX
                            + self.glyphs.acquire(
                                content,
                                |value| value.as_ref() == content,
                                || content.into(),
                                GLYPH_MASK - COMPLEX,
                            )
                    }
                }
            }
        };
        let (sgr, extras) = style.encode();
        let extras_id = self.acquire_extras(extras);
        PackedCell {
            glyph_width: (u32::from(width) << 24) | token,
            extras_id,
            sgr,
        }
    }

    fn acquire_extras(&mut self, extras: ExtrasRef<'_>) -> u32 {
        if extras.link.is_none() && extras.underline_color.is_none() {
            0
        } else {
            self.extras.acquire(
                &extras,
                |value| extras.matches(value),
                || extras.to_owned(),
                u32::MAX - 1,
            ) + 1
        }
    }

    /// Borrow the canonical payload. Zero means the empty extras value.
    #[inline]
    pub(crate) fn extras(&self, cell: PackedCell) -> Option<&Extras> {
        cell.extras_id.checked_sub(1).map(|id| self.extras.get(id))
    }

    #[inline]
    pub(crate) fn glyph(&self, cell: PackedCell) -> Glyph<'_> {
        match cell.token() {
            EMPTY => Glyph::Empty,
            token if token >= COMPLEX => Glyph::Str(self.glyphs.get(token - COMPLEX)),
            scalar => Glyph::Scalar(char::from_u32(scalar).expect("private scalar code")),
        }
    }

    pub(crate) fn style(&self, cell: PackedCell) -> StyleParts<'_> {
        let extras = self.extras(cell);
        let fg = color_value(cell.sgr);
        let bg = color_value(cell.sgr >> 25);
        StyleParts {
            fg,
            bg,
            underline_color: match (cell.sgr >> 61) & 3 {
                0 => None,
                1 => fg,
                2 => bg,
                3 => extras.unwrap().underline_color,
                _ => unreachable!(),
            },
            underline: underline_shape(cell.sgr),
            attrs: AttrFlags::from_bits_truncate(((cell.sgr >> 50) & 0xff) as u16),
            link: extras.and_then(|extras| extras.link.as_ref()),
        }
    }

    /// Acquire a second owning handle, including an operation hold.
    #[inline]
    pub(crate) fn retain(&mut self, cell: PackedCell) {
        if cell.token() >= COMPLEX {
            self.glyphs.retain(cell.token() - COMPLEX);
        }
        if cell.extras_id != 0 {
            self.extras.retain(cell.extras_id - 1);
        }
    }

    /// Consume one owning handle. A released handle must not be used again.
    #[inline]
    pub(crate) fn release(&mut self, cell: PackedCell) {
        if cell.token() >= COMPLEX {
            self.glyphs.release(cell.token() - COMPLEX);
        }
        if cell.extras_id != 0 {
            self.extras.release(cell.extras_id - 1);
        }
    }

    /// Return a new owning continuation with the source style and empty glyph.
    pub(crate) fn continuation(&mut self, source: PackedCell) -> PackedCell {
        if source.extras_id != 0 {
            self.extras.retain(source.extras_id - 1);
        }

        PackedCell {
            glyph_width: EMPTY,
            ..source
        }
    }

    pub(crate) fn blank(&mut self, source: PackedCell) -> PackedCell {
        let mut blank = self.continuation(source);
        blank.glyph_width = (1 << 24) | b' ' as u32;
        blank
    }

    #[cfg(test)]
    pub(crate) fn replace(&mut self, slot: &mut PackedCell, incoming: &Cell) {
        let incoming = self.intern(incoming);
        self.release(std::mem::replace(slot, incoming));
    }

    pub(crate) fn set_style(&mut self, cell: &mut PackedCell, style: StyleParts<'_>) {
        let (sgr, extras) = style.encode();
        let extras_id = self.acquire_extras(extras);
        if cell.extras_id != 0 {
            self.extras.release(cell.extras_id - 1);
        }
        cell.extras_id = extras_id;
        cell.sgr = sgr;
    }

    pub(crate) fn copy_style(&mut self, cell: &mut PackedCell, source: PackedCell) {
        if source.extras_id != 0 {
            self.extras.retain(source.extras_id - 1);
        }
        if cell.extras_id != 0 {
            self.extras.release(cell.extras_id - 1);
        }
        cell.extras_id = source.extras_id;
        cell.sgr = source.sgr;
    }

    /// Preserve the explicit underline color before choosing a new selector.
    #[cfg(test)]
    pub(crate) fn set_colors(
        &mut self,
        cell: &mut PackedCell,
        fg: Option<Color>,
        bg: Option<Color>,
    ) {
        let style = self.style(*cell).to_owned().fg(fg).bg(bg);
        self.set_style(cell, StyleParts::from_style(&style));
    }

    #[cfg(test)]
    pub(crate) fn materialize(&self, cell: PackedCell) -> Cell {
        let content = match self.glyph(cell) {
            Glyph::Empty => compact_str::CompactString::default(),
            Glyph::Scalar(scalar) => {
                let mut bytes = [0; 4];
                compact_str::CompactString::from(&*scalar.encode_utf8(&mut bytes))
            }
            Glyph::Str(content) => content.into(),
        };
        Cell::new(content, cell.width()).style(self.style(cell).to_owned())
    }

    #[inline]
    pub(crate) fn value_eq(&self, cell: PackedCell, other: &Self, other_cell: PackedCell) -> bool {
        if std::ptr::eq(self, other) {
            return cell == other_cell;
        }
        if cell.sgr != other_cell.sgr || cell.width() != other_cell.width() {
            return false;
        }
        let glyph_equal = if cell.token() < COMPLEX || other_cell.token() < COMPLEX {
            cell.token() == other_cell.token()
        } else {
            self.glyphs.get(cell.token() - COMPLEX)
                == other.glyphs.get(other_cell.token() - COMPLEX)
        };
        glyph_equal && self.extras(cell) == other.extras(other_cell)
    }

    #[inline]
    pub(crate) fn styles_eq(&self, cell: PackedCell, other: &Self, other_cell: PackedCell) -> bool {
        cell.sgr == other_cell.sgr
            && if std::ptr::eq(self, other) {
                cell.extras_id == other_cell.extras_id
            } else {
                self.extras(cell) == other.extras(other_cell)
            }
    }

    #[inline]
    pub(super) fn underline_color(&self, cell: PackedCell) -> Option<Color> {
        match (cell.sgr >> 61) & 3 {
            0 => None,
            1 => color_value(cell.sgr),
            2 => color_value(cell.sgr >> 25),
            3 => self.extras(cell).unwrap().underline_color,
            _ => unreachable!(),
        }
    }

    #[inline]
    pub(crate) fn style_eq(&self, cell: PackedCell, style: &Style) -> bool {
        if cell.sgr & ((1 << 61) - 1) != style.sgr_word() {
            return false;
        }
        self.underline_color(cell) == style.ul_color()
            && self.extras(cell).and_then(|extras| extras.link.as_ref()) == style.link_arc()
    }

    /// Copy the supplied owning handles into fresh arenas.
    ///
    /// Counts in the new owner cover only this slice, not source operation
    /// holds or other source cells. Source and destination release independently.
    #[cfg(test)]
    pub(crate) fn clone_cells(&self, cells: &[PackedCell]) -> (Self, Vec<PackedCell>) {
        let mut resources = Self::default();
        let mut cache = ImportCache::default();
        let cells = cells
            .iter()
            .map(|&cell| resources.import_packed(self, cell, &mut cache))
            .collect();
        resources.release_imports(cache);
        (resources, cells)
    }

    #[cfg(test)]
    pub(crate) fn audit(&self, cells: &[PackedCell]) {
        tests::audit(self, cells);
    }

    #[cfg(test)]
    pub(crate) fn slots(&self) -> (usize, usize, usize, usize) {
        let Some(pools) = self.pools.as_ref() else {
            return (0, 0, 0, 0);
        };
        (
            pools.glyphs.entries.len(),
            pools.glyphs.index.len(),
            pools.extras.entries.len(),
            pools.extras.index.len(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::CellRef;

    pub(super) fn audit(resources: &Resources, owners: &[PackedCell]) {
        let empty = ResourcePools::default();
        let resources = resources.pools.as_deref().unwrap_or(&empty);
        fn pool<T: Eq + Hash>(pool: &Pool<T>, expected: &[usize]) {
            assert_eq!(pool.entries.len(), expected.len());
            assert_eq!(pool.index.len() + pool.free.len(), pool.entries.len());
            let mut free = pool.free.clone();
            free.sort_unstable();
            free.dedup();
            assert_eq!(free.len(), pool.free.len());
            for (id, (entry, &count)) in pool.entries.iter().zip(expected).enumerate() {
                if let Some(entry) = entry {
                    assert_eq!(entry.refs, count, "resource {id}");
                    assert!(count > 0);
                    assert!(!free.contains(&(id as u32)));
                    let mut hasher = rustc_hash::FxHasher::default();
                    entry.value.hash(&mut hasher);
                    assert_eq!(hasher.finish(), entry.hash);
                    assert_eq!(
                        pool.index
                            .find(entry.hash, |&value| pool.get(value) == &entry.value),
                        Some(&(id as u32))
                    );
                } else {
                    assert_eq!(count, 0);
                    assert!(free.contains(&(id as u32)));
                }
            }
        }
        let mut glyphs = vec![0; resources.glyphs.entries.len()];
        let mut extras = vec![0; resources.extras.entries.len()];
        for &cell in owners {
            if cell.token() >= COMPLEX {
                glyphs[(cell.token() - COMPLEX) as usize] += 1;
            }
            if cell.extras_id != 0 {
                extras[(cell.extras_id - 1) as usize] += 1;
            }
        }
        pool(&resources.glyphs, &glyphs);
        pool(&resources.extras, &extras);
    }

    #[test]
    fn import_paths_preserve_scalar_selectors_and_resource_fallbacks() {
        let fg = Color::Rgb(7, 11, 13);
        let bg = Color::Indexed(4);
        let base = Style::EMPTY
            .fg(fg)
            .bg(bg)
            .attrs(AttrFlags::all())
            .underline_style(UnderlineStyle::Curly);
        let styles = [
            base.clone(),
            base.clone().underline_color(fg),
            base.clone().underline_color(bg),
            base.clone().underline_color(Color::Rgb(17, 19, 23)),
            base.clone().link("target", "id=test"),
            base.underline_color(fg).link("target", "id=test"),
        ];
        let mut source = Resources::default();
        let mut destination = Resources::default();
        let source_hold =
            source.intern(&Cell::new("source text", 1).style(Style::EMPTY.link("source", "")));
        let destination_hold = destination
            .intern(&Cell::new("destination text", 1).style(Style::EMPTY.link("destination", "")));
        for content in ["", " ", "\0", "▀", "中", "\u{10ffff}", "e\u{301}", "👩‍💻"] {
            for width in [0, 1, 2, 255] {
                for style in &styles {
                    let owned = Cell::new(content, width).style(style.clone());
                    let imported_owned = destination.import(CellRef::from(&owned));
                    assert_eq!(destination.materialize(imported_owned), owned);
                    let packed = source.intern(&owned);
                    let imported_packed = destination.import(CellRef::packed(&packed, &source));
                    assert_eq!(destination.materialize(imported_packed), owned);
                    assert!(source.value_eq(packed, &destination, imported_packed));
                    audit(&source, &[source_hold, packed]);
                    audit(
                        &destination,
                        &[destination_hold, imported_owned, imported_packed],
                    );
                    source.release(packed);
                    destination.release(imported_owned);
                    destination.release(imported_packed);
                }
            }
        }
        source.release(source_hold);
        destination.release(destination_hold);
        audit(&source, &[]);
        audit(&destination, &[]);
    }

    #[test]
    fn layout_and_exact_owned_cell_import() {
        assert_eq!(std::mem::size_of::<PackedCell>(), 16);
        assert_eq!(std::mem::align_of::<PackedCell>(), 8);
        assert_eq!(std::mem::offset_of!(PackedCell, glyph_width), 0);
        assert_eq!(std::mem::offset_of!(PackedCell, extras_id), 4);
        assert_eq!(std::mem::offset_of!(PackedCell, sgr), 8);
        fn send_sync<T: Send + Sync>() {}
        send_sync::<Resources>();
        send_sync::<PackedCell>();
        send_sync::<crate::buffer::Buffer>();
        send_sync::<super::super::CellRef<'static>>();
        let mut resources = Resources::default();
        for content in ["", " ", "\0", "é", "\u{10ffff}", "ab", "e\u{301}", "👩‍💻"] {
            for width in [0, 1, 2, 8, 255] {
                let owned = Cell::new(content, width);
                let packed = resources.intern(&owned);
                match resources.glyph(packed) {
                    Glyph::Scalar(scalar) => assert_eq!(packed.token(), scalar as u32),
                    Glyph::Empty => assert_eq!(packed.token(), EMPTY),
                    Glyph::Str(_) => assert!(packed.token() >= COMPLEX),
                }
                assert_eq!(resources.materialize(packed), owned);
                assert_eq!(packed.is_blank(), owned.is_blank());
                assert_eq!(packed.is_continuation(), owned.is_continuation());
                resources.release(packed);
            }
        }
        assert!(PackedCell::BLANK.is_blank());
        assert!(PackedCell::CONTINUATION.is_continuation());
        assert_eq!(resources.materialize(PackedCell::BLANK), Cell::BLANK);
        assert_eq!(
            resources.materialize(PackedCell::CONTINUATION),
            Cell::CONTINUATION
        );
        audit(&resources, &[]);
    }

    #[test]
    fn colors_shapes_and_selectors_round_trip_without_color_aliases() {
        let mut colors = vec![None];
        colors.extend((0..16).map(Color::from_named));
        colors.extend((0..=255).map(|index| Some(Color::Indexed(index))));
        colors.extend([
            Some(Color::Rgb(0, 0, 0)),
            Some(Color::Rgb(255, 255, 255)),
            Some(Color::Rgb(17, 127, 239)),
        ]);
        let mut words = std::collections::HashSet::new();
        let mut resources = Resources::default();
        for color in colors {
            assert!(words.insert(color_word(color)));
            for shape in [
                UnderlineStyle::None,
                UnderlineStyle::Single,
                UnderlineStyle::Double,
                UnderlineStyle::Curly,
                UnderlineStyle::Dotted,
                UnderlineStyle::Dashed,
            ] {
                for ul in [None, color, Some(Color::Rgb(3, 5, 7))] {
                    let style = Style::EMPTY
                        .fg(color)
                        .bg(color)
                        .underline_color(ul)
                        .underline_style(shape)
                        .attrs(AttrFlags::all());
                    let cell = resources.intern(&Cell::BLANK.style(style.clone()));
                    assert_eq!(resources.style(cell).to_owned(), style);
                    let borrowed = CellRef::packed(&cell, &resources);
                    assert_eq!(borrowed.fg_color(), style.fg_color());
                    assert_eq!(borrowed.bg_color(), style.bg_color());
                    assert_eq!(borrowed.ul_color(), style.ul_color());
                    assert_eq!(borrowed.attributes(), style.attributes());
                    assert_eq!(borrowed.underline_shape(), style.underline_shape());
                    assert!(borrowed.style_eq(&style));
                    assert_eq!(cell.sgr >> 63, 0);
                    resources.release(cell);
                    let style = style.fg(None).bg(color);
                    let cell = resources.intern(&Cell::BLANK.style(style.clone()));
                    assert_eq!(resources.style(cell).to_owned(), style);
                    let borrowed = CellRef::packed(&cell, &resources);
                    assert_eq!(borrowed.fg_color(), style.fg_color());
                    assert_eq!(borrowed.bg_color(), style.bg_color());
                    assert_eq!(borrowed.ul_color(), style.ul_color());
                    assert!(borrowed.style_eq(&style));
                    resources.release(cell);
                }
            }
        }
        let unknown = Style::EMPTY.attrs(AttrFlags::from_bits_retain(0xffa5));
        let cell = resources.intern(&Cell::BLANK.style(unknown));
        assert_eq!(resources.style(cell).attrs.bits(), 0xa5);
        resources.release(cell);
        let style = Style::EMPTY
            .fg(Color::from_named(15))
            .bg(Color::Indexed(255))
            .attrs(AttrFlags::all())
            .underline_style(UnderlineStyle::Dashed)
            .underline_color(Color::from_named(15));
        let cell = resources.intern(&Cell::BLANK.style(style));
        assert_eq!(
            cell.sgr,
            16 | (272 << 25) | (255 << 50) | (5 << 58) | (1 << 61)
        );
        assert_eq!(cell.extras_id, 0);
        resources.release(cell);
        audit(&resources, &[]);
    }

    #[test]
    fn explicit_underline_survives_color_changes_and_selector_spills() {
        let red = Some(Color::Indexed(1));
        let blue = Some(Color::Indexed(4));
        let mut resources = Resources::default();
        let mut cell = resources
            .intern(&Cell::BLANK.style(Style::EMPTY.fg(red).bg(blue).underline_color(red)));
        assert_eq!((cell.sgr >> 61) & 3, 1);
        assert_eq!(cell.extras_id, 0);
        for (fg, bg, selector) in [
            (blue, red, 2),
            (blue, None, 3),
            (None, blue, 3),
            (red, blue, 1),
            (None, None, 3),
        ] {
            resources.set_colors(&mut cell, fg, bg);
            assert_eq!(resources.style(cell).underline_color, red);
            assert_eq!((cell.sgr >> 61) & 3, selector);
            assert_eq!(
                resources.extras(cell).and_then(|e| e.underline_color),
                if selector == 3 { red } else { None }
            );
            audit(&resources, &[cell]);
        }
        resources.set_style(&mut cell, StyleParts::from_style(&Style::EMPTY));
        resources.set_colors(&mut cell, red, blue);
        assert_eq!(resources.style(cell).underline_color, None);
        assert_eq!(cell.extras_id, 0);
        resources.release(cell);
        audit(&resources, &[]);
    }

    #[test]
    fn canonical_payloads_reclaim_and_replace_shared_incoming_resources() {
        let mut resources = Resources::default();
        let style = Style::EMPTY.link("https://example.com", "id=example");
        let link = style.link_arc().unwrap();
        let owned = Cell::new("e\u{301}", 1).style(style.clone());
        let mut a = resources.intern(&owned);
        let other_link = Style::EMPTY.link("https://example.com", "id=example");
        assert!(!Arc::ptr_eq(link, other_link.link_arc().unwrap()));
        let b = resources.intern(&Cell::new("e\u{301}", 2).style(other_link));
        assert_eq!(a.token(), b.token());
        assert_eq!(a.extras_id, b.extras_id);
        assert!(Arc::ptr_eq(resources.style(b).link.unwrap(), link));
        assert_eq!(
            resources.style(b).link.map(Arc::as_ref),
            Some(link.as_ref())
        );
        assert_eq!(Arc::strong_count(link), 3);
        let old_token = a.token();
        let old_extras = a.extras_id;
        resources.release(b);
        // A sole outgoing owner must survive an equal incoming replacement.
        resources.replace(&mut a, &owned);
        resources.set_style(&mut a, StyleParts::from_style(&owned.style));
        assert_eq!(a.token(), old_token);
        assert_eq!(a.extras_id, old_extras);
        resources.retain(a); // An operation hold outlives the grid slot.
        resources.release(a);
        audit(&resources, &[a]);
        resources.release(a);
        assert_eq!(Arc::strong_count(link), 2);
        audit(&resources, &[]);
        let recycled =
            resources.intern(&Cell::new("different text", 8).style(Style::EMPTY.link("new", "")));
        assert_eq!(recycled.token(), old_token);
        assert_eq!(recycled.extras_id, old_extras);
        resources.release(recycled);
        audit(&resources, &[]);
    }

    #[test]
    fn continuations_own_extras_and_empty_values_bypass_pools() {
        let mut resources = Resources::default();
        let primary = resources.intern(
            &Cell::new("👩‍💻", 2).style(
                Style::EMPTY
                    .link("target", "")
                    .underline_color(Color::Rgb(7, 11, 13)),
            ),
        );
        let continuation = resources.continuation(primary);
        assert_eq!(resources.glyph(continuation), Glyph::Empty);
        assert_eq!(resources.style(continuation), resources.style(primary));
        resources.release(primary);
        assert!(resources.glyphs.index.is_empty());
        assert_eq!(resources.style(continuation).link.unwrap().url, "target");
        assert_eq!(
            resources.style(continuation).underline_color,
            Some(Color::Rgb(7, 11, 13))
        );
        audit(&resources, &[continuation]);
        resources.release(continuation);
        let empty = resources.intern(&Cell::new("", 255));
        let scalar = resources.intern(&Cell::new("中", 2));
        let default_continuation = resources.continuation(scalar);
        assert!(resources.glyphs.index.is_empty());
        assert!(resources.extras.index.is_empty());
        resources.release(empty);
        resources.release(scalar);
        resources.release(default_continuation);
        audit(&resources, &[]);
    }

    #[test]
    fn owner_local_ids_and_independent_clone_counts() {
        let mut left = Resources::default();
        let mut right = Resources::default();
        let a = left.intern(&Cell::new("first text", 1).style(Style::EMPTY.link("first", "")));
        let b = right.intern(&Cell::new("other text", 1).style(Style::EMPTY.link("other", "")));
        assert_eq!(a.token(), b.token());
        assert_eq!(a.extras_id, b.extras_id);
        assert!(!left.value_eq(a, &right, b));
        let different_link =
            right.intern(&Cell::new("first text", 1).style(Style::EMPTY.link("other", "")));
        assert!(!left.value_eq(a, &right, different_link));
        let equal = right.intern(&left.materialize(a));
        assert_ne!(a.token(), equal.token());
        assert!(left.value_eq(a, &right, equal));
        left.retain(a); // A second source cell.
        left.retain(a); // Source holds must not leak into the clone's counts.
        let (mut cloned, cells) = left.clone_cells(&[a, a]);
        audit(&cloned, &cells);
        assert!(left.value_eq(a, &cloned, cells[0]));
        left.release(a);
        left.release(a);
        left.release(a);
        audit(&left, &[]);
        assert_eq!(cloned.materialize(cells[0]).content(), "first text");
        for cell in cells {
            cloned.release(cell);
        }
        for cell in [b, different_link, equal] {
            right.release(cell);
        }
        audit(&right, &[]);
        audit(&cloned, &[]);
    }

    #[test]
    fn churn_audits_copies_moves_holds_rehash_and_slot_reuse() {
        let mut resources = Resources::default();
        let mut cells = vec![PackedCell::BLANK; 128];
        for pass in 0..5 {
            for (i, slot) in cells.iter_mut().enumerate() {
                let style = Style::EMPTY
                    .link(format!("target-{pass}-{i}"), "id=churn")
                    .underline_color(Color::Indexed((i % 256) as u8));
                resources.replace(slot, &Cell::new(format!("text-{pass}-{i}"), 1).style(style));
            }
            audit(&resources, &cells);
            for i in 1..cells.len() {
                let incoming = cells[i - 1];
                resources.retain(incoming);
                resources.release(std::mem::replace(&mut cells[i], incoming));
            }
            audit(&resources, &cells);
            cells.rotate_left(17); // Moves transfer the existing ownership.
            let hold = cells[0];
            resources.retain(hold);
            for slot in &mut cells {
                resources.release(std::mem::replace(slot, PackedCell::BLANK));
            }
            audit(&resources, &[hold]);
            resources.release(hold);
            audit(&resources, &[]);
        }
        assert_eq!(resources.glyphs.entries.len(), cells.len());
        assert_eq!(resources.extras.entries.len(), cells.len());
        // Exercise the same checked slot limit used by the 24-bit glyph pool.
        let mut small = Pool::<Box<str>>::default();
        for text in ["first", "second"] {
            small.acquire(text, |v| v.as_ref() == text, || text.into(), 1);
        }
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                small.acquire("overflow", |_| false, || "overflow".into(), 1);
            }))
            .is_err()
        );
        assert_eq!(small.entries.len(), 2);
        small.release(0);
        let id = small.acquire("recycled", |_| false, || "recycled".into(), 1);
        assert_eq!(id, 0);
        small.release(id);
        small.release(1);
    }
}
