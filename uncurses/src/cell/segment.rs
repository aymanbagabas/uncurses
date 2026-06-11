//! Grapheme-cluster segmentation abstraction.
//!
//! Selecting a backend:
//!
//! - default (`unicode-rs` feature): small pure-Rust UAX #29
//!   implementation.
//! - `icu`: faster on emoji/ZWJ-heavy text at the cost of a larger
//!   binary (segmentation tables are baked in). Wins over
//!   `unicode-rs` when both are enabled.
//!
//! At least one of the two features must be enabled — the crate
//! root emits a `compile_error!` otherwise.
//!
//! Both implementations honour Unicode extended grapheme clusters.

/// Iterate over the extended grapheme clusters of `s`.
#[cfg(all(not(feature = "icu"), feature = "unicode-rs"))]
pub fn graphemes(s: &str) -> impl Iterator<Item = &str> {
    use unicode_segmentation::UnicodeSegmentation;
    s.graphemes(true)
}

/// Iterate over the extended grapheme clusters of `s`.
#[cfg(feature = "icu")]
pub fn graphemes(s: &str) -> impl Iterator<Item = &str> {
    use icu_segmenter::GraphemeClusterSegmenter;

    // `new()` with the `compiled_data` feature is effectively zero-cost: data
    // is baked into the binary and the returned segmenter is a thin handle.
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
