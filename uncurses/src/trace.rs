//! Debug-only wire tracing for output (screen → terminal) and input
//! (terminal → parser) byte streams.
//!
//! Compiled in only when `debug_assertions` is enabled (i.e. non-release
//! builds). Each tee is activated by setting the corresponding
//! environment variable to a file path:
//!
//! - `UNCURSES_OUTPUT_TRACE` — receives one entry per `Screen::flush`
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
fn escape_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        match b {
            0x1b => out.push_str("\\e"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x08 => out.push_str("\\b"),
            0x07 => out.push_str("\\a"),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(b as char),
            _ => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\x{b:02x}");
            }
        }
    }
    out
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
