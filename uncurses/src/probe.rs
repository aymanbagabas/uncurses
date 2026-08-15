//! Asking the terminal questions and understanding the answers.
//!
//! A probe pairs the bytes that ask with the arm that listens, so the two
//! cannot drift apart: change what you ask for and forget the matching arm, and
//! the field silently stays `None`. There is one probe per query the
//! [`ansi`](crate::ansi) layer speaks, each named for the reply it folds.
//!
//! Nothing here needs a [`Screen`](crate::screen::Screen). A probe writes into
//! any [`Write`] and folds any [`Event`], so it works just as well over an
//! [`EventSource`](crate::event::EventSource) and a raw terminal handle.
//!
//! # Build your capabilities out of these
//!
//! Hold the probes you need as fields and forward to them. The composed type is
//! itself a [`Probe`], so it goes wherever a single one does.
//!
//! ```no_run
//! use std::io::{self, Write};
//! use uncurses::ansi::mode::Mode;
//! use uncurses::event::Event;
//! use uncurses::probe::{self, Probe};
//!
//! struct Capabilities {
//!     background: probe::BackgroundColor,
//!     palette: probe::PaletteColor,
//!     modes: probe::ModeReport,
//! }
//!
//! impl Default for Capabilities {
//!     fn default() -> Self {
//!         Self {
//!             background: probe::BackgroundColor::default(),
//!             palette: probe::PaletteColor::ansi(),
//!             modes: probe::ModeReport::new([
//!                 Mode::LEFT_RIGHT_MARGIN,
//!                 Mode::FOCUS,
//!                 Mode::MOUSE_SGR_PIXEL,
//!             ]),
//!         }
//!     }
//! }
//!
//! impl Probe for Capabilities {
//!     fn write_queries(&self, out: &mut dyn Write) -> io::Result<()> {
//!         self.background.write_queries(out)?;
//!         self.palette.write_queries(out)?;
//!         self.modes.write_queries(out)
//!     }
//!
//!     fn observe_event(&mut self, event: &Event) -> io::Result<()> {
//!         self.background.observe_event(event)?;
//!         self.palette.observe_event(event)?;
//!         self.modes.observe_event(event)
//!     }
//! }
//!
//! # fn read(caps: &Capabilities) {
//! let columns = caps.modes.is_available(Mode::LEFT_RIGHT_MARGIN);
//! let pixels = caps.modes.is_available(Mode::MOUSE_SGR_PIXEL);
//! let bg = caps.background.0;
//! # }
//! ```
//!
//! Write a [`observe_event`](Probe::observe_event) arm by hand only for
//! sequences this crate does not decode; those arrive as
//! [`UnknownCsi`](Event::UnknownCsi), [`UnknownOsc`](Event::UnknownOsc) and
//! friends, carrying their raw bytes.
//!
//! # On its own
//!
//! ```no_run
//! # use std::io::{self, Write};
//! # use uncurses::event::Event;
//! # use uncurses::probe::Probe;
//! # #[derive(Default)] struct Capabilities;
//! # impl Probe for Capabilities {
//! #     fn observe_event(&mut self, _: &Event) -> io::Result<()> { Ok(()) }
//! # }
//! # fn main() -> io::Result<()> {
//! # let input = std::io::stdin();
//! # let mut output = std::io::stdout();
//! let mut caps = Capabilities::default();
//! let mut events = uncurses::event::EventSource::new(input)?;
//!
//! output.write_all(&caps.queries()?)?;
//! output.flush()?;
//!
//! loop {
//!     caps.observe_event(&events.read()?)?;
//!     # break;
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Replies come back whenever the terminal feels like it, and a terminal may
//! answer nothing at all — so decide for yourself how long to wait, and treat
//! every field as optional.
//!
//! # With a screen
//!
//! [`ScreenOptions::extra_init_queries`](crate::screen::ScreenOptions::extra_init_queries)
//! sends the queries with the screen's own, and
//! [`Screen::query`](crate::screen::Screen::query) sends them later. Both
//! bound the batch and drain what is still in flight at teardown; read their
//! documentation for what may and may not go in one.
//!
//! ```no_run
//! # use std::io::{self, Write};
//! # use uncurses::event::Event;
//! # use uncurses::probe::Probe;
//! # use uncurses::screen::{Screen, ScreenOptions};
//! # #[derive(Default)] struct Capabilities;
//! # impl Probe for Capabilities {
//! #     fn observe_event(&mut self, _: &Event) -> io::Result<()> { Ok(()) }
//! # }
//! # fn main() -> io::Result<()> {
//! let mut caps = Capabilities::default();
//! let mut screen = Screen::stdio()?;
//!
//! screen.init_with(ScreenOptions {
//!     extra_init_queries: caps.queries()?,
//!     ..Default::default()
//! })?;
//!
//! loop {
//!     let event = screen.read_event()?;
//!     screen.observe_event(&event)?; // the screen's own capabilities
//!     caps.observe_event(&event)?; // yours
//!     # break;
//! }
//! # Ok(())
//! # }
//! ```

use std::collections::BTreeMap;
use std::io::{self, Write};

use crate::ansi::kitty::KittyKeyboardFlags;
use crate::ansi::mode::{Mode, ModeSetting};
use crate::ansi::{color, ctrl, kitty, mode, status, termcap, winop};
use crate::color::Color;
use crate::event::{ColorScheme as Scheme, Event};
use crate::layout::Size;

/// Something that asks the terminal questions and understands the answers.
///
/// Implement it on one type of your own that holds everything the program
/// wants to know, rather than one type per question: the fields are yours, so
/// reading an answer back is an ordinary field access.
///
/// Neither half is required. A probe that only listens — for an unsolicited
/// [`ColorScheme`](Event::ColorScheme) change, say — leaves
/// [`write_queries`](Self::write_queries) defaulted.
pub trait Probe {
    /// Write the queries this probe needs answered.
    ///
    /// Compose them with the writers in [`ansi`](crate::ansi). Do not append a
    /// Primary DA request: the batch is terminated for you, and a second
    /// terminator would end it early.
    ///
    /// # Errors
    ///
    /// Returns any error from writing to `out`.
    fn write_queries(&self, _out: &mut dyn Write) -> io::Result<()> {
        Ok(())
    }

    /// Fold an event, recording anything it answers.
    ///
    /// Called with *every* event, not just replies — match the ones you asked
    /// for and ignore the rest. Match on content (mode number, OSC code,
    /// palette index), never on arrival position: a terminal answers what it
    /// chooses to, in the order it chooses.
    ///
    /// # Errors
    ///
    /// Returns an error only if folding needs to do fallible work; simple
    /// recording cannot fail.
    fn observe_event(&mut self, event: &Event) -> io::Result<()>;

    /// Collect [`write_queries`](Self::write_queries) into a buffer.
    ///
    /// # Errors
    ///
    /// Returns any error from [`write_queries`](Self::write_queries).
    fn queries(&self) -> io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.write_queries(&mut buf)?;
        Ok(buf)
    }
}

/// OSC 10: the terminal's default foreground color.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ForegroundColor(pub Option<Color>);

impl Probe for ForegroundColor {
    fn write_queries(&self, out: &mut dyn Write) -> io::Result<()> {
        out.write_all(color::REQUEST_FOREGROUND_COLOR)
    }

    fn observe_event(&mut self, event: &Event) -> io::Result<()> {
        if let Event::ForegroundColor(c) = *event {
            self.0 = Some(c);
        }
        Ok(())
    }
}

/// OSC 11: the terminal's default background color.
///
/// More precise than [`ColorScheme`], which only says light or dark.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BackgroundColor(pub Option<Color>);

impl Probe for BackgroundColor {
    fn write_queries(&self, out: &mut dyn Write) -> io::Result<()> {
        out.write_all(color::REQUEST_BACKGROUND_COLOR)
    }

    fn observe_event(&mut self, event: &Event) -> io::Result<()> {
        if let Event::BackgroundColor(c) = *event {
            self.0 = Some(c);
        }
        Ok(())
    }
}

/// OSC 12: the terminal's cursor color.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CursorColor(pub Option<Color>);

impl Probe for CursorColor {
    fn write_queries(&self, out: &mut dyn Write) -> io::Result<()> {
        out.write_all(color::REQUEST_CURSOR_COLOR)
    }

    fn observe_event(&mut self, event: &Event) -> io::Result<()> {
        if let Event::CursorColor(c) = *event {
            self.0 = Some(c);
        }
        Ok(())
    }
}

/// OSC 4: indexed palette entries.
///
/// Ask for the entries you need — the first 16 are the ANSI colors — rather
/// than all 256, since every index costs a query and a reply.
///
/// Only the indices it asked about are recorded. Replies are broadcast, and a
/// screen or another probe on the same terminal is asking its own questions —
/// absorbing those answers would make the contents depend on who else is on
/// the wire.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PaletteColor {
    wanted: Vec<u8>,
    entries: BTreeMap<u8, Color>,
}

impl PaletteColor {
    /// Probe the given palette indices.
    #[must_use]
    pub fn new(indices: impl IntoIterator<Item = u8>) -> Self {
        let mut wanted: Vec<u8> = indices.into_iter().collect();
        wanted.sort_unstable();
        wanted.dedup();
        Self {
            wanted,
            entries: BTreeMap::new(),
        }
    }

    /// The 16 ANSI palette entries.
    #[must_use]
    pub fn ansi() -> Self {
        Self::new(0..16)
    }

    /// The color reported for `index`, if the terminal answered.
    #[must_use]
    pub fn get(&self, index: u8) -> Option<Color> {
        self.entries.get(&index).copied()
    }

    /// Every entry reported so far, in index order.
    pub fn entries(&self) -> impl Iterator<Item = (u8, Color)> + '_ {
        self.entries.iter().map(|(i, c)| (*i, *c))
    }

    /// Whether every requested index has been answered.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.wanted.iter().all(|i| self.entries.contains_key(i))
    }
}

impl Probe for PaletteColor {
    fn write_queries(&self, mut out: &mut dyn Write) -> io::Result<()> {
        for &index in &self.wanted {
            color::write_request_palette_color(&mut out, index)?;
        }
        Ok(())
    }

    fn observe_event(&mut self, event: &Event) -> io::Result<()> {
        if let Event::PaletteColor { index, color } = *event
            && self.wanted.contains(&index)
        {
            self.entries.insert(index, color);
        }
        Ok(())
    }
}

/// DECRQM (`CSI ? Ps $ p`): whether the terminal supports given modes.
///
/// Keeps the terminal's answer rather than a `bool`, so "reported
/// unsupported" and "never answered" stay distinguishable.
///
/// Only the modes it asked about are recorded. Replies are broadcast, and a
/// screen or another probe on the same terminal is asking its own questions —
/// absorbing those answers would make the contents depend on who else is on
/// the wire.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ModeReport {
    wanted: Vec<Mode>,
    reports: Vec<(Mode, ModeSetting)>,
}

impl ModeReport {
    /// Probe the given modes.
    #[must_use]
    pub fn new(modes: impl IntoIterator<Item = Mode>) -> Self {
        Self {
            wanted: modes.into_iter().collect(),
            reports: Vec::new(),
        }
    }

    /// The terminal's answer for `mode`, or `None` if it never answered.
    #[must_use]
    pub fn get(&self, mode: Mode) -> Option<ModeSetting> {
        self.reports
            .iter()
            .find(|(m, _)| *m == mode)
            .map(|(_, s)| *s)
    }

    /// Whether the terminal reported `mode` as available.
    #[must_use]
    pub fn is_available(&self, mode: Mode) -> bool {
        self.get(mode).is_some_and(ModeSetting::is_available)
    }

    /// Every answer received so far, in arrival order.
    pub fn reports(&self) -> impl Iterator<Item = (Mode, ModeSetting)> + '_ {
        self.reports.iter().copied()
    }
}

impl Probe for ModeReport {
    fn write_queries(&self, mut out: &mut dyn Write) -> io::Result<()> {
        for &mode in &self.wanted {
            mode::write_request_mode(&mut out, mode)?;
        }
        Ok(())
    }

    fn observe_event(&mut self, event: &Event) -> io::Result<()> {
        if let Event::ModeReport { mode, setting } = *event
            && self.wanted.contains(&mode)
        {
            match self.reports.iter_mut().find(|(m, _)| *m == mode) {
                Some(slot) => slot.1 = setting,
                None => self.reports.push((mode, setting)),
            }
        }
        Ok(())
    }
}

/// DA1 (`CSI c`): the terminal's primary device attributes.
///
/// Listens only. Every batch is terminated by a Primary DA request, so the
/// answer arrives without asking — and asking again would put a second
/// terminator on the wire and end the batch early.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PrimaryDeviceAttributes(pub Vec<Option<u32>>);

impl PrimaryDeviceAttributes {
    /// Whether attribute 4, sixel graphics, was advertised.
    #[must_use]
    pub fn supports_sixel(&self) -> bool {
        self.0.contains(&Some(4))
    }

    /// Whether attribute 52, clipboard access, was advertised. This says the
    /// terminal speaks OSC 52; whether it will *answer* a read is a separate
    /// policy question, and many terminals refuse or prompt.
    #[must_use]
    pub fn supports_clipboard(&self) -> bool {
        self.0.contains(&Some(52))
    }
}

impl Probe for PrimaryDeviceAttributes {
    fn observe_event(&mut self, event: &Event) -> io::Result<()> {
        if let Event::PrimaryDeviceAttributes(attrs) = event {
            self.0.clone_from(attrs);
        }
        Ok(())
    }
}

/// XTVERSION (`CSI > q`): the terminal's self-reported name and version.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TerminalName(pub Option<String>);

impl Probe for TerminalName {
    fn write_queries(&self, out: &mut dyn Write) -> io::Result<()> {
        out.write_all(ctrl::REQUEST_XTVERSION)
    }

    fn observe_event(&mut self, event: &Event) -> io::Result<()> {
        if let Event::TerminalName(name) = event {
            self.0 = Some(name.clone());
        }
        Ok(())
    }
}

/// XTGETTCAP: terminfo capabilities by name.
///
/// Each name is asked for in its own request: some terminals answer only the
/// first capability of a batched one, and a batched *reply* cannot be taken
/// apart again — values are hex-decoded before they reach here and terminfo
/// strings contain the same `;` that separates entries.
///
/// Only the names it asked about are recorded. Replies are broadcast, and a
/// screen on the same terminal asks for capabilities of its own.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Termcap {
    wanted: Vec<String>,
    entries: BTreeMap<String, String>,
    unsupported: Vec<String>,
}

impl Termcap {
    /// Probe the given terminfo capability names, e.g. `["RGB", "TN", "Su"]`.
    #[must_use]
    pub fn new<S: Into<String>>(names: impl IntoIterator<Item = S>) -> Self {
        Self {
            wanted: names.into_iter().map(Into::into).collect(),
            entries: BTreeMap::new(),
            unsupported: Vec::new(),
        }
    }

    /// The value reported for `name`. An empty string means the capability is
    /// present as a boolean flag with no value.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries.get(name).map(String::as_str)
    }

    /// Whether the terminal reported `name` as supported.
    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Names the terminal explicitly reported as unrecognized.
    pub fn unsupported(&self) -> impl Iterator<Item = &str> {
        self.unsupported.iter().map(String::as_str)
    }
}

impl Probe for Termcap {
    fn write_queries(&self, mut out: &mut dyn Write) -> io::Result<()> {
        for name in &self.wanted {
            termcap::write_xtgettcap(&mut out, &[name.as_str()])?;
        }
        Ok(())
    }

    fn observe_event(&mut self, event: &Event) -> io::Result<()> {
        let Event::Termcap {
            recognized,
            payload,
        } = event
        else {
            return Ok(());
        };
        // One name per request, so a reply carries one entry. Deliberately not
        // split on `;`: the payload is already hex-decoded, and terminfo string
        // values contain semicolons of their own — `setaf` is `48;5;%p1%d` —
        // so the entry boundaries a batched reply had are gone by now.
        let (name, value) = payload.split_once('=').unwrap_or((payload.as_str(), ""));
        if !self.wanted.iter().any(|w| w == name) {
            return Ok(());
        }
        if *recognized {
            self.unsupported.retain(|u| u != name);
            self.entries.insert(name.to_owned(), value.to_owned());
        } else {
            // A later reply replaces the earlier status rather than adding a
            // second one: reprobing after enabling a feature must not leave a
            // capability reported both supported and unsupported.
            self.entries.remove(name);
            if !self.unsupported.iter().any(|u| u == name) {
                self.unsupported.push(name.to_owned());
            }
        }
        Ok(())
    }
}

/// DEC mode 996 (`CSI ? 996 n`): the terminal's light or dark appearance.
///
/// Also folds unsolicited reports, so a theme change while the program runs
/// updates it — on terminals where such notifications have been enabled.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ColorScheme(pub Option<Scheme>);

impl Probe for ColorScheme {
    fn write_queries(&self, mut out: &mut dyn Write) -> io::Result<()> {
        status::write_request_light_dark_report(&mut out)
    }

    fn observe_event(&mut self, event: &Event) -> io::Result<()> {
        if let Event::ColorScheme(scheme) = *event {
            self.0 = Some(scheme);
        }
        Ok(())
    }
}

/// Kitty keyboard protocol support (`CSI ? u`).
///
/// Any reply means the protocol exists; the flags say what is enabled right
/// now, which may be nothing.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct KittyKeyboard(pub Option<KittyKeyboardFlags>);

impl KittyKeyboard {
    /// Whether the terminal answered at all, which is what indicates support.
    #[must_use]
    pub fn is_supported(&self) -> bool {
        self.0.is_some()
    }
}

impl Probe for KittyKeyboard {
    fn write_queries(&self, out: &mut dyn Write) -> io::Result<()> {
        out.write_all(kitty::REQUEST_KITTY_KEYBOARD)
    }

    fn observe_event(&mut self, event: &Event) -> io::Result<()> {
        if let Event::KittyKeyboardEnhancements(flags) = *event {
            self.0 = Some(flags);
        }
        Ok(())
    }
}

/// XTWINOPS 14 (`CSI 14 t`): the window size in pixels.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WindowPixelSize(pub Option<Size>);

impl Probe for WindowPixelSize {
    fn write_queries(&self, out: &mut dyn Write) -> io::Result<()> {
        out.write_all(winop::REQUEST_WINDOW_PIXEL_SIZE)
    }

    fn observe_event(&mut self, event: &Event) -> io::Result<()> {
        if let Event::WindowPixelSize { width, height } = *event {
            self.0 = Some(Size::new(width, height));
        }
        Ok(())
    }
}

/// XTWINOPS 16 (`CSI 16 t`): the character cell size in pixels.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CellPixelSize(pub Option<Size>);

impl Probe for CellPixelSize {
    fn write_queries(&self, out: &mut dyn Write) -> io::Result<()> {
        out.write_all(winop::REQUEST_CELL_PIXEL_SIZE)
    }

    fn observe_event(&mut self, event: &Event) -> io::Result<()> {
        if let Event::CellPixelSize { width, height } = *event {
            self.0 = Some(Size::new(width, height));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(p: &dyn Probe) -> String {
        String::from_utf8_lossy(&p.queries().unwrap()).into_owned()
    }

    #[test]
    fn each_color_probe_asks_for_exactly_its_own_osc() {
        assert_eq!(q(&ForegroundColor::default()), "\x1b]10;?\x07");
        assert_eq!(q(&BackgroundColor::default()), "\x1b]11;?\x07");
        assert_eq!(q(&CursorColor::default()), "\x1b]12;?\x07");

        // And each folds only its own reply.
        let mut fg = ForegroundColor::default();
        fg.observe_event(&Event::BackgroundColor(Color::Red))
            .unwrap();
        assert_eq!(fg.0, None, "a background reply is not a foreground one");
        fg.observe_event(&Event::ForegroundColor(Color::Blue))
            .unwrap();
        assert_eq!(fg.0, Some(Color::Blue));
    }

    #[test]
    fn palette_color_asks_only_for_the_indices_it_wants() {
        let mut p = PaletteColor::new([1, 1, 0]);
        assert_eq!(q(&p), "\x1b]4;0;?\x07\x1b]4;1;?\x07", "sorted and deduped");
        assert!(!p.is_complete());

        p.observe_event(&Event::PaletteColor {
            index: 0,
            color: Color::Red,
        })
        .unwrap();
        assert_eq!(p.get(0), Some(Color::Red));
        assert!(!p.is_complete(), "index 1 is still outstanding");

        p.observe_event(&Event::PaletteColor {
            index: 1,
            color: Color::Blue,
        })
        .unwrap();
        assert!(p.is_complete());
        assert_eq!(p.entries().count(), 2);
        assert_eq!(q(&PaletteColor::ansi()).matches("\x1b]4;").count(), 16);

        // Something else on this terminal asked about index 9.
        p.observe_event(&Event::PaletteColor {
            index: 9,
            color: Color::Green,
        })
        .unwrap();
        assert_eq!(p.get(9), None, "records only the indices it asked about");
        assert_eq!(p.entries().count(), 2);
    }

    #[test]
    fn mode_report_keeps_the_answer_not_just_a_flag() {
        let mut p = ModeReport::new([Mode::SYNCHRONIZED_OUTPUT, Mode::UNICODE_CORE]);
        assert_eq!(q(&p), "\x1b[?2026$p\x1b[?2027$p");

        p.observe_event(&Event::ModeReport {
            mode: Mode::SYNCHRONIZED_OUTPUT,
            setting: ModeSetting::Set,
        })
        .unwrap();
        p.observe_event(&Event::ModeReport {
            mode: Mode::UNICODE_CORE,
            setting: ModeSetting::NotRecognized,
        })
        .unwrap();

        assert!(p.is_available(Mode::SYNCHRONIZED_OUTPUT));
        assert!(!p.is_available(Mode::UNICODE_CORE));
        // The distinction a bool cannot make: answered-unsupported vs unasked.
        assert_eq!(p.get(Mode::UNICODE_CORE), Some(ModeSetting::NotRecognized));
        assert_eq!(p.get(Mode::IN_BAND_RESIZE), None);
    }

    #[test]
    fn mode_report_replaces_rather_than_duplicates_a_repeated_answer() {
        let mut p = ModeReport::new([Mode::SYNCHRONIZED_OUTPUT]);
        for setting in [ModeSetting::NotRecognized, ModeSetting::Set] {
            p.observe_event(&Event::ModeReport {
                mode: Mode::SYNCHRONIZED_OUTPUT,
                setting,
            })
            .unwrap();
        }
        assert_eq!(p.reports().count(), 1);
        assert!(p.is_available(Mode::SYNCHRONIZED_OUTPUT));
    }

    #[test]
    fn primary_device_attributes_asks_and_folds() {
        let mut p = PrimaryDeviceAttributes::default();
        // Listens only: the batch terminator is a DA1 request already, and a
        // second one would end the batch early.
        assert_eq!(q(&p), "");

        p.observe_event(&Event::PrimaryDeviceAttributes(vec![
            Some(65),
            Some(4),
            Some(52),
        ]))
        .unwrap();
        assert!(p.supports_sixel());
        assert!(p.supports_clipboard());
    }

    /// Replies are broadcast, so a probe shares the stream with the screen's
    /// own capability queries. It must keep to what it asked about.
    #[test]
    fn mode_report_records_only_what_it_asked_about() {
        let mut p = ModeReport::new([Mode::LEFT_RIGHT_MARGIN, Mode::FOCUS]);

        for ev in [
            Event::ModeReport {
                mode: Mode::LEFT_RIGHT_MARGIN,
                setting: ModeSetting::Set,
            },
            Event::ModeReport {
                mode: Mode::FOCUS,
                setting: ModeSetting::Reset,
            },
            // Something else on this terminal asked about this one.
            Event::ModeReport {
                mode: Mode::SYNCHRONIZED_OUTPUT,
                setting: ModeSetting::Set,
            },
        ] {
            p.observe_event(&ev).unwrap();
        }

        assert!(p.is_available(Mode::LEFT_RIGHT_MARGIN));
        assert_eq!(p.get(Mode::FOCUS), Some(ModeSetting::Reset));
        assert_eq!(
            p.get(Mode::SYNCHRONIZED_OUTPUT),
            None,
            "not its question, even though the answer went past"
        );
        assert_eq!(p.reports().count(), 2);
    }

    #[test]
    fn termcap_asks_one_key_per_request() {
        let asked = q(&Termcap::new(["RGB", "TN"]));
        assert_eq!(
            asked.matches("\x1bP+q").count(),
            2,
            "batched keys can go unanswered: {asked:?}"
        );
    }

    /// Terminfo string values contain semicolons of their own, and the payload
    /// reaching us is already decoded — so the value must be taken verbatim
    /// after the first `=`, not re-split.
    #[test]
    fn termcap_keeps_values_containing_semicolons() {
        let mut p = Termcap::new(["setaf", "RGB", "Su"]);
        p.observe_event(&Event::Termcap {
            recognized: true,
            payload: "setaf=48;5;%p1%d".into(),
        })
        .unwrap();
        p.observe_event(&Event::Termcap {
            recognized: true,
            payload: "RGB".into(),
        })
        .unwrap();
        p.observe_event(&Event::Termcap {
            recognized: false,
            payload: "Su".into(),
        })
        .unwrap();

        assert_eq!(p.get("setaf"), Some("48;5;%p1%d"));
        assert_eq!(p.get("RGB"), Some(""), "a flag has no value");
        assert!(!p.has("5"), "value fragments are not capability names");
        assert!(!p.has("Su"));
        assert_eq!(p.unsupported().collect::<Vec<_>>(), ["Su"]);
    }

    /// The screen puts its own XTGETTCAP requests on the same wire.
    #[test]
    fn termcap_records_only_what_it_asked_about() {
        let mut p = Termcap::new(["TN"]);
        for payload in ["RGB", "TN=ghostty"] {
            p.observe_event(&Event::Termcap {
                recognized: true,
                payload: payload.into(),
            })
            .unwrap();
        }
        assert_eq!(p.get("TN"), Some("ghostty"));
        assert!(!p.has("RGB"), "not its question");
    }

    #[test]
    fn termcap_does_not_repeat_an_unsupported_name() {
        let mut p = Termcap::new(["Su"]);
        for _ in 0..3 {
            p.observe_event(&Event::Termcap {
                recognized: false,
                payload: "Su".into(),
            })
            .unwrap();
        }
        assert_eq!(p.unsupported().count(), 1);
    }

    /// Reprobing after enabling a feature can flip a capability's status. The
    /// later reply replaces the earlier one rather than adding a second, so a
    /// name is never reported supported and unsupported at once.
    #[test]
    fn termcap_replaces_a_status_rather_than_accumulating() {
        let mut p = Termcap::new(["Su"]);

        p.observe_event(&Event::Termcap {
            recognized: false,
            payload: "Su".into(),
        })
        .unwrap();
        assert!(!p.has("Su"));
        assert_eq!(p.unsupported().collect::<Vec<_>>(), ["Su"]);

        p.observe_event(&Event::Termcap {
            recognized: true,
            payload: "Su=1".into(),
        })
        .unwrap();
        assert_eq!(p.get("Su"), Some("1"));
        assert_eq!(p.unsupported().count(), 0, "no longer unsupported");

        p.observe_event(&Event::Termcap {
            recognized: false,
            payload: "Su".into(),
        })
        .unwrap();
        assert!(!p.has("Su"), "the stale value is dropped");
        assert_eq!(p.unsupported().collect::<Vec<_>>(), ["Su"]);
    }

    #[test]
    fn name_scheme_and_kitty_ask_and_fold() {
        let mut name = TerminalName::default();
        assert_eq!(q(&name), "\x1b[>q");
        name.observe_event(&Event::TerminalName("ghostty 1.0".into()))
            .unwrap();
        assert_eq!(name.0.as_deref(), Some("ghostty 1.0"));

        let mut scheme = ColorScheme::default();
        assert_eq!(q(&scheme), "\x1b[?996n");
        scheme
            .observe_event(&Event::ColorScheme(Scheme::Dark))
            .unwrap();
        assert_eq!(scheme.0, Some(Scheme::Dark));

        let mut kitty = KittyKeyboard::default();
        assert_eq!(q(&kitty), "\x1b[?u");
        assert!(!kitty.is_supported());
        kitty
            .observe_event(&Event::KittyKeyboardEnhancements(
                KittyKeyboardFlags::empty(),
            ))
            .unwrap();
        assert!(
            kitty.is_supported(),
            "an empty-flags reply still means the protocol exists"
        );
    }

    /// `CSI 14 t` and `CSI 16 t` answer with different reports, so the probes
    /// must not fold each other's.
    #[test]
    fn window_and_cell_pixel_sizes_are_distinct() {
        let mut window = WindowPixelSize::default();
        let mut cell = CellPixelSize::default();
        assert_eq!(q(&window), "\x1b[14t");
        assert_eq!(q(&cell), "\x1b[16t");

        let reports = [
            Event::WindowPixelSize {
                width: 800,
                height: 600,
            },
            Event::CellPixelSize {
                width: 8,
                height: 16,
            },
        ];
        for ev in &reports {
            window.observe_event(ev).unwrap();
            cell.observe_event(ev).unwrap();
        }
        assert_eq!(window.0, Some(Size::new(800, 600)));
        assert_eq!(cell.0, Some(Size::new(8, 16)));
    }

    /// The pattern the module docs recommend: hold the shipped probes as
    /// fields and forward to them, rather than matching events by hand.
    #[test]
    fn probes_compose_into_an_application_capability_type() {
        struct Caps {
            background: BackgroundColor,
            modes: ModeReport,
        }

        impl Probe for Caps {
            fn write_queries(&self, out: &mut dyn Write) -> io::Result<()> {
                self.background.write_queries(out)?;
                self.modes.write_queries(out)
            }

            fn observe_event(&mut self, event: &Event) -> io::Result<()> {
                self.background.observe_event(event)?;
                self.modes.observe_event(event)
            }
        }

        let mut caps = Caps {
            background: BackgroundColor::default(),
            modes: ModeReport::new([Mode::LEFT_RIGHT_MARGIN, Mode::MOUSE_SGR_PIXEL]),
        };

        // Asking is the concatenation of the parts, in field order.
        assert_eq!(q(&caps), "\x1b]11;?\x07\x1b[?69$p\x1b[?1016$p");

        for ev in [
            Event::BackgroundColor(Color::Red),
            Event::ModeReport {
                mode: Mode::LEFT_RIGHT_MARGIN,
                setting: ModeSetting::Set,
            },
            Event::ModeReport {
                mode: Mode::MOUSE_SGR_PIXEL,
                setting: ModeSetting::Set,
            },
        ] {
            caps.observe_event(&ev).unwrap();
        }

        assert_eq!(caps.background.0, Some(Color::Red));
        assert!(caps.modes.is_available(Mode::LEFT_RIGHT_MARGIN));
        assert!(caps.modes.is_available(Mode::MOUSE_SGR_PIXEL));
    }

    /// Probes ignore what they did not ask about, so several fold the same
    /// broadcast stream without interfering.
    #[test]
    fn probes_ignore_events_that_are_not_theirs() {
        let mut bg = BackgroundColor::default();
        let mut modes = ModeReport::new([Mode::SYNCHRONIZED_OUTPUT]);
        for ev in [
            Event::BackgroundColor(Color::Red),
            Event::ModeReport {
                mode: Mode::SYNCHRONIZED_OUTPUT,
                setting: ModeSetting::Set,
            },
            Event::TerminalName("irrelevant".into()),
        ] {
            bg.observe_event(&ev).unwrap();
            modes.observe_event(&ev).unwrap();
        }
        assert_eq!(bg.0, Some(Color::Red));
        assert!(modes.is_available(Mode::SYNCHRONIZED_OUTPUT));
    }
}
