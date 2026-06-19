//! Non-render terminal/input mode state owned by the [`Screen`] facade.
//!
//! These modes do not affect how the [`Canvas`] measures, renders, or
//! presents a frame — they configure the terminal device and the input
//! reader. The facade tracks them so it can tear them down on a shell
//! handoff and re-apply them afterwards.
//!
//! [`Screen`]: super::Screen
//! [`Canvas`]: crate::canvas::Canvas

use crate::ansi::cursor::CursorStyle;
use crate::ansi::mode::{MouseEncoding, MouseMode};
use crate::color::Color;

/// Tracked non-render mode state for save/restore.
#[derive(Debug, Clone)]
pub(super) struct State {
    /// Cursor style.
    pub cursor_style: CursorStyle,
    /// Mouse tracking mode.
    pub mouse_mode: MouseMode,
    /// Mouse encoding.
    pub mouse_encoding: MouseEncoding,
    /// Bracketed paste mode.
    pub bracketed_paste: bool,
    /// Focus in/out reporting (DECSET 1004).
    pub focus_events: bool,
    /// Color scheme update notifications (DEC 2031). When `true`, the
    /// terminal sends unsolicited reports as the user/OS toggles the
    /// dark/light theme.
    pub color_scheme_updates: bool,
    /// In-band resize notifications (DEC 2048). When `true`, the
    /// terminal sends a `CSI 48 ; … t` report whenever the surface
    /// changes size, surfaced as [`Event::Resize`].
    ///
    /// [`Event::Resize`]: crate::event::Event::Resize
    pub in_band_resize: bool,
    /// Title (window title set via OSC 0/2). `None` when no title
    /// override has been set.
    pub title: Option<String>,
    /// Default foreground color override. `Some(c)` when the facade has
    /// emitted `OSC 10` to install `c`; `None` when the terminal is
    /// using its built-in default. Drives `OSC 110` on reset and
    /// re-emission on restore.
    pub foreground_color: Option<Color>,
    /// Default background color override. See [`State::foreground_color`].
    pub background_color: Option<Color>,
    /// Cursor color override. See [`State::foreground_color`].
    pub cursor_color: Option<Color>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            cursor_style: CursorStyle::Default,
            mouse_mode: MouseMode::None,
            mouse_encoding: MouseEncoding::X10,
            bracketed_paste: false,
            focus_events: false,
            color_scheme_updates: false,
            in_band_resize: false,
            title: None,
            foreground_color: None,
            background_color: None,
            cursor_color: None,
        }
    }
}
