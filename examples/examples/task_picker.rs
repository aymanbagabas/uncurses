//! Two-view demo: choose a task, then watch a progress bar fill.
//!
//! View 1: pick from a list with `j`/`k` (or arrows), confirm with `Enter`.
//! View 2: progress bar fills, then auto-exits after a 3-second countdown.
//! `q`, `Esc`, or `Ctrl-C` exits at any time.
//!
//! Inline screen: each renderer paints into a [`Painter`] and reports
//! the number of rows it wanted to use, and the screen is resized to
//! match. The screen height is *always* the current frame's height —
//! never the terminal window.

use std::io::Write;
use std::time::{Duration, Instant};

use uncurses::buffer::SurfaceMut;
use uncurses::color::BasicColor;
use uncurses::event::{Event, EventSource, Key, KeyCode, KeyModifiers};
use uncurses::screen::Screen;
use uncurses::style::{Style, write_style};
use uncurses::terminal::Terminal;
use uncurses::terminal::{Stdin, Stdout};
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

#[derive(Default)]
struct State {
    choice: usize,
    chosen: bool,
    progress: f32,
    loaded: bool,
    ticks: u32,
}

struct App {
    term: Terminal<Stdin, Stdout>,
    screen: Screen<Stdout>,
    events: EventSource<Stdin>,
    state: State,
    term_cols: u16,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut term = Terminal::stdio();
        term.make_raw()?;
        let size = term.window_size().unwrap_or_default();
        let term_cols = size.col;
        let state = State {
            ticks: 10,
            ..Default::default()
        };
        // Start at a single row; the first redraw will grow the screen to
        // match the first frame's measured height.
        let mut screen = Screen::new(term.output(), (term_cols, 1));
        screen.set_cursor_visible(false);
        let events = EventSource::new(term.input())?;

        Ok(Self {
            term,
            screen,
            events,
            state,
            term_cols,
        })
    }

    fn render(&mut self) -> std::io::Result<()> {
        fit_and_redraw(&mut self.screen, &self.state, self.term_cols);
        self.screen.present()
    }

    fn run(&mut self) -> std::io::Result<()> {
        let mut next_tick = Instant::now() + TICK;
        let mut next_frame = Instant::now() + FRAME;

        self.render()?;

        'running: loop {
            let now = Instant::now();
            let next = if self.state.chosen && !self.state.loaded {
                next_frame.min(next_tick)
            } else {
                next_tick
            };
            let timeout = next.saturating_duration_since(now);

            let mut dirty = false;
            if self.events.poll(Some(timeout))? {
                while let Some(ev) = self.events.try_read() {
                    match ev {
                        Event::KeyPress(Key {
                            code: KeyCode::Char('q') | KeyCode::Escape,
                            modifiers,
                            ..
                        }) if modifiers.is_empty() => break 'running,
                        Event::KeyPress(Key {
                            code: KeyCode::Char('c'),
                            modifiers,
                            ..
                        }) if modifiers.contains(KeyModifiers::CTRL) => break 'running,
                        Event::KeyPress(Key { code, .. }) if !self.state.chosen => match code {
                            KeyCode::Char('j') | KeyCode::Down
                                if self.state.choice + 1 < CHOICES.len() =>
                            {
                                self.state.choice += 1;
                                dirty = true;
                            }
                            KeyCode::Char('k') | KeyCode::Up if self.state.choice > 0 => {
                                self.state.choice -= 1;
                                dirty = true;
                            }
                            KeyCode::Enter => {
                                self.state.chosen = true;
                                self.state.progress = 0.0;
                                next_frame = Instant::now() + FRAME;
                                dirty = true;
                            }
                            _ => {}
                        },
                        Event::Resize(ws) => {
                            self.term_cols = ws.col;
                            dirty = true;
                        }
                        _ => {}
                    }
                }
            }

            let now = Instant::now();
            if self.state.chosen && !self.state.loaded && now >= next_frame {
                next_frame += FRAME;
                if next_frame < now {
                    next_frame = now + FRAME;
                }
                self.state.progress = (self.state.progress + 0.01).min(1.0);
                if self.state.progress >= 1.0 {
                    self.state.loaded = true;
                    self.state.ticks = 3;
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
                if !self.state.chosen || self.state.loaded {
                    if self.state.ticks == 0 {
                        break 'running;
                    } else {
                        self.state.ticks -= 1;
                    }
                    dirty = true;
                }
            }

            if dirty {
                self.render()?;
            }
        }

        // Bye: "Bye!" on row 1 plus a trailing blank row so the prompt
        // returns on its own line below the message.
        self.screen.resize(self.term_cols, 3);
        self.screen.clear();
        self.screen.set_str((2, 1), "Bye!", WrapMode::Truncate);
        self.screen.render();

        Ok(())
    }

    fn stop(&mut self) -> std::io::Result<()> {
        self.screen.reset();
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

/// Paint the current frame and size the screen to the rows it used.
///
/// Each renderer tracks its own intended row count independent of any
/// clipping, so a too-small initial buffer simply triggers a resize and
/// a single repaint.
fn fit_and_redraw<W: Write>(screen: &mut Screen<W>, s: &State, cols: u16) {
    screen.clear();
    let needed = paint(screen, s);
    if screen.width() != cols || screen.height() != needed {
        screen.resize(cols, needed);
        screen.clear();
        paint(screen, s);
    }
}

fn paint<W: Write>(screen: &mut Screen<W>, s: &State) -> u16 {
    if s.chosen {
        draw_chosen(screen, s)
    } else {
        draw_choices(screen, s)
    }
}

/// SGR escape sequence for `style`, suitable for embedding in a string
/// painted by the screen (which interprets inline `CSI ... m`).
fn sgr(style: &Style) -> String {
    let mut buf = Vec::with_capacity(24);
    write_style(&mut buf, style).expect("write to Vec is infallible");
    String::from_utf8(buf).expect("SGR bytes are ASCII")
}

/// SGR reset (`ESC [ m`) — restores [`Style::default()`] for following cells.
const RESET: &str = "\x1b[m";

fn draw_choices<W: Write>(screen: &mut Screen<W>, s: &State) -> u16 {
    let subtle = sgr(&Style::default().fg(BasicColor::BrightBlack.into()));
    let checkbox = sgr(&Style::default().fg(BasicColor::Cyan.into()).bold());
    let ticks_st = sgr(&Style::default().fg(BasicColor::Yellow.into()).bold());

    let mut last = 0u16;
    let mut y = 1u16;
    screen.set_str((2, y), "What to do today?", WrapMode::Truncate);
    last = last.max(y);
    y += 2;

    for (i, choice) in CHOICES.iter().enumerate() {
        let row = y + i as u16;
        let line = if i == s.choice {
            format!("{checkbox}[x] {choice}{RESET}")
        } else {
            format!("{subtle}[ ] {choice}{RESET}")
        };
        screen.set_str((2, row), &line, WrapMode::Truncate);
        last = last.max(row);
    }

    y += CHOICES.len() as u16 + 1;
    let line = format!("Program quits in {ticks_st}{RESET} seconds");
    screen.set_str((2, y), &line, WrapMode::Truncate);
    last = last.max(y);

    y += 2;
    let line = format!("{subtle}j/k or up/down: select  •  enter: choose  •  q: quit{RESET}");
    screen.set_str((2, y), &line, WrapMode::Truncate);
    last = last.max(y);

    last + 1
}

fn draw_chosen<W: Write>(screen: &mut Screen<W>, s: &State) -> u16 {
    let keyword = sgr(&Style::default().fg(BasicColor::BrightMagenta.into()).bold());
    let ticks_st = sgr(&Style::default().fg(BasicColor::Yellow.into()).bold());
    let bar_st = sgr(&Style::default().fg(BasicColor::BrightGreen.into()));
    let empty_st = sgr(&Style::default().fg(BasicColor::BrightBlack.into()));

    let (head, deps): (&str, [&str; 2]) = match s.choice {
        0 => ("Carrot planting?", ["libgarden", "vegeutils"]),
        1 => ("A trip to the market?", ["marketkit", "libshopping"]),
        2 => ("Reading time?", ["a library", "an actual one"]),
        _ => (
            "It's always good to see friends.",
            ["social-skills", "conversationutils"],
        ),
    };

    let mut last = 0u16;
    let mut y = 1u16;
    screen.set_str((2, y), head, WrapMode::Truncate);
    last = last.max(y);
    y += 2;

    let line = format!(
        "Need {keyword}{}{RESET} and {keyword}{}{RESET}...",
        deps[0], deps[1]
    );
    screen.set_str((2, y), &line, WrapMode::Truncate);
    last = last.max(y);
    y += 2;

    let label = if s.loaded { "Done." } else { "Downloading..." };
    screen.set_str((2, y), label, WrapMode::Truncate);
    last = last.max(y);
    y += 1;

    let filled = (BAR_WIDTH as f32 * s.progress).round() as u16;
    let filled_str = "█".repeat(filled as usize);
    let empty_str = "░".repeat((BAR_WIDTH - filled) as usize);
    let pct = format!(" {:>3.0}%", s.progress * 100.0);
    let bar = format!("{bar_st}{filled_str}{empty_st}{empty_str}{RESET}{pct}");
    screen.set_str((2, y), &bar, WrapMode::Truncate);
    last = last.max(y);

    if s.loaded {
        y += 2;
        let line = format!("Exiting in {ticks_st}{}{RESET} seconds", s.ticks);
        screen.set_str((2, y), &line, WrapMode::Truncate);
        last = last.max(y);
    }

    last + 1
}
