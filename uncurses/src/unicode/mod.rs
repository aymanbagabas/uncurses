//! Unicode text primitives.
//!
//! Low-level Unicode operations shared across the crate, independent of any
//! cell, style, or terminal concern. Today this is grapheme-cluster
//! segmentation; future Unicode helpers (normalization, property lookups, and
//! the like) belong here too.
//!
//! ## Backends
//!
//! Segmentation has two interchangeable backends, selected by Cargo features:
//!
//! - `unicode-rs` (default): a small pure-Rust UAX #29 implementation.
//! - `icu`: ICU4X-backed, faster on emoji/ZWJ-heavy text at the cost of a
//!   larger binary, and takes precedence when both features are enabled.
//!
//! At least one of the two must be enabled; the crate root emits a
//! `compile_error!` otherwise. Both honour Unicode extended grapheme clusters.

mod segment;

pub use segment::graphemes;
