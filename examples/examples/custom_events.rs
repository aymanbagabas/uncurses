//! One event bus, many producers.
//!
//! `Screen` is synchronous and owns the terminal (input + rendering), so it
//! stays on the main thread. Everything else that wants to talk to the UI,
//! a clock, a background job, a network task, pushes into a single
//! `std::sync::mpsc` channel. The app defines one `AppEvent` enum that wraps
//! uncurses terminal events alongside its own message kinds, so the draw loop
//! reacts to keystrokes and worker updates through the same match.
//!
//! The main loop merges the two sources by hand: it polls the terminal for a
//! short interval (so a keystroke is picked up promptly), then drains whatever
//! the worker threads have queued, then repaints once. This is the idiomatic
//! shape for a sync `Screen`: the channel decouples the producers, and the
//! terminal poll timeout is what keeps the loop responsive to both.
//!
//! Run with `cargo run --example custom_events`. Type to log keys; watch the
//! clock and the background job tick on their own threads. Press `q` or
//! `Ctrl-C` to quit.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use uncurses::buffer::SurfaceMut;
use uncurses::color::Color;
use uncurses::event::{Event, Key, KeyCode};
use uncurses::screen::{Screen, ScreenOptions};
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

/// The application's event type. It encompasses uncurses terminal events
/// (`Term`) and the app's own cross-thread messages.
enum AppEvent {
    /// A decoded terminal event: key, resize, mouse, paste, etc.
    Term(Event),
    /// The clock thread ticked; carries seconds since start.
    Tick(u64),
    /// A background job reported progress (0..=100).
    Job(u8),
}

/// How long the main loop waits on terminal input before checking the channel
/// and repainting. Small enough to feel instant, large enough to idle cheaply.
// ponytail: fixed poll interval; make it adaptive only if idle wakeups matter.
const POLL: Duration = Duration::from_millis(50);

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init_with(ScreenOptions::default())?;
    screen.enter_alt_screen()?;
    screen.hide_cursor()?;

    let result = run(&mut screen);
    screen.finish()?;
    result
}

fn run(screen: &mut Screen<Stdin, Stdout>) -> std::io::Result<()> {
    let (tx, rx): (Sender<AppEvent>, Receiver<AppEvent>) = mpsc::channel();

    // Producer 1: a 1 Hz clock.
    spawn_ticker(tx.clone());
    // Producer 2: a background job that counts to 100 and stops.
    spawn_job(tx.clone());
    // Drop the original sender so the channel would close if every worker
    // exited; the main loop reads terminal input directly, not through `rx`.
    drop(tx);

    let quit: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
    let mut app = App::default();
    app.draw(screen);

    loop {
        // Source A: the terminal. Block up to POLL for input, then drain
        // everything decoded so far as `AppEvent::Term`.
        if screen.poll_event(Some(POLL))? {
            while let Some(ev) = screen.try_read_event() {
                screen.observe_event(&ev)?;
                if let Event::KeyPress(ref k) = ev
                    && quit.contains(k)
                {
                    return Ok(());
                }
                app.on(AppEvent::Term(ev), screen);
            }
        }

        // Source B: the worker threads. Drain without blocking.
        while let Ok(msg) = rx.try_recv() {
            app.on(msg, screen);
        }

        app.draw(screen);
    }
}

#[derive(Default)]
struct App {
    typed: String,
    seconds: u64,
    progress: u8,
    styles: Styles,
}

struct Styles {
    dim: Style,
    key: Style,
    val: Style,
}

impl Default for Styles {
    fn default() -> Self {
        Self {
            dim: Style::default().fg(Color::BrightBlack),
            key: Style::default().fg(Color::BrightGreen),
            val: Style::default().fg(Color::BrightCyan),
        }
    }
}

impl App {
    /// Fold one event into the app state. Terminal and worker events land
    /// here through the same door.
    fn on(&mut self, ev: AppEvent, screen: &mut Screen<Stdin, Stdout>) {
        match ev {
            AppEvent::Term(Event::KeyPress(Key {
                code: KeyCode::Char(c),
                ..
            })) => self.typed.push(c),
            AppEvent::Term(Event::KeyPress(Key {
                code: KeyCode::Backspace,
                ..
            })) => {
                self.typed.pop();
            }
            AppEvent::Term(Event::Resize(ws)) => screen.resize((ws.col, ws.row)),
            AppEvent::Term(_) => {}
            AppEvent::Tick(s) => self.seconds = s,
            AppEvent::Job(p) => self.progress = p,
        }
    }

    fn draw(&self, screen: &mut Screen<Stdin, Stdout>) {
        screen.clear();
        let Styles { dim, key, val } = &self.styles;

        screen.set_str((0, 0), "One event bus, three producers. q quits.", dim);
        screen.set_str((0, 2), "typed:  ", key);
        screen.set_str((8, 2), &self.typed, val);
        screen.set_str((0, 3), "clock:  ", key);
        screen.set_str((8, 3), &format!("{}s", self.seconds), val);
        screen.set_str((0, 4), "job:    ", key);
        screen.set_str((8, 4), &format!("{}%", self.progress), val);

        let _ = screen.render();
    }
}

fn spawn_ticker(tx: Sender<AppEvent>) {
    thread::spawn(move || {
        let mut secs = 0u64;
        loop {
            thread::sleep(Duration::from_secs(1));
            secs += 1;
            // A closed channel means the UI is gone; stop the thread.
            if tx.send(AppEvent::Tick(secs)).is_err() {
                break;
            }
        }
    });
}

fn spawn_job(tx: Sender<AppEvent>) {
    thread::spawn(move || {
        for p in 0..=100u8 {
            thread::sleep(Duration::from_millis(80));
            if tx.send(AppEvent::Job(p)).is_err() {
                break;
            }
        }
    });
}
