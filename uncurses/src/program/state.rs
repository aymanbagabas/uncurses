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
use crate::event::{ColorScheme, ModifyOtherKeysMode};

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
/// device-attribute lists, the reported colors, and so on. A reply that says
/// "I do not recognize that" is recorded too, which is why the accessors
/// return [`Option`] — `None` means the terminal never answered, which is
/// different from answering no.
///
/// Everything here is what the terminal reported, never what the facade told
/// it. The two are easy to confuse where both exist: a
/// [`background_color`](Self::background_color) recorded here stays the
/// terminal's own default even after
/// [`set_background_color`](super::Program::set_background_color) overrides
/// it.
///
/// Only replies land here, so questions the environment can also answer are
/// deliberately absent. Direct-color support is the example: `COLORTERM` and
/// `TERM` establish it as readily as an XTGETTCAP reply does, so the answer is
/// [`Screen::color_profile`](crate::screen::Screen::color_profile), which
/// folds in both, and what remains here is the reply itself via
/// [`supports_termcap`](Self::supports_termcap).
///
/// The facade records these as reply events flow through
/// [`read_event`](super::Program::read_event) /
/// [`try_read_event`](super::Program::try_read_event). Nothing lands here
/// unless you ask: see
/// [`query_capabilities`](super::Program::query_capabilities), which asks for
/// only some of this, and takes extra bytes so you can ask for the rest. Read
/// it back with [`Program::capabilities`](super::Program::capabilities).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub(super) modes: BTreeMap<Mode, ModeSetting>,
    pub(super) primary_device_attributes: Option<Vec<Option<u32>>>,
    pub(super) secondary_device_attributes: Option<Vec<Option<u32>>>,
    pub(super) tertiary_device_attributes: Option<String>,
    pub(super) kitty_keyboard: Option<KittyKeyboardFlags>,
    pub(super) modify_other_keys: Option<ModifyOtherKeysMode>,
    pub(super) terminal_name: Option<String>,
    pub(super) termcap: BTreeMap<String, Option<String>>,
    pub(super) foreground_color: Option<Color>,
    pub(super) background_color: Option<Color>,
    pub(super) cursor_color: Option<Color>,
    pub(super) palette: BTreeMap<u8, Color>,
    pub(super) color_scheme: Option<ColorScheme>,
    pub(super) kitty_graphics: bool,
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

    /// The raw Primary DA (`CSI c`) attribute list, or `None` if the terminal
    /// never answered. Entries are `None` where the terminal sent an empty
    /// parameter.
    pub fn primary_device_attributes(&self) -> Option<&[Option<u32>]> {
        self.primary_device_attributes.as_deref()
    }

    /// The raw Secondary DA (`CSI > c`) attribute list, or `None` if the
    /// terminal never answered. Conventionally terminal type, firmware
    /// version, and hardware option, but the meaning of each entry varies by
    /// terminal, so it is reported unparsed.
    pub fn secondary_device_attributes(&self) -> Option<&[Option<u32>]> {
        self.secondary_device_attributes.as_deref()
    }

    /// The Tertiary DA (`CSI = c`) terminal unit ID, or `None` if the
    /// terminal never answered.
    pub fn tertiary_device_attributes(&self) -> Option<&str> {
        self.tertiary_device_attributes.as_deref()
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

    /// Whether the terminal has answered a Kitty graphics query, which is the
    /// protocol's own support test: a terminal that does not implement it
    /// stays silent.
    pub fn kitty_graphics(&self) -> bool {
        self.kitty_graphics
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

    /// The value the terminal reported for the XTGETTCAP capability `name`,
    /// or `None` if it reported the capability as unsupported or was never
    /// asked. Boolean capabilities report an empty string, so use
    /// [`supports_termcap`](Self::supports_termcap) to test for presence.
    pub fn termcap(&self, name: &str) -> Option<&str> {
        self.termcap.get(name)?.as_deref()
    }

    /// Whether the terminal reported the XTGETTCAP capability `name` as
    /// supported. `false` both for a capability reported unsupported and for
    /// one never asked about; tell them apart with
    /// [`termcap_reports`](Self::termcap_reports).
    pub fn supports_termcap(&self, name: &str) -> bool {
        matches!(self.termcap.get(name), Some(Some(_)))
    }

    /// Every XTGETTCAP reply recorded so far, keyed by capability name. A
    /// value of `None` is the terminal reporting that capability as
    /// unsupported, which is different from the key being absent.
    pub fn termcap_reports(&self) -> &BTreeMap<String, Option<String>> {
        &self.termcap
    }

    /// The terminal's default foreground color (`OSC 10`), or `None` if it
    /// never answered.
    pub fn foreground_color(&self) -> Option<Color> {
        self.foreground_color
    }

    /// The terminal's default background color (`OSC 11`), or `None` if it
    /// never answered.
    pub fn background_color(&self) -> Option<Color> {
        self.background_color
    }

    /// The terminal's cursor color (`OSC 12`), or `None` if it never
    /// answered.
    pub fn cursor_color(&self) -> Option<Color> {
        self.cursor_color
    }

    /// The color the terminal reported for palette entry `index`
    /// (`OSC 4 ; index ; ?`), or `None` if it never answered for that entry.
    pub fn palette_color(&self, index: u8) -> Option<Color> {
        self.palette.get(&index).copied()
    }

    /// Every palette color reported so far, keyed by index.
    pub fn palette(&self) -> &BTreeMap<u8, Color> {
        &self.palette
    }

    /// Whether the terminal is in its dark or light scheme (DEC mode 2031),
    /// or `None` if it never reported one. Updated as the scheme changes
    /// while [`enable_color_scheme_updates`](super::Program::enable_color_scheme_updates)
    /// is on. This is the terminal's own preference flag, which a terminal
    /// can report independently of the colors it uses; when it is absent,
    /// [`background_color`](Self::background_color) is the fallback.
    pub fn color_scheme(&self) -> Option<ColorScheme> {
        self.color_scheme
    }
}
