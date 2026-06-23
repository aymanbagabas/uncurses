//! Grapheme-cluster segmentation, backed by `unicode-rs` or `icu`.
//!
//! See the [`unicode`](super) module docs for backend selection. Both
//! implementations honour Unicode extended grapheme clusters.

/// Iterate over the extended grapheme clusters in a string.
///
/// # Parameters
///
/// - `s`: UTF-8 string slice to segment.
///
/// # Returns
///
/// An iterator of `&str` slices, each covering one extended grapheme
/// cluster from `s` in order.
///
/// # Panics
///
/// Never panics.
///
/// # Usage notes
///
/// The returned slices borrow from `s`; no cell-width classification is
/// performed here.
#[cfg(all(not(feature = "icu"), feature = "unicode-rs"))]
pub fn graphemes(s: &str) -> impl Iterator<Item = &str> {
    use unicode_segmentation::UnicodeSegmentation;
    s.graphemes(true)
}

/// Iterate over the extended grapheme clusters in a string.
///
/// # Parameters
///
/// - `s`: UTF-8 string slice to segment.
///
/// # Returns
///
/// An iterator of `&str` slices, each covering one extended grapheme
/// cluster from `s` in order.
///
/// # Panics
///
/// Never panics.
///
/// # Usage notes
///
/// The returned slices borrow from `s`; no cell-width classification is
/// performed here.
#[cfg(feature = "icu")]
pub fn graphemes(s: &str) -> impl Iterator<Item = &str> {
    use icu_segmenter::GraphemeClusterSegmenter;

    // `new()` is effectively zero-cost with compiled-in data: the returned
    // segmenter is a thin handle.
    let segmenter = GraphemeClusterSegmenter::new();
    let mut iter = segmenter.segment_str(s);
    let mut prev = iter.next().unwrap_or(0);
    core::iter::from_fn(move || {
        let next = iter.next()?;
        let slice = &s[prev..next];
        prev = next;
        Some(slice)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii() {
        let v: Vec<_> = graphemes("abc").collect();
        assert_eq!(v, vec!["a", "b", "c"]);
    }

    #[test]
    fn combining() {
        // "é" as e + combining acute should be a single grapheme.
        let v: Vec<_> = graphemes("e\u{0301}f").collect();
        assert_eq!(v, vec!["e\u{0301}", "f"]);
    }

    #[test]
    fn zwj_emoji() {
        // Family ZWJ sequence — one grapheme.
        let s = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F466}X";
        let v: Vec<_> = graphemes(s).collect();
        assert_eq!(v.len(), 2);
        assert_eq!(v[1], "X");
    }

    #[test]
    fn empty() {
        assert_eq!(graphemes("").count(), 0);
    }
}
