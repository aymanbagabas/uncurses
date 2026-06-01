//! Two-view demo: choose a task, then watch a progress bar fill.
//!
//! View 1: pick from a list with `j`/`k` (or arrows), confirm with `Enter`.
//! View 2: progress bar fills, then auto-exits after a 3-second countdown.
//! `q`, `Esc`, or `Ctrl-C` exits at any time.

use std::io::Write;
use std::time::{Duration, Instant};

use uncurses::SurfaceMut;
use uncurses::color::BasicColor;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, Source};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses::text::WrapMode;

const CHOICES: &[&str] = &[
    "Plant carrots",
    "Go to the market",
    "Read something",
    "See friends",
];

const BAR_WIDTH: u16 = 50;
const TICK: Duration = Duration::from_secs(1);
const FRAME: Duration = Duration::from_micros(16_667);
// Choice view: header + blank + 4 choices + blank + countdown + blank + footer.
// Loaded view: header + blank + deps + blank + label + bar + blank + countdown.
// 11 rows fits both.
const VIEW_H: u16 = 11;

#[derive(Default)]
struct State {
    choice: usize,
    chosen: bool,
    progress: f32,
    loaded: bool,
    ticks: u32,
}

fn main() -> std::io::Result<()> {
    let state_term = enable_raw_mode(stdin(), stdout())?;
    let size = get_window_size(stdout()).unwrap_or_default();
    let mut screen = Screen::new(stdout()).with_size(size.col, VIEW_H);
    screen.set_cursor_visible(false)?;

    let mut events = Source::new(stdin())?;
    let mut s = State {
        ticks: 10,
        ..Default::default()
    };
    let mut quit = false;

    let mut next_tick = Instant::now() + TICK;
    let mut next_frame = Instant::now() + FRAME;

    redraw(&mut screen, &s);
    screen.render()?;
    screen.flush()?;

    while !quit {
        let now = Instant::now();
        let next = if s.chosen && !s.loaded {
            next_frame.min(next_tick)
        } else {
            next_tick
        };
        let timeout = next.saturating_duration_since(now);

        let mut dirty = false;
        if events.poll(Some(timeout))? {
            while let Some(ev) = events.try_read() {
                match ev {
                    Event::KeyPress(Key {
                        code: KeyCode::Char('q') | KeyCode::Escape,
                        modifiers,
                        ..
                    }) if modifiers.is_empty() => quit = true,
                    Event::KeyPress(Key {
                        code: KeyCode::Char('c'),
                        modifiers,
                        ..
                    }) if modifiers.contains(KeyModifiers::CTRL) => quit = true,
                    Event::KeyPress(Key { code, .. }) if !s.chosen => match code {
                        KeyCode::Char('j') | KeyCode::Down if s.choice + 1 < CHOICES.len() => {
                            s.choice += 1;
                            dirty = true;
                        }
                        KeyCode::Char('k') | KeyCode::Up if s.choice > 0 => {
                            s.choice -= 1;
                            dirty = true;
                        }
                        KeyCode::Enter => {
                            s.chosen = true;
                            s.progress = 0.0;
                            next_frame = Instant::now() + FRAME;
                            dirty = true;
                        }
                        _ => {}
                    },
                    Event::Resize(ws) => {
                        screen.resize(ws.col, VIEW_H);
                        dirty = true;
                    }
                    _ => {}
                }
            }
        }

        let now = Instant::now();
        if s.chosen && !s.loaded && now >= next_frame {
            next_frame += FRAME;
            if next_frame < now {
                next_frame = now + FRAME;
            }
            s.progress = (s.progress + 0.01).min(1.0);
            if s.progress >= 1.0 {
                s.loaded = true;
                s.ticks = 3;
                next_tick = now + TICK;
            }
            dirty = true;
        }
        if now >= next_tick {
            next_tick += TICK;
            if next_tick < now {
                next_tick = now + TICK;
            }
            // Tick fires for the choice-screen countdown, and again after
            // loading completes (exit countdown).
            if !s.chosen || s.loaded {
                if s.ticks == 0 {
                    quit = true;
                } else {
                    s.ticks -= 1;
                }
                dirty = true;
            }
        }

        if dirty && !quit {
            redraw(&mut screen, &s);
            screen.render()?;
            screen.flush()?;
        }
    }

    screen.resize(screen.width(), 3);
    screen.clear();
    screen.set_str((2, 1), "Bye!", WrapMode::Truncate);
    screen.render()?;

    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state_term)?;
    Ok(())
}

fn redraw<W: Write>(screen: &mut Screen<W>, s: &State) {
    screen.clear();
    if s.chosen {
        draw_chosen(screen, s);
    } else {
        draw_choices(screen, s);
    }
}

fn draw_choices<W: Write>(screen: &mut Screen<W>, s: &State) {
    let subtle = Style::EMPTY.with_fg(BasicColor::BrightBlack.into());
    let checkbox = Style::EMPTY.with_fg(BasicColor::Cyan.into()).bold();
    let ticks_st = Style::EMPTY.with_fg(BasicColor::Yellow.into()).bold();

    let mut y = 1u16;
    screen.set_str((2, y), "What to do today?", WrapMode::Truncate);
    y += 2;

    for (i, choice) in CHOICES.iter().enumerate() {
        let row = y + i as u16;
        if i == s.choice {
            let line = format!("[x] {choice}");
            screen.set_str_with((2, row), &line, WrapMode::Truncate, checkbox.clone());
        } else {
            let line = format!("[ ] {choice}");
            screen.set_str_with((2, row), &line, WrapMode::Truncate, subtle.clone());
        }
    }

    y += CHOICES.len() as u16 + 1;
    screen.set_str((2, y), "Program quits in", WrapMode::Truncate);
    let n = format!(" {} ", s.ticks);
    screen.set_str_with((2 + 17, y), &n, WrapMode::Truncate, ticks_st.clone());
    let after_x = 2 + 17 + n.chars().count() as u16;
    screen.set_str((after_x, y), "seconds", WrapMode::Truncate);

    y += 2;
    screen.set_str_with(
        (2, y),
        "j/k or up/down: select  •  enter: choose  •  q: quit",
        WrapMode::Truncate,
        subtle.clone(),
    );
}

fn draw_chosen<W: Write>(screen: &mut Screen<W>, s: &State) {
    let keyword = Style::EMPTY
        .with_fg(BasicColor::BrightMagenta.into())
        .bold();
    let ticks_st = Style::EMPTY.with_fg(BasicColor::Yellow.into()).bold();
    let bar_st = Style::EMPTY.with_fg(BasicColor::BrightGreen.into());
    let empty_st = Style::EMPTY.with_fg(BasicColor::BrightBlack.into());

    let (head, deps): (&str, [&str; 2]) = match s.choice {
        0 => ("Carrot planting?", ["libgarden", "vegeutils"]),
        1 => ("A trip to the market?", ["marketkit", "libshopping"]),
        2 => ("Reading time?", ["a library", "an actual one"]),
        _ => (
            "It's always good to see friends.",
            ["social-skills", "conversationutils"],
        ),
    };

    let mut y = 1u16;
    screen.set_str((2, y), head, WrapMode::Truncate);
    y += 2;

    let prefix = "Need ";
    screen.set_str((2, y), prefix, WrapMode::Truncate);
    let mut x = 2 + prefix.chars().count() as u16;
    screen.set_str_with((x, y), deps[0], WrapMode::Truncate, keyword.clone());
    x += deps[0].chars().count() as u16;
    screen.set_str((x, y), " and ", WrapMode::Truncate);
    x += 5;
    screen.set_str_with((x, y), deps[1], WrapMode::Truncate, keyword.clone());
    x += deps[1].chars().count() as u16;
    screen.set_str((x, y), "...", WrapMode::Truncate);
    y += 2;

    let label = if s.loaded { "Done." } else { "Downloading..." };
    screen.set_str((2, y), label, WrapMode::Truncate);
    y += 1;

    let filled = (BAR_WIDTH as f32 * s.progress).round() as u16;
    let filled_str: String = "█".repeat(filled as usize);
    let empty_str: String = "░".repeat((BAR_WIDTH - filled) as usize);
    screen.set_str_with((2, y), &filled_str, WrapMode::Truncate, bar_st.clone());
    screen.set_str_with(
        (2 + filled, y),
        &empty_str,
        WrapMode::Truncate,
        empty_st.clone(),
    );
    let pct = format!(" {:>3.0}%", s.progress * 100.0);
    screen.set_str((2 + BAR_WIDTH, y), &pct, WrapMode::Truncate);

    if s.loaded {
        y += 2;
        screen.set_str((2, y), "Exiting in", WrapMode::Truncate);
        let n = format!(" {} ", s.ticks);
        screen.set_str_with((2 + 11, y), &n, WrapMode::Truncate, ticks_st.clone());
        let after_x = 2 + 11 + n.chars().count() as u16;
        screen.set_str((after_x, y), "seconds", WrapMode::Truncate);
    }
}
