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
//! The screen is the oracle. It is the only one here that answers without
//! a model of its own: a real terminal consumed the bytes, and what it shows
//! is what a person would see.
//!
//! `UNCURSES_OUTPUT_TRACE` is worth reaching for when a test fails, and it
//! is what found the defect these tests guard. It is not an oracle, though.
//! The frames it records show a move that keeps the column, which is correct
//! or not depending on where the cursor already was, and answering that
//! needs a model of the terminal. A model written here would inherit the
//! assumptions it is meant to check, which is how an earlier attempt at one
//! came to pass on code it was written to fail on.
//!
//! Every test names the change it guards and describes the shape it needs,
//! because these reproductions depend on geometry that is easy to lose: the
//! wide-cluster cursor bug needs a short row beside a wide one, and without
//! that note the fixture reads as arbitrary.
