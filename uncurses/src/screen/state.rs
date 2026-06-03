//! Terminal state management — cursor, modes, etc.

use crate::ansi::KittyKeyboardFlags;
use crate::ansi::cursor::CursorStyle;
use crate::ansi::mode::{MouseEncoding, MouseMode};
use crate::color::Color;

/// Tracked terminal state for save/restore.
#[derive(Debug, Clone)]
pub(super) struct State {
    /// Whether we're in the alternate screen.
    pub alt_screen: bool,
    /// Cursor visibility.
    pub cursor_visible: bool,
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
    /// Synchronized updates.
    pub sync_updates: bool,
    /// Unicode core / grapheme cluster mode (DEC 2027). When `true`,
    /// width is calculated per grapheme cluster (UTS-29 + emoji rules);
    /// when `false`, width is calculated per code point (wcwidth-style).
    pub grapheme_clusters: bool,
    /// Color scheme update notifications (DEC 2031). When `true`, the
    /// terminal sends unsolicited reports as the user/OS toggles the
    /// dark/light theme.
    pub color_scheme_updates: bool,
    /// Title (window title set via OSC 0/2). `None` when no title
    /// override has been set.
    pub title: Option<String>,
    /// Default foreground color override. `Some(c)` when the screen
    /// has emitted `OSC 10` to install `c`; `None` when the terminal
    /// is using its built-in default. Drives `OSC 110` on reset and
    /// re-emission on restore.
    pub foreground_color: Option<Color>,
    /// Default background color override. See [`State::foreground_color`].
    pub background_color: Option<Color>,
    /// Cursor color override. See [`State::foreground_color`].
    pub cursor_color: Option<Color>,
    /// Active Kitty keyboard enhancement flag set. The kitty stack is
    /// per-screen-buffer, so the screen re-emits this onto whichever
    /// buffer becomes active. `NONE` means no frame is set.
    pub kitty_keyboard: KittyKeyboardFlags,
}

impl Default for State {
    fn default() -> Self {
        Self {
            alt_screen: false,
            cursor_visible: true,
            cursor_style: CursorStyle::Default,
            mouse_mode: MouseMode::None,
            mouse_encoding: MouseEncoding::X10,
            bracketed_paste: false,
            focus_events: false,
            sync_updates: false,
            grapheme_clusters: false,
            color_scheme_updates: false,
            title: None,
            foreground_color: None,
            background_color: None,
            cursor_color: None,
            kitty_keyboard: KittyKeyboardFlags::NONE,
        }
    }
}
