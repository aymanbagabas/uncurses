//! Minimal interactive example, polling for events.
//!
//! Run with `cargo run --example interactive`. Press any key to see events;
//! press `q` or Ctrl-C to exit. Resize the terminal to see Resize events.
//!
//! Demonstrates the non-blocking [`Source::poll`] / [`Source::try_read`]
//! style: every 500 ms the main loop wakes up to advance a clock in the
//! header, even when no input has arrived.

use std::collections::VecDeque;
use std::io::Write;
use std::time::{Duration, Instant};

use uncurses::SurfaceMut;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, Source};
use uncurses::screen::{Options, Screen};
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses::text::WrapMode;

const TICK: Duration = Duration::from_millis(500);

fn main() -> std::io::Result<()> {
    let stdin = stdin();
    let stdout = stdout();
    let state = enable_raw_mode(stdin, stdout)?;

    let size = get_window_size(stdout).unwrap_or_default();
    let mut screen = Screen::with_options(
        stdout,
        Options {
            size: (size.col, size.row),
            ..Default::default()
        },
    );

    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(false)?;

    let mut events = Source::new(stdin)?;

    let started = Instant::now();
    let mut log: VecDeque<String> = VecDeque::with_capacity(64);
    redraw(&mut screen, &log, started.elapsed())?;
    screen.render()?;
    screen.flush()?;

    let mut next_tick = Instant::now() + TICK;
    let mut quit = false;
    while !quit {
        let remaining = next_tick.saturating_duration_since(Instant::now());
        let got = events.poll(Some(remaining))?;
        let mut dirty = false;

        if got {
            while let Some(ev) = events.try_read() {
                match &ev {
                    Event::KeyPress(Key {
                        code: KeyCode::Char('q'),
                        modifiers,
                        ..
                    }) if modifiers.is_empty() => {
                        quit = true;
                        break;
                    }
                    Event::KeyPress(Key {
                        code: KeyCode::Char('c'),
                        modifiers,
                        ..
                    }) if modifiers.contains(KeyModifiers::CTRL) => {
                        quit = true;
                        break;
                    }
                    Event::Resize(ws) => {
                        screen.resize(ws.col, ws.row);
                        push(
                            &mut log,
                            format!("Resize {}x{}", ws.col, ws.row),
                            screen.height(),
                        );
                    }
                    _ => push(&mut log, format!("{:?}", ev), screen.height()),
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
            redraw(&mut screen, &log, started.elapsed())?;
            screen.render()?;
            screen.flush()?;
        }
    }

    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin, stdout, &state)?;
    Ok(())
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
