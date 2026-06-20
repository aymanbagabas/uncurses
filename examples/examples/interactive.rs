//! Minimal interactive example, polling for events.
//!
//! Run with `cargo run --example interactive`. Press any key to see events;
//! press `q` or Ctrl-C to exit. Resize the terminal to see Resize events.
//!
//! Demonstrates the non-blocking [`Screen::poll`] / [`Screen::try_read`]
//! style: every 500 ms the main loop wakes up to advance a clock in the
//! header, even when no input has arrived.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::event::{Event, Key, KeyCode, KeyModifiers};
use uncurses::screen::Screen;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

const TICK: Duration = Duration::from_millis(500);

/// Event-polling demo app. `start` enters raw mode + alternate screen,
/// `run` polls on a 500 ms tick to advance the clock even when idle.
struct App {
    screen: Screen<Stdin, Stdout>,
    started: Instant,
    log: VecDeque<String>,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut screen = Screen::stdio()?;
        screen.init()?;
        screen.enter_alt_screen()?;
        screen.hide_cursor()?;
        Ok(Self {
            screen,
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
            let got = self.screen.poll(Some(remaining))?;
            let mut dirty = false;

            if got {
                while let Some(ev) = self.screen.try_read() {
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
                            self.screen.resize((ws.col, ws.row));
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

fn push(log: &mut VecDeque<String>, line: String, height: u16) {
    log.push_back(line);
    while log.len() as u16 > height {
        log.pop_front();
    }
}

fn redraw(
    screen: &mut Screen<Stdin, Stdout>,
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
        screen.set_str(
            (0, 0),
            &truncate(&header, w),
            uncurses::style::Style::default(),
        );
    };
    let body_top = 2;
    let body_height = screen.height().saturating_sub(body_top);
    let start = log.len().saturating_sub(body_height as usize);
    for (i, line) in log.iter().skip(start).enumerate() {
        let row = body_top + i as u16;
        {
            screen.set_str(
                (0, row),
                &truncate(line, w),
                uncurses::style::Style::default(),
            );
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
