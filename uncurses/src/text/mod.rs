//! Text measurement and string painting for terminal-cell surfaces.
//!
//! This module is the text layer for [`SurfaceMut`](crate::buffer::SurfaceMut):
//! it segments UTF-8 strings into grapheme clusters, measures their terminal
//! display width, interprets inline styling escapes, and writes cells into any
//! mutable surface implementation.
//!
//! ## Width measurement and grapheme handling
//!
//! Terminal layout is expressed in cells, not bytes or scalar values. The
//! width API therefore separates **segmentation** from **measurement**:
//!
//! * [`grapheme_cells`] always walks a string as extended grapheme clusters.
//! * [`WidthMode::Wc`] measures each cluster by its first code point.
//! * [`WidthMode::Grapheme`] measures the whole cluster with
//!   [`grapheme_width`], including variation selectors, regional indicators,
//!   zero-width joiners, and pictographic presentation.
//! * [`char_width`] measures a single code point and is useful for code that
//!   already has its own segmentation.
//!
//! ```text
//! bytes/scalars             grapheme cluster              terminal cells
//! ┌───────────────┐         ┌───────────────┐             ┌────┬────┐
//! │ "e" + U+0301  │ ──────▶ │ "e\u{0301}"   │ ─ width 1 ▶ │ é  │    │
//! └───────────────┘         └───────────────┘             └────┴────┘
//!
//! ┌───────────────┐         ┌───────────────┐             ┌────┬────┐
//! │ "中"          │ ──────▶ │ "中"          │ ─ width 2 ▶ │ 中 │ ▶  │
//! └───────────────┘         └───────────────┘             └────┴────┘
//! ```
//!
//! A width of `0` is not written as a standalone cell. While painting, a
//! zero-width cluster is appended to the previous pending cluster so combining
//! marks and similar suffixes stay attached to the base cell.
//!
//! ## East-Asian-Width policy
//!
//! The `eaw_wide` boolean is the East-Asian Ambiguous policy. When it is
//! `true`, code points whose East-Asian-Width property is `Ambiguous` are
//! measured as two cells; when it is `false`, they are measured as one. This is
//! intentionally independent from [`WidthMode`] so callers can choose the
//! terminal's ambiguous-width policy without changing grapheme segmentation.
//!
//! ## Painting and wrapping
//!
//! [`Painter`] binds a target [`SurfaceMut`](crate::buffer::SurfaceMut), a
//! [`WidthMode`], and an `eaw_wide` policy. It paints styled strings into the
//! target, honoring inline SGR (`CSI … m`) attributes and OSC 8 hyperlinks.
//! [`WrapMode`] controls only what happens when a non-zero-width cluster would
//! cross the right edge of the clipping rectangle: truncate or continue at the
//! next row.
//!
//! ## The `TextSurface` trait
//!
//! [`TextSurface`] is the ergonomic extension trait for drawing text onto any
//! surface. A surface supplies its [`WidthMode`] and East-Asian-Width policy;
//! the trait then provides [`TextSurface::set_str`],
//! [`TextSurface::set_str_wrap`], [`TextSurface::set_str_rect`],
//! [`TextSurface::set_str_rect_wrap`], [`TextSurface::str_width`], and
//! [`TextSurface::painter`]. This keeps higher-level widgets generic over
//! `&mut impl TextSurface` instead of depending on a concrete buffer type.
//!
//! ## Feature backends
//!
//! The default `unicode-rs` feature uses compact built-in tables for code-point
//! width plus a conservative pictographic/default-ignorable subset. Enabling
//! the `icu` feature uses property data with broader Unicode coverage. The
//! public API is identical for both backends; select the backend that matches
//! your binary-size and Unicode-coverage needs.

mod display;
mod mode;
mod painter;
mod surface;
mod width;

pub use display::{Encode, SurfaceDisplay};
pub use mode::{WidthMode, grapheme_cells};
pub use painter::{Painter, WrapMode};
pub use surface::TextSurface;
pub use width::{char_width, grapheme_width};
