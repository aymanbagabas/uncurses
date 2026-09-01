//! Terminal-level tests for uncurses.
//!
//! The unit tests in `uncurses` assert on the bytes the renderer emits,
//! which is what that crate owns. They cannot say what those bytes do to a
//! terminal, and some defects only appear there: a run can land the cursor
//! correctly and still write the wrong cells, and a cursor can drift a
//! column without any single frame looking wrong.
//!
//! These tests answer that question with a real terminal. Each drives a
//! program through a pty and reads the screen back, so what it asserts is
//! what a person would see.
//!
//! Two oracles, used for different questions:
//!
//! - the screen, for what the terminal ended up showing
//! - `UNCURSES_OUTPUT_TRACE`, for the bytes that put it there, one entry per
//!   flush, which is how a cursor that drifts a column is caught before the
//!   frame that reveals it
//!
//! Every test names the change it guards and describes the shape it needs,
//! because these reproductions depend on geometry that is easy to lose: the
//! wide-cluster cursor bug needs a short row beside a wide one, and without
//! that note the fixture reads as arbitrary.
