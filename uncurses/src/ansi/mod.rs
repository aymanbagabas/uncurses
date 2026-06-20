//! ANSI escape sequence helpers.
//!
//! This module groups small encoders, parsers, and utilities for terminal control sequences.
//! It covers cursor movement, modes, colors, titles, hyperlinks, clipboard data, wrapping, and related text handling.
//! Reach for it when you need to emit or inspect raw control sequences directly instead of going through the higher level rendering APIs.

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

pub use cursor::*;
pub use hyperlink::*;
pub use kitty::{
    KittyKeyboardFlags, KittyKeyboardMode, write_pop_kitty_keyboard, write_push_kitty_keyboard,
    write_request_kitty_keyboard, write_set_kitty_keyboard,
};
pub use mode::*;
pub use screen::*;
pub use sgr::*;
pub use strip::strip as strip_ansi;
pub use text::{Token, WidthMode, string_width, tokenize};
pub use title::*;
pub use truncate::{cut, cut_mode, truncate, truncate_left, truncate_left_mode, truncate_mode};
pub use wrap::{
    DEFAULT_BREAKPOINTS, hardwrap, hardwrap_mode, wordwrap, wordwrap_mode, wrap, wrap_mode,
};
