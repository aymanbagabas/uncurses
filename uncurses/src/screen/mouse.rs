//! High-level mouse tracking mode and encoding selection for the
//! [`Screen`](super::Screen) facade.
//!
//! These types abstract over the underlying DEC private modes (the
//! `MOUSE_*` constants in [`crate::ansi::mode`]); the facade maps the
//! chosen mode and encoding to those low-level modes when enabling or
//! restoring mouse tracking. They are private to the facade.

use crate::ansi::mode::Mode;

/// Mouse tracking mode: which pointer activity the terminal reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum MouseMode {
    /// Normal tracking: button press and release.
    Normal,
    /// Button-event tracking: presses, releases, and motion while a button
    /// is held.
    Button,
    /// Any-event tracking: presses, releases, and all motion.
    Any,
}

/// Mouse coordinate encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum MouseEncoding {
    /// Legacy encoding (coordinates limited to 223).
    X10,
    /// SGR encoding: cell coordinates, unbounded.
    Sgr,
    /// SGR-pixel encoding: pixel coordinates.
    SgrPixel,
}

impl MouseMode {
    /// The DEC private mode for this tracking mode.
    pub(super) fn dec_mode(self) -> Mode {
        match self {
            MouseMode::Normal => Mode::MOUSE_NORMAL,
            MouseMode::Button => Mode::MOUSE_BUTTON,
            MouseMode::Any => Mode::MOUSE_ANY,
        }
    }
}

impl MouseEncoding {
    /// The DEC private mode for this encoding, or `None` for the legacy
    /// [`MouseEncoding::X10`] (no mode to set).
    pub(super) fn dec_mode(self) -> Option<Mode> {
        match self {
            MouseEncoding::X10 => None,
            MouseEncoding::Sgr => Some(Mode::MOUSE_SGR),
            MouseEncoding::SgrPixel => Some(Mode::MOUSE_SGR_PIXEL),
        }
    }
}
