//! Lazy walker over CSI / DCS parameter bodies.
//!
//! A parameter body is the bytes between the introducer (`\x1b[`,
//! `\x9b`, `\x1bP`, `\x90`) and the final byte. Top-level parameters
//! are separated by `;`; each top-level parameter may itself carry
//! colon-separated sub-parameters.
//!
//! [`Params`] borrows the raw body and decodes it on demand:
//!
//! * [`Params::get`] / [`Params::get_or`] — the first sub-parameter of
//!   one top-level group (random access).
//! * [`Params::group`] — the n-th top-level group as a [`Group`].
//! * [`Params::iter`] — sequential walk over groups.
//! * [`Group::iter`] — sequential walk over the sub-parameters of one
//!   group.
//!
//! No allocations are performed. Each slot decodes to `Option<u32>`:
//! `None` for an omitted slot (empty `;;` or empty between two `:`s,
//! and non-numeric input), `Some(v)` for an explicit value (including
//! `Some(0)`).
//!
//! Per ECMA-48 §5.4.2 the default for an omitted parameter is
//! sequence-specific (e.g. `1` for `CUU`/`CUD`/`CUP`, `0` for
//! `SGR`/`ED`); callers apply that default with [`Params::get_or`].
//!
//! `u32` accommodates astral-plane codepoints carried by extensions
//! like `modifyOtherKeys` (`CSI 27 ; mod ; codepoint ~`) and the Kitty
//! keyboard protocol.

use std::fmt;

/// Borrowed view of a CSI / DCS parameter body.
#[derive(Copy, Clone)]
pub struct Params<'a> {
    raw: &'a [u8],
}

impl<'a> Params<'a> {
    /// Empty parameter list.
    pub const EMPTY: Self = Self { raw: &[] };

    /// Wrap the raw parameter bytes (between the introducer's last
    /// byte and the final byte, exclusive on both ends).
    #[inline]
    pub fn from_raw(raw: &'a [u8]) -> Self {
        Self { raw }
    }

    /// The raw parameter bytes, exactly as they appeared on the wire.
    #[inline]
    pub fn raw(&self) -> &'a [u8] {
        self.raw
    }

    /// `true` when the parameter body is empty (no groups at all).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    /// Number of `;`-separated groups. An empty body has zero groups;
    /// a trailing `;` produces an extra empty group.
    pub fn len(&self) -> usize {
        if self.raw.is_empty() {
            0
        } else {
            self.raw.iter().filter(|&&b| b == b';').count() + 1
        }
    }

    /// Sequential iterator over the `;`-separated groups.
    pub fn iter(&self) -> ParamIter<'a> {
        ParamIter {
            inner: if self.raw.is_empty() {
                None
            } else {
                Some(self.raw.split(is_semicolon))
            },
        }
    }

    /// The n-th group, or `None` when `idx >= len()`.
    pub fn group(&self, idx: usize) -> Option<Group<'a>> {
        self.iter().nth(idx)
    }

    /// First sub-parameter of the n-th group. `None` for absent groups
    /// and explicitly empty slots.
    pub fn get(&self, idx: usize) -> Option<u32> {
        self.group(idx).and_then(|g| g.first())
    }

    /// First sub-parameter of the n-th group, or `default` when the
    /// group is absent or the slot is explicitly empty.
    pub fn get_or(&self, idx: usize, default: u32) -> u32 {
        self.get(idx).unwrap_or(default)
    }

    /// A [`Params`] view starting at the group at index `start`. Used
    /// when a caller needs to expose "the rest of the parameter list"
    /// (e.g. `Event::WindowOp`'s args).
    pub fn slice_from(&self, start: usize) -> Params<'a> {
        let mut cur = self.raw;
        let mut skipped = 0;
        while skipped < start {
            match cur.iter().position(|&b| b == b';') {
                Some(p) => {
                    cur = &cur[p + 1..];
                    skipped += 1;
                }
                None => return Params::EMPTY,
            }
        }
        Params { raw: cur }
    }
}

impl<'a> fmt::Debug for Params<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.iter().map(|g| g.first()))
            .finish()
    }
}

impl<'a> IntoIterator for Params<'a> {
    type Item = Group<'a>;
    type IntoIter = ParamIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// One `;`-separated group within a [`Params`] body. Wraps the raw
/// bytes for that group (no separator); decodes its colon-separated
/// sub-parameters on demand.
#[derive(Copy, Clone)]
pub struct Group<'a> {
    raw: &'a [u8],
}

impl<'a> Group<'a> {
    /// Group with no sub-parameters.
    pub const EMPTY: Self = Self { raw: &[] };

    /// Wrap raw bytes as a single group.
    #[inline]
    pub fn from_raw(raw: &'a [u8]) -> Self {
        Self { raw }
    }

    /// Raw bytes between the surrounding `;` separators.
    #[inline]
    pub fn raw(&self) -> &'a [u8] {
        self.raw
    }

    /// `true` when the group's raw body is empty (i.e. between two
    /// `;` separators with nothing between them).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    /// Number of `:`-separated sub-parameters. An empty group has
    /// exactly one slot (which decodes as `None`).
    pub fn len(&self) -> usize {
        if self.raw.is_empty() {
            1
        } else {
            self.raw.iter().filter(|&&b| b == b':').count() + 1
        }
    }

    /// Sequential iterator over sub-parameters.
    pub fn iter(&self) -> SubIter<'a> {
        SubIter {
            inner: self.raw.split(is_colon),
        }
    }

    /// First sub-parameter of the group.
    pub fn first(&self) -> Option<u32> {
        self.iter().next().flatten()
    }

    /// The n-th sub-parameter, `None` when out of range or explicitly
    /// empty.
    pub fn nth(&self, i: usize) -> Option<u32> {
        self.iter().nth(i).flatten()
    }
}

impl<'a> fmt::Debug for Group<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<'a> IntoIterator for Group<'a> {
    type Item = Option<u32>;
    type IntoIter = SubIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Iterator over the `;`-separated groups of a [`Params`].
pub struct ParamIter<'a> {
    // `None` when the body was empty (yielding zero items, matching
    // the historical "empty body = zero params" convention).
    #[allow(clippy::type_complexity)]
    inner: Option<std::slice::Split<'a, u8, fn(&u8) -> bool>>,
}

impl<'a> Iterator for ParamIter<'a> {
    type Item = Group<'a>;
    fn next(&mut self) -> Option<Group<'a>> {
        self.inner.as_mut()?.next().map(|raw| Group { raw })
    }
}

/// Iterator over the `:`-separated sub-parameters of a [`Group`].
pub struct SubIter<'a> {
    inner: std::slice::Split<'a, u8, fn(&u8) -> bool>,
}

impl<'a> Iterator for SubIter<'a> {
    type Item = Option<u32>;
    fn next(&mut self) -> Option<Option<u32>> {
        self.inner.next().map(parse_u32)
    }
}

fn is_semicolon(b: &u8) -> bool {
    *b == b';'
}

fn is_colon(b: &u8) -> bool {
    *b == b':'
}

fn parse_u32(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() {
        return None;
    }
    let mut acc: u32 = 0;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        acc = acc.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(acc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(p: Params<'_>) -> Vec<Option<u32>> {
        p.iter().map(|g| g.first()).collect()
    }

    fn nested(p: Params<'_>) -> Vec<Vec<Option<u32>>> {
        p.iter().map(|g| g.iter().collect()).collect()
    }

    #[test]
    fn flat_distinguishes_absent_from_explicit_zero() {
        assert_eq!(flat(Params::from_raw(b"")), Vec::<Option<u32>>::new());
        assert_eq!(flat(Params::from_raw(b";")), vec![None, None]);
        assert_eq!(
            flat(Params::from_raw(b"0;;3")),
            vec![Some(0), None, Some(3)]
        );
        assert_eq!(flat(Params::from_raw(b";5")), vec![None, Some(5)]);
        assert_eq!(
            flat(Params::from_raw(b"27;5;128512")),
            vec![Some(27), Some(5), Some(128512)]
        );
    }

    #[test]
    fn flat_takes_first_colon_subparam() {
        assert_eq!(flat(Params::from_raw(b"4:3;1")), vec![Some(4), Some(1)]);
    }

    #[test]
    fn nested_preserves_colon_groups() {
        assert_eq!(
            nested(Params::from_raw(b"")),
            Vec::<Vec<Option<u32>>>::new()
        );
        assert_eq!(
            nested(Params::from_raw(b"38:2::255:100:50")),
            vec![vec![
                Some(38),
                Some(2),
                None,
                Some(255),
                Some(100),
                Some(50),
            ]]
        );
        assert_eq!(
            nested(Params::from_raw(b"1;31;48:5:200")),
            vec![
                vec![Some(1)],
                vec![Some(31)],
                vec![Some(48), Some(5), Some(200)],
            ]
        );
    }

    #[test]
    fn nested_empty_slots() {
        assert_eq!(
            nested(Params::from_raw(b";1;")),
            vec![vec![None], vec![Some(1)], vec![None]]
        );
        assert_eq!(nested(Params::from_raw(b"4:")), vec![vec![Some(4), None]]);
    }

    #[test]
    fn get_or_returns_default_only_for_absent() {
        // Mirrors `;`-separated layout: [Some(5), None, Some(0), Some(7)]
        let p = Params::from_raw(b"5;;0;7");
        assert_eq!(p.get_or(0, 1), 5);
        assert_eq!(p.get_or(1, 1), 1);
        assert_eq!(p.get_or(2, 1), 0);
        assert_eq!(p.get_or(3, 1), 7);
        assert_eq!(p.get_or(4, 1), 1);
        assert_eq!(Params::EMPTY.get_or(0, 42), 42);
    }

    #[test]
    fn len_counts_groups() {
        assert_eq!(Params::from_raw(b"").len(), 0);
        assert_eq!(Params::from_raw(b"1").len(), 1);
        assert_eq!(Params::from_raw(b";").len(), 2);
        assert_eq!(Params::from_raw(b"1;2;3").len(), 3);
        assert_eq!(Params::from_raw(b"1;").len(), 2);
    }

    #[test]
    fn slice_from_skips_groups() {
        let p = Params::from_raw(b"1;2;3;4");
        assert_eq!(
            flat(p.slice_from(0)),
            vec![Some(1), Some(2), Some(3), Some(4)]
        );
        assert_eq!(flat(p.slice_from(2)), vec![Some(3), Some(4)]);
        assert_eq!(flat(p.slice_from(4)), Vec::<Option<u32>>::new());
        assert_eq!(flat(p.slice_from(99)), Vec::<Option<u32>>::new());
    }

    #[test]
    fn debug_format_matches_flat_list() {
        let p = Params::from_raw(b"1;2");
        assert_eq!(format!("{:?}", p), "[Some(1), Some(2)]");
    }
}
