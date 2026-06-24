//! Cell-diff renderer and terminal byte planner.
//!
//! The renderer turns a desired [`RenderBuffer`] into the shortest safe
//! sequence of terminal bytes it can produce for the configured
//! [`Optimizations`]. Most applications use it through
//! [`crate::screen::Screen`]; direct access is useful for lower-level
//! integrations and renderer-focused tests.
//!
//! ## Diff pipeline
//!
//! The renderer keeps a tracked current buffer (`cur_buf`) representing
//! what it believes is on the terminal. The buffer flow first syncs
//! touched desired cells into the renderer-owned staging buffer
//! (`back_buf`), filtering unchanged cells. Rendering then prepares the
//! frame, optionally applies scroll optimizations, transforms touched
//! rows, and finalizes the pen/cursor state.
//!
//! ```text
//! desired cells ─▶ sync_front ─▶ back_buf (actual changes)
//!                                      │
//!                                      ▼
//! cur_buf (tracked screen) ─────▶ row diff / scroll diff
//!                                      │
//!                                      ▼
//!                           cursor + pen planners ─▶ escape bytes
//!                                      │
//!                                      ▼
//!                         cur_buf updated to match terminal
//! ```
//!
//! ## Cursor movement planning
//!
//! Cursor movement is planned by byte cost before any bytes are
//! materialized. Absolute mode seeds CUP and, for local moves, lets
//! relative decompositions compete. Relative mode considers optional
//! prefixes (`none`, carriage return, and in absolute mode home), then
//! vertical and horizontal legs. Horizontal planning can use hardware
//! tabs, CHT/CBT, CUF/CUB, literal backspace, or re-emitting matching
//! cells as an overwrite advance. Forward tabs use the terminal's true
//! next tab stop, not a right-edge-clamped value, so a tab is never
//! chosen unless it lands at or before the target column.
//!
//! ```text
//! from ─▶ prefix? ─▶ vertical leg ─▶ horizontal leg ─▶ to
//!          │             │                 │
//!          │             │                 ├─ CUF/CUB, BS, tabs, CHT/CBT
//!          │             └─ LF/CUD/CUU/RI/VPA
//!          └─ none / CR / HOME (when eligible)
//! ```
//!
//! ## Pen and SGR state
//!
//! The renderer tracks the active style ("pen") alongside cursor
//! position. Before emitting a cell it writes only the style difference
//! needed from the current pen to the target cell style, including OSC 8
//! hyperlink open/close transitions. At frame end the pen is reset to
//! default only if the terminal state is not already default.
//!
//! ## Tab stops
//!
//! `TabStops` stores configurable horizontal tab stops and
//! precomputes previous, clamped-next, and unclamped-next lookup tables.
//! The unclamped next stop models the column a terminal tab would really
//! reach beyond the surface edge; cursor planning depends on that to
//! avoid over-counting forward tab moves near the right margin.

pub(crate) mod buffer;
pub(crate) mod caps;
pub(crate) mod color_cache;
pub(crate) mod cursor;
#[cfg(test)]
mod cursor_planner_tests;
pub(crate) mod frame;
pub(crate) mod hash;
pub(crate) mod pen;
pub(crate) mod scroll;
pub(crate) mod state;
pub(crate) mod sync;
pub(crate) mod tabstops;
pub(crate) mod transform;

#[cfg(test)]
mod transform_branch_tests;

#[cfg(test)]
#[path = "tests/golden.rs"]
mod golden;

#[cfg(test)]
#[path = "tests/render.rs"]
mod render_tests;

#[cfg(test)]
#[path = "tests/optimizations_golden.rs"]
mod optimizations_golden;

pub use buffer::RenderBuffer;
pub use caps::Optimizations;
pub use state::Renderer;
