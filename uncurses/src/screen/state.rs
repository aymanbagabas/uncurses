//! Non-render terminal/input mode state owned by the [`Screen`] facade.
//!
//! These modes do not affect how the [`Canvas`] measures, renders, or
//! presents a frame — they configure the terminal device and the input
//! reader. The facade tracks them so it can tear them down on a shell
//! handoff and re-apply them afterwards.
//!
//! [`Screen`]: super::Screen
//! [`Canvas`]: crate::canvas::Canvas

use std::collections::BTreeMap;

use crate::ansi::cursor::CursorStyle;
use crate::color::Color;
use crate::event::ModifyOtherKeysMode;

use super::mouse::{MouseEncoding, MouseMode};

/// Tracked non-render mode state for save/restore.
#[derive(Debug, Clone)]
pub(super) struct State {
    /// Cursor style.
    pub cursor_style: CursorStyle,
    /// Mouse tracking mode, or `None` when mouse tracking is disabled.
    pub mouse_mode: Option<MouseMode>,
    /// Mouse coordinate encoding.
    pub mouse_encoding: MouseEncoding,
    /// Bracketed paste mode.
    pub bracketed_paste: bool,
    /// Focus in/out reporting (DECSET 1004).
    pub focus_events: bool,
    /// Color-theme update notifications (DEC 2031). When `true`, the
    /// terminal sends unsolicited reports as the user/OS toggles the
    /// dark/light theme. Reports the dark/light preference only, not the
    /// actual colors.
    pub color_theme_updates: bool,
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
    /// Indexed palette overrides set via `OSC 4`, keyed by palette index.
    /// Drives `OSC 104 ; index` on reset and re-emission on restore.
    pub palette: BTreeMap<u8, Color>,
    /// Active xterm modifyOtherKeys mode (`CSI > 4 ; n m`). Drives
    /// `CSI > 4 m` on reset and re-emission on restore.
    pub modify_other_keys: ModifyOtherKeysMode,
    /// Pointer (mouse cursor) shape override set via `OSC 22`. `None` when
    /// using the terminal default. Drives the `OSC 22` reset on reset and
    /// re-emission on restore.
    pub pointer_shape: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            cursor_style: CursorStyle::Default,
            mouse_mode: None,
            mouse_encoding: MouseEncoding::X10,
            bracketed_paste: false,
            focus_events: false,
            color_theme_updates: false,
            in_band_resize: false,
            title: None,
            foreground_color: None,
            background_color: None,
            cursor_color: None,
            palette: BTreeMap::new(),
            modify_other_keys: ModifyOtherKeysMode::Disabled,
            pointer_shape: None,
        }
    }
}

/// Terminal capabilities detected from the replies to the queries
/// [`Screen::init`](super::Screen::init) fires. Every field answers a
/// single question: does the terminal support this? The facade intercepts
/// the reply events as they flow through
/// [`read_event`](super::Screen::read_event) / [`try_read_event`](super::Screen::try_read_event),
/// records support here, and applies the render-affecting ones — the app
/// never sees the reply events. Read back with
/// [`Screen::capabilities`](super::Screen::capabilities).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// Synchronized output (DEC private mode 2026). Applied: frames are
    /// wrapped in begin/end-synchronized-update markers.
    pub synchronized_output: bool,
    /// Unicode core / grapheme-cluster mode (DEC private mode 2027).
    /// Applied: cell widths are measured per grapheme cluster.
    pub grapheme_clusters: bool,
    /// In-band resize notifications (DEC private mode 2048).
    pub in_band_resize: bool,
    /// Normal mouse button tracking (DEC private mode 1000).
    pub mouse_normal: bool,
    /// Button-event mouse tracking (DEC private mode 1002).
    pub mouse_button: bool,
    /// Any-event mouse tracking (DEC private mode 1003).
    pub mouse_any: bool,
    /// SGR mouse encoding (DEC private mode 1006).
    pub mouse_sgr: bool,
    /// SGR-pixel mouse encoding (DEC private mode 1016).
    pub mouse_sgr_pixel: bool,
    /// Sixel graphics (Primary DA attribute 4).
    pub sixel: bool,
    /// Clipboard access (Primary DA attribute 52).
    pub clipboard: bool,
    /// Kitty keyboard protocol (the terminal answered `CSI ? u`).
    pub kitty_keyboard: bool,
    /// xterm modifyOtherKeys (the terminal answered `CSI ? 4 m`).
    pub modify_other_keys: bool,
    /// Direct (24-bit) color, confirmed by an XTGETTCAP `RGB`/`Tc` reply.
    /// Applied: the renderer's color profile is upgraded to
    /// [`Profile::TrueColor`](crate::color::Profile::TrueColor).
    pub true_color: bool,
}
