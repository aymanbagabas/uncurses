//! Window title sequences (OSC 0/1/2).

use std::io::{self, Write};

/// Set window title and icon name (OSC 0).
pub fn write_window_title_and_icon<W: Write>(w: &mut W, title: &str) -> io::Result<()> {
    write!(w, "\x1b]0;{title}\x1b\\")
}

/// Set icon name (OSC 1).
pub fn write_icon_name<W: Write>(w: &mut W, name: &str) -> io::Result<()> {
    write!(w, "\x1b]1;{name}\x1b\\")
}

/// Set window title (OSC 2).
pub fn write_window_title<W: Write>(w: &mut W, title: &str) -> io::Result<()> {
    write!(w, "\x1b]2;{title}\x1b\\")
}
