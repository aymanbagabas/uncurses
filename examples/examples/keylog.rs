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

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::event::{Event, Key, KeyCode, KeyModifiers};
use uncurses::screen::{MouseTracking, Screen, ScreenOptions};
use uncurses::terminal::{TtyInput, TtyOutput};
use uncurses::text::TextSurface;

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

fn redraw(screen: &mut Screen<TtyInput, TtyOutput>, last: &str) -> std::io::Result<()> {
    screen.clear();
    let w = screen.width();
    let header = "keylog — press q or Ctrl-C to quit. Type, click, drag, paste, resize.";
    screen.set_str(
        (0, 0),
        &truncate(header, w),
        uncurses::style::Style::default(),
    );
    let line = format!("last: {}", last);
    screen.set_str(
        (0, 1),
        &truncate(&line, w),
        uncurses::style::Style::default(),
    );
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

/// Event-logger app. Runs inline (no alt screen) on the controlling
/// tty. `start` enters raw mode and configures the inline status area,
/// `run` logs every event into the scrollback, and `stop` restores the
/// terminal. On Unix, Ctrl-Z suspends and resumes cleanly.
struct App {
    screen: Screen<TtyInput, TtyOutput>,
    last: String,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut screen = Screen::open()?;
        // Enable motion mouse tracking (for drag) and focus reporting so the
        // log shows the full breadth of events.
        screen.init_with(ScreenOptions {
            mouse: Some(MouseTracking::MOTION),
            ..ScreenOptions::default()
        })?;
        screen.enable_focus_events()?;
        screen.hide_cursor()?;
        // Inline status area is two rows tall; insert_above scrolls events
        // into the scrollback above it.
        let cols = screen.width();
        screen.resize((cols, 2));
        Ok(Self {
            screen,
            last: String::from("(waiting for input)"),
        })
    }

    fn render(&mut self) -> std::io::Result<()> {
        redraw(&mut self.screen, &self.last)?;
        self.screen.render()
    }

    fn run(&mut self) -> std::io::Result<()> {
        self.render()?;

        while let Ok(ev) = self.screen.read_event() {
            match &ev {
                Event::KeyPress(Key {
                    code: KeyCode::Char('q'),
                    modifiers,
                    ..
                }) if modifiers.is_empty() => break,
                Event::KeyPress(Key {
                    code: KeyCode::Char('c'),
                    modifiers,
                    ..
                }) if modifiers.contains(KeyModifiers::CTRL) => break,
                #[cfg(unix)]
                Event::KeyPress(Key {
                    code: KeyCode::Char('z'),
                    modifiers,
                    ..
                }) if modifiers.contains(KeyModifiers::CTRL) => {
                    self.screen.suspend()?;
                    self.screen.resume()?;
                    self.last = String::from("(resumed)");
                    self.render()?;
                    continue;
                }
                Event::Resize(ws) => {
                    self.screen.resize((ws.col, 2));
                }
                _ => {}
            }

            let line = format_event(&ev);
            self.screen.insert_above(&line)?;
            self.last = line;
            self.render()?;
        }
        Ok(())
    }

    fn stop(self) -> std::io::Result<()> {
        self.screen.finish()
    }
}

fn main() -> std::io::Result<()> {
    let mut app = App::start()?;
    let result = app.run();
    app.stop()?;
    result
}
