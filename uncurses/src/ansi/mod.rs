//! ANSI and terminal-control sequence subsystem.
//!
//! ## Scope
//!
//! The modules under `ansi` are the byte-level building blocks used to emit,
//! parse, measure, strip, truncate, and wrap terminal control streams. They cover
//! cursor motion, screen editing, modes, SGR styling, OSC metadata, DCS/APC
//! payloads, C0/C1 controls, and ANSI-aware text utilities.
//!
//! ## Sequence families
//!
//! Most writers emit 7-bit forms because they are broadly accepted on byte
//! streams that are otherwise UTF-8 text:
//!
//! ```text
//! CSI: ESC [ params intermediates final      e.g. ESC [ ? 2048 h
//! OSC: ESC ] command ; payload BEL|ST        e.g. ESC ] 2 ; title ESC \\
//! DCS: ESC P params payload ST               e.g. ESC P + q 524742 ESC \\
//! APC: ESC _ command payload ST              e.g. ESC _ G ... ESC \\
//! ```
//!
//! Anatomy of a DEC private mode sequence:
//!
//! ```text
//! ESC [  ?  2 0 4 8  h        CSI ? 2048 h  (enable mode 2048)
//! ──┬── ─┬─ ───┬──── ┬
//!  CSI  priv  params final
//! ```
//!
//! ## 7-bit and 8-bit controls
//!
//! The constants in [`c0`] and [`c1`] name single-byte controls. Parser utilities
//! recognize both the 7-bit `ESC` spellings and the 8-bit C1 bytes, while writer
//! functions generally choose explicit 7-bit byte strings.
//!
//! ## Mode interaction
//!
//! Mode-aware features are represented by [`mode::Mode`]. Enable or disable
//! modes with [`mode::write_set_mode`] and [`mode::write_reset_mode`] before
//! expecting mode-controlled reports such as bracketed paste, focus events,
//! in-band resize, or light/dark notifications.
//!
//! ## Example
//!
//! ```rust,ignore
//! use uncurses::ansi::title::write_window_title;
//!
//! let mut out = Vec::new();
//! write_window_title(&mut out, "my app")?; // ESC ] 2 ; my app ESC \\
//! # Ok::<(), std::io::Error>(())
//! ```

pub mod ascii;
pub mod background;
pub mod c0;
pub mod c1;
pub mod charset;
pub mod clipboard;
pub mod cost;
pub mod ctrl;
pub mod cursor;
pub mod cwd;
pub mod finalterm;
pub mod focus;
pub mod graphics;
pub mod hyperlink;
pub mod inband;
pub mod iterm2;
pub mod keypad;
pub mod kitty;
pub mod mode;
pub mod notification;
pub mod palette;
pub mod params;
pub mod passthrough;
pub mod paste;
pub mod progress;
pub mod screen;
pub mod sgr;
pub mod status;
pub mod strip;
pub mod termcap;
pub mod text;
pub mod title;
pub mod truncate;
pub mod urxvt;
pub mod winop;
pub mod wrap;
pub mod xterm;
