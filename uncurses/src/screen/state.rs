//! Terminal state management — cursor, modes, etc.

use crate::ansi::cursor::CursorStyle;
use crate::ansi::mode::{MouseEncoding, MouseMode};

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
    /// Window title.
    pub title: Option<String>,
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
            title: None,
        }
    }
}
