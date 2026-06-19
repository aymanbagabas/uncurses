//! Terminal state management — cursor, modes, etc.

use crate::ansi::KittyKeyboardFlags;

/// Tracked render-coupled terminal state for save/restore.
#[derive(Debug, Clone)]
pub(super) struct State {
    /// Whether we're in the alternate screen.
    pub alt_screen: bool,
    /// Cursor visibility.
    pub cursor_visible: bool,
    /// Synchronized updates.
    pub sync_updates: bool,
    /// Unicode core / grapheme cluster mode (DEC 2027). When `true`,
    /// width is calculated per grapheme cluster (UTS-29 + emoji rules);
    /// when `false`, width is calculated per code point (wcwidth-style).
    pub grapheme_clusters: bool,
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
            sync_updates: false,
            grapheme_clusters: false,
            kitty_keyboard: KittyKeyboardFlags::NONE,
        }
    }
}
