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
use crate::ansi::mode::{Mode, ModeSetting};
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

/// What the terminal told us about itself.
///
/// This holds the replies themselves, not a summary of them: the
/// [`ModeSetting`] reported for every mode that was asked about, the raw
/// Primary DA attribute list, the Kitty keyboard flags, and the
/// modifyOtherKeys mode. A reply that says "I do not recognize that" is
/// recorded too, which is why the accessors return [`Option`] — `None` means
/// the terminal never answered, which is different from answering no.
///
/// The facade records these as reply events flow through
/// [`read_event`](super::Program::read_event) /
/// [`try_read_event`](super::Program::try_read_event). Nothing lands here
/// unless you ask: see
/// [`query_capabilities`](super::Program::query_capabilities). Read it back
/// with [`Program::capabilities`](super::Program::capabilities).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    modes: BTreeMap<Mode, ModeSetting>,
    primary_device_attributes: Option<Vec<Option<u32>>>,
    kitty_keyboard: Option<KittyKeyboardFlags>,
    modify_other_keys: Option<ModifyOtherKeysMode>,
    terminal_name: Option<String>,
    true_color: bool,
}

impl Capabilities {
    /// The [`ModeSetting`] the terminal reported for `mode`, or `None` if it
    /// never reported on that mode.
    ///
    /// Use this when the distinction matters: [`ModeSetting::Set`] means the
    /// mode is currently on, [`ModeSetting::PermanentlySet`] means it cannot
    /// be turned off, and [`ModeSetting::NotRecognized`] is a definite "no"
    /// rather than silence.
    pub fn mode(&self, mode: Mode) -> Option<ModeSetting> {
        self.modes.get(&mode).copied()
    }

    /// Whether the terminal reported `mode` as available, in any state.
    ///
    /// ```ignore
    /// use uncurses::ansi::mode::Mode;
    ///
    /// if program.capabilities().supports(Mode::MOUSE_SGR_PIXEL) {
    ///     // pixel-accurate mouse reporting is available
    /// }
    /// ```
    pub fn supports(&self, mode: Mode) -> bool {
        self.mode(mode).is_some_and(ModeSetting::is_available)
    }

    /// Every mode report recorded so far, keyed by mode.
    pub fn modes(&self) -> &BTreeMap<Mode, ModeSetting> {
        &self.modes
    }

    /// The raw Primary DA attribute list, or `None` if the terminal never
    /// answered. Entries are `None` where the terminal sent an empty
    /// parameter.
    pub fn primary_device_attributes(&self) -> Option<&[Option<u32>]> {
        self.primary_device_attributes.as_deref()
    }

    /// Whether the Primary DA reply advertised `attribute`.
    pub fn da_attribute(&self, attribute: u32) -> bool {
        self.primary_device_attributes
            .as_ref()
            .is_some_and(|attrs| attrs.contains(&Some(attribute)))
    }

    /// Sixel graphics support (Primary DA attribute 4).
    pub fn sixel(&self) -> bool {
        self.da_attribute(4)
    }

    /// Clipboard access (Primary DA attribute 52).
    pub fn clipboard(&self) -> bool {
        self.da_attribute(52)
    }

    /// The Kitty keyboard enhancements the terminal reported, or `None` if it
    /// never answered `CSI ? u`. An answer of
    /// [`empty`](KittyKeyboardFlags::empty) means the protocol is supported
    /// with no enhancements currently active.
    pub fn kitty_keyboard(&self) -> Option<KittyKeyboardFlags> {
        self.kitty_keyboard
    }

    /// The xterm modifyOtherKeys mode the terminal reported, or `None` if it
    /// never answered `CSI ? 4 m`.
    pub fn modify_other_keys(&self) -> Option<ModifyOtherKeysMode> {
        self.modify_other_keys
    }

    /// The terminal's self-reported name from XTVERSION (for example
    /// `"XTerm(380)"`), or `None` if it never answered.
    pub fn terminal_name(&self) -> Option<&str> {
        self.terminal_name.as_deref()
    }

    /// Direct (24-bit) color, confirmed by an XTGETTCAP `RGB`/`Tc` reply.
    ///
    /// Unlike the others this is not a reply value but a fact derived from
    /// one, because the reply is a capability string rather than a state.
    pub fn true_color(&self) -> bool {
        self.true_color
    }

    /// Record a mode report.
    pub(super) fn set_mode(&mut self, mode: Mode, setting: ModeSetting) {
        self.modes.insert(mode, setting);
    }

    /// Record the Primary DA attribute list.
    pub(super) fn set_primary_device_attributes(&mut self, attrs: Vec<Option<u32>>) {
        self.primary_device_attributes = Some(attrs);
    }

    /// Record the reported Kitty keyboard enhancements.
    pub(super) fn set_kitty_keyboard(&mut self, flags: KittyKeyboardFlags) {
        self.kitty_keyboard = Some(flags);
    }

    /// Record the reported modifyOtherKeys mode.
    pub(super) fn set_modify_other_keys(&mut self, mode: ModifyOtherKeysMode) {
        self.modify_other_keys = Some(mode);
    }

    /// Record the terminal's XTVERSION name.
    pub(super) fn set_terminal_name(&mut self, name: String) {
        self.terminal_name = Some(name);
    }

    /// Record confirmed direct-color support.
    pub(super) fn set_true_color(&mut self) {
        self.true_color = true;
    }
}
