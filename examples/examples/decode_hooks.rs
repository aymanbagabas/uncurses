//! Decoder hook playground.
//!
//! Run with `cargo run --example decode_hooks`. Demonstrates the
//! per-class hook API on [`EventSource`]:
//!
//! * **Override a recognised sequence.** A CSI hook claims cursor
//!   position reports (`CSI <r>;<c> R`) before the builtin recogniser
//!   sees them, surfacing a custom string event with the parsed
//!   coordinates instead of the usual `Event::CursorPosition`.
//! * **Surface an unknown sequence.** An OSC hook claims an arbitrary
//!   non-standard payload (here, an `OSC 777;...` notification) that
//!   the builtin recogniser doesn't know about, so it never becomes
//!   `Event::UnknownOsc`.
//! * **Inspect and pass through.** A second CSI hook logs every
//!   incoming CSI for sequences it doesn't claim by returning `None`,
//!   letting the builtin recogniser run normally. (User hooks run in
//!   registration order; the first `Some(_)` wins.)
//!
//! Key bindings while running:
//! * `p` — send DSR cursor-position query (`CSI 6 n`). Terminal
//!   replies with `CSI <r>;<c> R`; the override hook claims it.
//! * `d` — send Primary DA query (`CSI c`). Terminal replies and the
//!   builtin recogniser emits `Event::PrimaryDeviceAttributes`,
//!   demonstrating that hooks returning `None` don't block defaults.
//! * `b` — send OSC 11 background-color query. Terminal replies and
//!   the builtin recogniser emits `Event::BackgroundColor`.
//! * `n` — write a synthetic `OSC 777;hello\\x1b\\\\` echo to the
//!   terminal. Most terminals just ignore it, but if yours echoes the
//!   bytes back the OSC hook will fire.
//! * `q` / Ctrl-C — quit.

use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;

use uncurses::SurfaceMut;
use uncurses::Terminal;
use uncurses::event::{Event, EventSource, Key, KeyCode, KeyModifiers};
use uncurses::screen::Screen;
use uncurses::terminal::{TtyInput, TtyOutput};
use uncurses::text::WrapMode;

fn truncate(s: &str, width: u16) -> String {
    let max = width as usize;
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

fn redraw<W: std::io::Write>(screen: &mut Screen<W>, last: &str) {
    screen.clear();
    let w = screen.width();
    let header = "decode_hooks — p=CPR  d=DA1  b=BG  n=OSC777  q/Ctrl-C=quit";
    screen.set_str((0, 0), &truncate(header, w), WrapMode::Truncate);
    let line = format!("last: {}", last);
    screen.set_str((0, 1), &truncate(&line, w), WrapMode::Truncate);
}

fn fmt_event(ev: &Event) -> String {
    match ev {
        Event::KeyPress(k) => format!("Key {:?}", k.code),
        Event::Resize(ws) => format!("Resize {}x{}", ws.col, ws.row),
        Event::CursorPosition(p) => format!("CursorPosition (builtin) @ {:?}", p),
        Event::PrimaryDeviceAttributes(p) => format!("PrimaryDeviceAttributes {:?}", p),
        Event::BackgroundColor(c) => format!("BackgroundColor {:?}", c),
        Event::Unknown(b) => format!("Unknown {:?}", String::from_utf8_lossy(b)),
        Event::UnknownCsi(b) => format!("UnknownCsi {:?}", String::from_utf8_lossy(b)),
        Event::UnknownOsc(b) => format!("UnknownOsc {:?}", String::from_utf8_lossy(b)),
        other => format!("{:?}", other),
    }
}

struct App {
    term: Terminal<TtyInput, TtyOutput>,
    screen: Screen<TtyOutput>,
    events: EventSource<TtyInput>,
    hook_log: Arc<Mutex<Vec<String>>>,
    last: String,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut term = Terminal::open()?;
        term.make_raw()?;
        let mut screen = Screen::new(
            term.output(),
            (term.window_size().unwrap_or_default().col, 2),
        );

        screen.set_cursor_visible(false)?;

        let mut events = EventSource::new(term.input())?;

        // Shared log channel for hook-side observations. Hooks run on the
        // decoder thread; the main loop drains the log after each `read()`.
        let hook_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        // 1. Override-style CSI hook: claim cursor-position reports and
        //    emit a custom `Event::Unknown` payload. Because user hooks
        //    run before the builtin recogniser, this pre-empts the usual
        //    `Event::CursorPosition`.
        let log_cpr = hook_log.clone();
        events.on_csi(move |view| {
            if view.final_byte == b'R' && view.private.is_none() {
                let params = view.params();
                let r = params.get(0).unwrap_or(0);
                let c = params.get(1).unwrap_or(0);
                let msg = format!("[hook] claim CPR row={} col={}", r, c);
                log_cpr.lock().unwrap().push(msg.clone());
                return Some(Event::Unknown(msg.into_bytes()));
            }
            None
        });

        // 2. Pass-through CSI hook: log every CSI we see and return None,
        //    letting the builtin recogniser run. The earlier hook still
        //    short-circuits CPR before this one is consulted only when it
        //    returns `Some(_)` — for everything else, both hooks see the
        //    sequence in registration order.
        let log_csi = hook_log.clone();
        events.on_csi(move |view| {
            let summary = format!(
                "[hook] csi private={:?} params={:?} intermediates={:?} final={:?}",
                view.private.map(|b| b as char),
                view.params(),
                std::str::from_utf8(view.intermediates).unwrap_or("?"),
                view.final_byte as char,
            );
            log_csi.lock().unwrap().push(summary);
            None
        });

        // 3. OSC hook for a payload the library doesn't recognise. OSC 777
        //    is a de-facto extension used by some terminals for desktop
        //    notifications (`OSC 777;notify;title;body`). Returning `Some`
        //    surfaces it instead of `Event::UnknownOsc`.
        let log_osc = hook_log.clone();
        events.on_osc(move |view| {
            let mut parts = view.payload.splitn(2, |&b| b == b';');
            let code = parts.next().unwrap_or(&[]);
            if code == b"777" {
                let rest = parts.next().unwrap_or(&[]);
                let msg = format!(
                    "[hook] claim OSC 777 payload={:?}",
                    String::from_utf8_lossy(rest)
                );
                log_osc.lock().unwrap().push(msg.clone());
                return Some(Event::Unknown(msg.into_bytes()));
            }
            None
        });

        Ok(Self {
            term,
            screen,
            events,
            hook_log,
            last: "(waiting for input — press p / d / b / n)".to_string(),
        })
    }

    fn render(&mut self) {
        redraw(&mut self.screen, &self.last);
    }

    fn drain_log(&mut self) {
        let mut buf = self.hook_log.lock().unwrap();
        for line in buf.drain(..) {
            let _ = self.screen.insert_above(&line);
        }
    }

    fn run(&mut self) -> std::io::Result<()> {
        self.render();
        self.screen.render()?;
        self.screen.flush()?;

        while let Ok(ev) = self.events.read() {
            // Surface hook-side observations that happened during decode.
            self.drain_log();

            let mut last = fmt_event(&ev);
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
                Event::KeyPress(Key {
                    code: KeyCode::Char('p'),
                    modifiers,
                    ..
                }) if modifiers.is_empty() => {
                    (&self.term.output()).write_all(b"\x1b[6n")?;
                    (&self.term.output()).flush()?;
                    last = "sent: DSR cursor-position (CSI 6 n)".to_string();
                }
                Event::KeyPress(Key {
                    code: KeyCode::Char('d'),
                    modifiers,
                    ..
                }) if modifiers.is_empty() => {
                    (&self.term.output()).write_all(b"\x1b[c")?;
                    (&self.term.output()).flush()?;
                    last = "sent: Primary DA (CSI c)".to_string();
                }
                Event::KeyPress(Key {
                    code: KeyCode::Char('b'),
                    modifiers,
                    ..
                }) if modifiers.is_empty() => {
                    (&self.term.output()).write_all(b"\x1b]11;?\x1b\\")?;
                    (&self.term.output()).flush()?;
                    last = "sent: OSC 11 background-color query".to_string();
                }
                Event::KeyPress(Key {
                    code: KeyCode::Char('n'),
                    modifiers,
                    ..
                }) if modifiers.is_empty() => {
                    (&self.term.output()).write_all(b"\x1b]777;notify;decode_hooks;hello\x1b\\")?;
                    (&self.term.output()).flush()?;
                    last = "sent: OSC 777 notification (terminal likely ignores)".to_string();
                }
                Event::Resize(ws) => {
                    self.screen.resize(ws.col, 2);
                }
                _ => {}
            }

            self.screen.insert_above(&last)?;
            self.last = last;
            self.render();
            self.screen.render()?;
            self.screen.flush()?;
        }

        self.drain_log();
        Ok(())
    }

    fn stop(&mut self) -> std::io::Result<()> {
        self.screen.reset()?;
        self.screen.flush()?;
        self.term.restore()
    }
}

fn main() -> std::io::Result<()> {
    let mut app = App::start()?;
    let result = app.run();
    app.stop()?;
    result
}
