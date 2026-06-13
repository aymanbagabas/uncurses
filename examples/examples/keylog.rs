//! Comprehensive event logger.
//!
//! Run with `cargo run --example keylog`. Runs in *inline* mode (no alt
//! screen): a one-line status bar at the bottom of the screen tracks the
//! latest event, and every event is inserted above the screen via
//! [`Screen::insert_above`], so the full event history scrolls naturally
//! into the terminal's scrollback.
//!
//! Demonstrates the full breadth of events the source can produce: keys
//! (with modifiers, repeat, release), mouse move / clicks / wheel,
//! bracketed paste, focus changes, and window resizes.
//!
//! Press `q` or Ctrl-C to exit. On Unix, Ctrl-Z suspends the process and
//! it resumes cleanly with `fg`.

use std::io::Write;

use uncurses::SurfaceMut;
use uncurses::ansi::mode::{MouseEncoding, MouseMode};
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, Source};
use uncurses::screen::Screen;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, open_tty};
use uncurses::text::WrapMode;

fn format_modifiers(m: KeyModifiers) -> String {
    let mut parts = Vec::new();
    if m.contains(KeyModifiers::CTRL) {
        parts.push("Ctrl");
    }
    if m.contains(KeyModifiers::ALT) {
        parts.push("Alt");
    }
    if m.contains(KeyModifiers::SHIFT) {
        parts.push("Shift");
    }
    if m.contains(KeyModifiers::SUPER) {
        parts.push("Super");
    }
    if m.contains(KeyModifiers::HYPER) {
        parts.push("Hyper");
    }
    if m.contains(KeyModifiers::META) {
        parts.push("Meta");
    }
    if m.contains(KeyModifiers::CAPS_LOCK) {
        parts.push("CapsLock");
    }
    if m.contains(KeyModifiers::NUM_LOCK) {
        parts.push("NumLock");
    }
    parts.join("+")
}

fn format_event(ev: &Event) -> String {
    match ev {
        Event::KeyPress(k) | Event::KeyRepeat(k) => {
            let mods = format_modifiers(k.modifiers);
            let sep = if mods.is_empty() { "" } else { "+" };
            let kind = if matches!(ev, Event::KeyRepeat(_)) {
                "Repeat"
            } else {
                "Key   "
            };
            format!("{} {}{}{:?}  text={:?}", kind, mods, sep, k.code, k.text)
        }
        Event::KeyRelease(k) => format!("Up    {:?}", k.code),
        Event::MouseClick(m) => {
            format!("Click {:?} @ ({}, {})", m.button, m.x, m.y)
        }
        Event::MouseRelease(m) => {
            format!("MUp   {:?} @ ({}, {})", m.button, m.x, m.y)
        }
        Event::MouseWheel(m) => {
            format!("Wheel {:?} @ ({}, {})", m.button, m.x, m.y)
        }
        Event::MouseMove(m) => format!("Move  @ ({}, {})", m.x, m.y),
        Event::Resize(ws) => format!("Resize {}x{}", ws.col, ws.row),
        Event::FocusIn => "FocusIn".to_string(),
        Event::FocusOut => "FocusOut".to_string(),
        Event::PasteChunk(bytes) => format!(
            "PasteChunk ({} bytes): {:?}",
            bytes.len(),
            String::from_utf8_lossy(bytes)
        ),
        other => format!("Other {:?}", other),
    }
}

fn redraw<W: std::io::Write>(screen: &mut Screen<W>, last: &str) -> std::io::Result<()> {
    screen.clear();
    let w = screen.width();
    let header = "keylog — press q or Ctrl-C to quit. Type, click, drag, paste, resize.";
    {
        screen.set_str((0, 0), &truncate(header, w), WrapMode::Truncate);
    };
    let line = format!("last: {}", last);
    {
        screen.set_str((0, 1), &truncate(&line, w), WrapMode::Truncate);
    };
    Ok(())
}

fn truncate(s: &str, width: u16) -> String {
    let max = width as usize;
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

fn main() -> std::io::Result<()> {
    let (input, output) = open_tty()?;

    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut state = enable_raw_mode(input, output)?;

    let size = get_window_size(output).unwrap_or_default();
    // Inline status area is two rows tall; insert_above scrolls events
    // into the scrollback above it.
    let mut screen = Screen::new(output, (size.col, 2));

    screen.set_cursor_visible(false)?;
    screen.set_mouse_mode(MouseMode::Any, MouseEncoding::Sgr)?;
    screen.set_focus_events(true)?;
    screen.set_bracketed_paste(true)?;
    screen.set_title("📺 keylog — events 🎹🖱️")?;

    let mut events = Source::new(input)?;

    redraw(&mut screen, "(waiting for input)")?;
    screen.render()?;
    screen.flush()?;

    while let Ok(ev) = events.read() {
        match &ev {
            Event::KeyPress(Key {
                code: KeyCode::Char('q'),
                modifiers,
                ..
            }) if modifiers.is_empty() => {
                break;
            }
            Event::KeyPress(Key {
                code: KeyCode::Char('c'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CTRL) => {
                break;
            }
            #[cfg(unix)]
            Event::KeyPress(Key {
                code: KeyCode::Char('z'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CTRL) => {
                // Tear down the screen, drop raw mode, then send SIGTSTP to
                // ourselves. The kernel pauses the process until a SIGCONT
                // brings it back; control returns here on resume.
                screen.reset()?;
                screen.flush()?;
                disable_raw_mode(input, output, &state)?;

                // SAFETY: raise is async-signal-safe.
                unsafe { libc::raise(libc::SIGTSTP) };

                // Resumed: re-acquire raw mode, refit to the current window
                // size, and reinstate the screen modes we had before.
                state = enable_raw_mode(input, output)?;
                if let Ok(size) = get_window_size(output) {
                    screen.resize(size.col, 2);
                }
                screen.restore()?;
                screen.invalidate();
                redraw(&mut screen, "(resumed)")?;
                screen.render()?;
                screen.flush()?;
                continue;
            }
            Event::Resize(ws) => {
                screen.resize(ws.col, 2);
            }
            _ => {}
        }

        let line = format_event(&ev);
        screen.insert_above(&line)?;
        redraw(&mut screen, &line)?;
        screen.render()?;
        screen.flush()?;
    }

    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(input, output, &state)?;
    Ok(())
}
