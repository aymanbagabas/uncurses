//! Minimal interactive example, polling for events.
//!
//! Run with `cargo run --example interactive`. Press any key to see events;
//! press `q` or Ctrl-C to exit. Resize the terminal to see Resize events.
//!
//! Demonstrates the non-blocking [`EventSource::poll`] / [`EventSource::try_read`]
//! style: every 500 ms the main loop wakes up to advance a clock in the
//! header, even when no input has arrived.

use std::collections::VecDeque;
use std::io::Write;
use std::time::{Duration, Instant};

use uncurses::SurfaceMut;
use uncurses::Terminal;
use uncurses::event::{Event, EventSource, Key, KeyCode, KeyModifiers};
use uncurses::screen::Screen;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::WrapMode;

const TICK: Duration = Duration::from_millis(500);

/// Event-polling demo app. `start` enters raw mode + alternate screen,
/// `run` polls on a 500 ms tick to advance the clock even when idle, and
/// `stop` restores the terminal.
struct App {
    term: Terminal<Stdin, Stdout>,
    screen: Screen<Stdout>,
    events: EventSource<Stdin>,
    started: Instant,
    log: VecDeque<String>,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut term = Terminal::stdio();
        term.make_raw()?;
        let mut screen = Screen::new(term.output(), term.window_size().unwrap_or_default());
        screen.set_alt_screen(true)?;
        screen.set_cursor_visible(false)?;
        let events = EventSource::new(term.input())?;
        Ok(Self {
            term,
            screen,
            events,
            started: Instant::now(),
            log: VecDeque::with_capacity(64),
        })
    }

    fn render(&mut self) -> std::io::Result<()> {
        redraw(&mut self.screen, &self.log, self.started.elapsed())?;
        self.screen.present()
    }

    fn run(&mut self) -> std::io::Result<()> {
        self.render()?;

        let mut next_tick = Instant::now() + TICK;
        loop {
            let remaining = next_tick.saturating_duration_since(Instant::now());
            let got = self.events.poll(Some(remaining))?;
            let mut dirty = false;

            if got {
                while let Some(ev) = self.events.try_read() {
                    match &ev {
                        Event::KeyPress(Key {
                            code: KeyCode::Char('q'),
                            modifiers,
                            ..
                        }) if modifiers.is_empty() => return Ok(()),
                        Event::KeyPress(Key {
                            code: KeyCode::Char('c'),
                            modifiers,
                            ..
                        }) if modifiers.contains(KeyModifiers::CTRL) => return Ok(()),
                        Event::Resize(ws) => {
                            self.screen.resize(ws.col, ws.row);
                            let h = self.screen.height();
                            push(&mut self.log, format!("Resize {}x{}", ws.col, ws.row), h);
                        }
                        _ => {
                            let h = self.screen.height();
                            push(&mut self.log, format!("{:?}", ev), h);
                        }
                    }
                    dirty = true;
                }
            }

            if Instant::now() >= next_tick {
                next_tick += TICK;
                // Skip ticks if we fell behind so we don't busy-spin catching up.
                let now = Instant::now();
                if next_tick < now {
                    next_tick = now + TICK;
                }
                dirty = true;
            }

            if dirty {
                self.render()?;
            }
        }
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

fn push(log: &mut VecDeque<String>, line: String, height: u16) {
    log.push_back(line);
    while log.len() as u16 > height {
        log.pop_front();
    }
}

fn redraw<W: std::io::Write>(
    screen: &mut Screen<W>,
    log: &VecDeque<String>,
    uptime: Duration,
) -> std::io::Result<()> {
    screen.clear();
    let w = screen.width();
    let header = format!(
        "Press q or Ctrl-C to quit.   uptime: {:>3}.{:01}s",
        uptime.as_secs(),
        uptime.subsec_millis() / 100,
    );
    {
        screen.set_str((0, 0), &truncate(&header, w), WrapMode::Truncate);
    };
    let body_top = 2;
    let body_height = screen.height().saturating_sub(body_top);
    let start = log.len().saturating_sub(body_height as usize);
    for (i, line) in log.iter().skip(start).enumerate() {
        let row = body_top + i as u16;
        {
            screen.set_str((0, row), &truncate(line, w), WrapMode::Truncate);
        };
    }
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
