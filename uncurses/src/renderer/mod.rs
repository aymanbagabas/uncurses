//! Cell-diff renderer internals.
//!
//! This module compares a front [`RenderBuffer`] with tracked terminal state and emits the minimal cell, style, cursor, and scroll changes it can.
//! Most users reach for [`crate::canvas::Canvas`] instead, while this module is useful for lower-level rendering tests and integrations that need direct access to [`Renderer`] or [`Optimizations`].

pub(crate) mod buffer;
pub(crate) mod caps;
pub(crate) mod color_cache;
pub(crate) mod cursor;
#[cfg(test)]
mod cursor_planner_tests;
pub(crate) mod frame;
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
