//! Debug-only wire tracing for output (screen → terminal) and input
//! (terminal → parser) byte streams.
//!
//! Compiled in only when `debug_assertions` is enabled (i.e. non-release
//! builds). Each tee is activated by setting the corresponding
//! environment variable to a file path:
//!
//! - `UNCURSES_OUTPUT_TRACE` — receives one entry per output flush
//!   with the full staged buffer (text frames + control sequences +
//!   any raw `Write` payloads such as raster image OSCs), in flush
//!   order.
//! - `UNCURSES_INPUT_TRACE`  — receives one entry per `parse` call.
//!
//! Entries are appended; ESC and other control bytes are escaped so the
//! file is human-readable. In release builds the entire module is
//! compiled out and call sites become no-ops.
#![cfg(debug_assertions)]

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// Build a printable representation of `bytes` with ESC and other
/// control characters escaped.
///
/// Valid UTF-8 printable characters (accented letters, CJK, emoji, …)
/// are emitted as themselves so the trace stays readable; control
/// characters and bytes that are not valid UTF-8 are escaped (`\e`,
/// `\n`, …, or `\xHH`) so multibyte sequences are never split mid-glyph.
fn escape_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < 0x80 {
            push_ascii(&mut out, b);
            i += 1;
        } else if let Some((ch, len)) = decode_utf8(&bytes[i..]) {
            if ch.is_control() {
                // Multibyte control (e.g. a C1 control): keep byte-accurate.
                for &cb in &bytes[i..i + len] {
                    push_hex(&mut out, cb);
                }
            } else {
                out.push(ch);
            }
            i += len;
        } else {
            // Invalid UTF-8 lead/continuation byte.
            push_hex(&mut out, b);
            i += 1;
        }
    }
    out
}

/// Escape a single ASCII byte (`b < 0x80`).
fn push_ascii(out: &mut String, b: u8) {
    match b {
        0x1b => out.push_str("\\e"),
        b'\n' => out.push_str("\\n"),
        b'\r' => out.push_str("\\r"),
        b'\t' => out.push_str("\\t"),
        0x08 => out.push_str("\\b"),
        0x07 => out.push_str("\\a"),
        b'\\' => out.push_str("\\\\"),
        0x20..=0x7e => out.push(b as char),
        _ => push_hex(out, b),
    }
}

fn push_hex(out: &mut String, b: u8) {
    use std::fmt::Write as _;
    let _ = write!(out, "\\x{b:02x}");
}

/// Try to decode one UTF-8 scalar value at the start of `bytes`,
/// returning the char and its byte length. `None` if the bytes are not
/// a complete, valid UTF-8 sequence.
fn decode_utf8(bytes: &[u8]) -> Option<(char, usize)> {
    let len = match bytes[0] {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => return None,
    };
    let chunk = bytes.get(..len)?;
    let ch = std::str::from_utf8(chunk).ok()?.chars().next()?;
    Some((ch, len))
}

fn append_entry(path: &PathBuf, label: &str, index: u64, bytes: &[u8]) {
    let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let escaped = escape_bytes(bytes);
    let _ = writeln!(f, "--- {label} {index} ({} bytes) ---", bytes.len());
    let _ = writeln!(f, "{escaped}");
}

pub(crate) fn tee_output(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    static DEST: OnceLock<Option<PathBuf>> = OnceLock::new();
    static INDEX: AtomicU64 = AtomicU64::new(0);
    let path = DEST.get_or_init(|| std::env::var_os("UNCURSES_OUTPUT_TRACE").map(PathBuf::from));
    let Some(path) = path.as_ref() else { return };
    let n = INDEX.fetch_add(1, Ordering::Relaxed);
    append_entry(path, "flush", n, bytes);
}

pub(crate) fn tee_input(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    static DEST: OnceLock<Option<PathBuf>> = OnceLock::new();
    static INDEX: AtomicU64 = AtomicU64::new(0);
    let path = DEST.get_or_init(|| std::env::var_os("UNCURSES_INPUT_TRACE").map(PathBuf::from));
    let Some(path) = path.as_ref() else { return };
    let n = INDEX.fetch_add(1, Ordering::Relaxed);
    append_entry(path, "input", n, bytes);
}

#[cfg(test)]
mod tests {
    use super::escape_bytes;

    #[test]
    fn utf8_glyphs_round_trip() {
        // Accented latin, CJK, and an emoji all survive intact.
        assert_eq!(escape_bytes("café 中文 🎉".as_bytes()), "café 中文 🎉");
    }

    #[test]
    fn ascii_controls_are_escaped() {
        assert_eq!(escape_bytes(b"\x1b[m\n\t"), "\\e[m\\n\\t");
    }

    #[test]
    fn invalid_utf8_bytes_are_hex_escaped() {
        // A lone continuation byte and a truncated multibyte sequence.
        assert_eq!(escape_bytes(b"a\xffz"), "a\\xffz");
        // `0xc3` starts a 2-byte sequence but the slice ends — incomplete.
        assert_eq!(escape_bytes(b"\xc3"), "\\xc3");
    }

    #[test]
    fn split_glyph_bytes_are_not_mangled_when_complete() {
        // 'é' = 0xc3 0xa9 must render as the glyph, not \xc3\xa9.
        assert_eq!(escape_bytes(&[0xc3, 0xa9]), "é");
    }
}
