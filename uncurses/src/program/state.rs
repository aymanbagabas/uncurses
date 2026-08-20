//! Terminal/input mode state owned by the [`Program`] facade.
//!
//! Every field records a mode the facade has *emitted*, so it can tear the
//! mode down on a shell handoff and re-apply it afterwards.
//!
//! Three of them — [`alt_screen`](State::alt_screen),
//! [`cursor_visible`](State::cursor_visible), and
//! [`grapheme_clusters`](State::grapheme_clusters) — are mirrored by a render
//! property on the [`Screen`](crate::screen::Screen) the facade draws with.
//! They are tracked separately on purpose: the screen's copy says how to draw
//! a frame, this one says what the terminal was told. Since
//! [`screen_mut`](super::Program::screen_mut) lets an app move the render
//! property on its own, inferring one from the other would make
//! [`reset`](super::Program::reset) emit modes that were never set (or skip
//! ones that were) and leave the terminal wedged after exit.
//!
//! [`Program`]: super::Program

use std::collections::BTreeMap;

use crate::ansi::cursor::CursorStyle;
use crate::ansi::kitty::KittyKeyboardFlags;
use crate::color::Color;
use crate::event::ModifyOtherKeysMode;

use super::MouseTracking;
use super::ProgressState;

/// Tracked non-render mode state for save/restore.
#[derive(Debug, Clone)]
pub(super) struct State {
    /// Cursor style.
    pub cursor_style: CursorStyle,
    /// Requested mouse tracking, or `None` when mouse tracking is disabled.
    pub mouse: Option<MouseTracking>,
    /// Bracketed paste mode.
    pub bracketed_paste: bool,
    /// Focus in/out reporting (DECSET 1004).
    pub focus_events: bool,
    /// Color-scheme update notifications (DEC 2031). When `true`, the
    /// terminal sends unsolicited reports as the user/OS toggles the
    /// dark/light scheme. Reports the dark/light preference only, not the
    /// actual colors.
    pub color_scheme_updates: bool,
    /// In-band resize notifications (DEC 2048). When `true`, the
    /// terminal sends a `CSI 48 ; … t` report whenever the surface
    /// changes size, surfaced as [`Event::Resize`].
    ///
    /// [`Event::Resize`]: crate::event::Event::Resize
    pub in_band_resize: bool,
    /// Window title set via [`OSC 2`] (or [`OSC 0`], which sets both this and
    /// [`icon_name`](Self::icon_name)). `None` when no
    /// [`set_window_title`](super::Program::set_window_title) or
    /// [`set_title`](super::Program::set_title) override has been set.
    ///
    /// [`OSC 2`]: crate::ansi::title::write_window_title
    /// [`OSC 0`]: crate::ansi::title::write_window_title_and_icon
    pub window_title: Option<String>,
    /// Icon name set via [`OSC 1`] (or [`OSC 0`], which sets both this and
    /// [`window_title`](Self::window_title)). `None` when no
    /// [`set_icon_title`](super::Program::set_icon_title) or
    /// [`set_title`](super::Program::set_title) override has been set.
    ///
    /// [`OSC 1`]: crate::ansi::title::write_icon_name
    /// [`OSC 0`]: crate::ansi::title::write_window_title_and_icon
    pub icon_name: Option<String>,
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
    /// Progress reported via `OSC 9;4`. `None` when no progress is being
    /// reported. Drives the `OSC 9;4;0` removal on reset and re-emission on
    /// restore.
    pub progress: Option<ProgressState>,
    /// Active Kitty keyboard enhancement flag set. The stack is
    /// per-screen-buffer, so the program re-emits this onto whichever buffer
    /// becomes active. `NONE` means no frame is set.
    pub kitty_keyboard: KittyKeyboardFlags,
    /// Whether the facade has put the terminal on the alternate screen buffer
    /// (DECSET 1049). Mirrors [`Screen::fullscreen`](crate::screen::Screen::fullscreen)
    /// while the app drives the buffer through
    /// [`enter_alt_screen`](super::Program::enter_alt_screen) /
    /// [`exit_alt_screen`](super::Program::exit_alt_screen).
    pub alt_screen: bool,
    /// Whether the terminal cursor is visible (DECTCEM). Mirrors
    /// [`Screen::cursor_visible`](crate::screen::Screen::cursor_visible).
    /// Starts `true`: a terminal shows its cursor until told otherwise.
    pub cursor_visible: bool,
    /// Whether grapheme-cluster mode is on (DEC 2027). Mirrors
    /// [`Screen::grapheme_clusters`](crate::screen::Screen::grapheme_clusters).
    pub grapheme_clusters: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            cursor_style: CursorStyle::Default,
            mouse: None,
            bracketed_paste: false,
            focus_events: false,
            color_scheme_updates: false,
            in_band_resize: false,
            window_title: None,
            icon_name: None,
            foreground_color: None,
            background_color: None,
            cursor_color: None,
            palette: BTreeMap::new(),
            modify_other_keys: ModifyOtherKeysMode::Disabled,
            pointer_shape: None,
            progress: None,
            kitty_keyboard: KittyKeyboardFlags::empty(),
            alt_screen: false,
            cursor_visible: true,
            grapheme_clusters: false,
        }
    }
}

/// Terminal capabilities, as reported by the replies the terminal has sent.
/// Every field answers a single question: does the terminal support this?
/// Nothing is queried at startup, so the replies arrive because the caller
/// asked, with [`query_capabilities`](super::Program::query_capabilities) or
/// an individual `request_*` method. Reading with
/// [`read_event`](super::Program::read_event) /
/// [`try_read_event`](super::Program::try_read_event) records support here and
/// applies the few noted below as `Applied:` on the way past; the event is
/// still handed back to the application. Read back with
/// [`Program::capabilities`](super::Program::capabilities).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// Synchronized output (DEC private mode 2026). Applied: frames are
    /// wrapped in begin/end-synchronized-update markers.
    pub synchronized_output: bool,
    /// Unicode core / grapheme-cluster mode (DEC private mode 2027).
    /// Detected only: call
    /// [`enable_grapheme_clusters`](super::Program::enable_grapheme_clusters)
    /// to act on it.
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
