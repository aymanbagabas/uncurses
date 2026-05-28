//! URxvt perl extension protocol (OSC 777).

use std::io::{self, Write};

/// Invoke a URxvt perl extension (`OSC 777 ; ext ; params... ST`).
pub fn write_urxvt_ext<W: Write>(w: &mut W, extension: &str, params: &[&str]) -> io::Result<()> {
    write!(w, "\x1b]777;{extension}")?;
    for p in params {
        write!(w, ";{p}")?;
    }
    w.write_all(b"\x07")
}
