//! Text shaping: width measurement, segmentation, wrapping policy,
//! and the [`Painter`] for writing strings into any
//! [`crate::buffer::SurfaceMut`].
//!
//! ## Width measurement
//!
//! * [`grapheme_width`] — cluster-aware width for one extended
//!   grapheme cluster (honours VS15/VS16, Regional Indicators, ZWJ,
//!   Extended_Pictographic default presentation).
//! * [`char_width`] — width of a single code point (wcwidth-style,
//!   cluster-blind).
//!
//! Strings are measured via [`WidthMode`] which decides whether each
//! cluster's width comes from the first code point alone
//! ([`WidthMode::Wc`]) or from the full cluster
//! ([`WidthMode::Grapheme`]). The East-Asian Ambiguous policy — whether
//! code points whose East-Asian-Width property is `Ambiguous` are
//! measured as 2 cells or 1 — is orthogonal to segmentation and is
//! passed alongside as a separate `eaw_wide: bool` (see
//! [`char_width`]).
//!
//! ## Wrapping
//!
//! [`WrapMode`] selects what happens when a cluster would cross the
//! right edge of the destination's bounds.
//!
//! ## Painter
//!
//! [`Painter`] binds a target [`crate::buffer::SurfaceMut`] together with a
//! [`WidthMode`] and an `eaw_wide` policy, then paints styled strings —
//! optionally interpreting inline SGR (`CSI … m`) and OSC 8 hyperlinks
//! in the input.
//!
//! ## Backends
//!
//! Default: `unicode-width`. Enable `--features icu` to use
//! `icu_properties` instead (UAX-correct emoji/ZWJ handling, larger
//! binary).

mod mode;
mod painter;
mod surface;
mod width;

pub use mode::{WidthMode, grapheme_cells};
pub use painter::{Painter, WrapMode};
pub use surface::TextSurface;
pub use width::{char_width, grapheme_width};
