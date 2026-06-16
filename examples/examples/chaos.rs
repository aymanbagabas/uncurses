//! Stress test: pre-generates 100 frame patterns of random ANSI 16
//! glyphs/colors/attrs and renders them as fast as possible. Reports
//! FPS on exit. `Esc` or `Ctrl-C` quits.

use std::io::Write;
use std::time::Instant;

use uncurses::Terminal;
use uncurses::cell::Cell;
use uncurses::color::{BasicColor, Color};
use uncurses::event::{Event, EventSource, Key, KeyCode, KeyModifiers};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};

const NUM_PATTERNS: usize = 100;
const GLYPHS: &[&str] = &["@", "#", "&", "*", "=", "%", "Z", "A"];

#[derive(Clone, Copy)]
enum Attr {
    None,
    Bold,
    Italic,
    Reverse,
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn pick<T: Copy>(&mut self, slice: &[T]) -> T {
        slice[(self.next_u64() as usize) % slice.len()]
    }
    fn color(&mut self) -> Color {
        Color::Basic(BasicColor::from_u8((self.next_u64() % 16) as u8).unwrap())
    }
}

fn build_pattern(width: u16, height: u16, rng: &mut Rng) -> Vec<Cell> {
    let attrs = [Attr::None, Attr::Bold, Attr::Italic, Attr::Reverse];
    let mut cells = Vec::with_capacity(width as usize * height as usize);
    for _ in 0..(width as usize * height as usize) {
        let glyph = rng.pick(GLYPHS);
        let fg = rng.color();
        let bg = rng.color();
        let attr = rng.pick(&attrs);
        let mut style = Style::default().fg(fg).bg(bg);
        style = match attr {
            Attr::None => style,
            Attr::Bold => style.bold(),
            Attr::Italic => style.italic(),
            Attr::Reverse => style.reverse(),
        };
        cells.push(Cell::narrow(glyph).style(style));
    }
    cells
}

fn build_patterns(width: u16, height: u16, rng: &mut Rng) -> Vec<Vec<Cell>> {
    (0..NUM_PATTERNS)
        .map(|_| build_pattern(width, height, rng))
        .collect()
}

struct App {
    term: Terminal<Stdin, Stdout>,
    screen: Screen<Stdout>,
    events: EventSource<Stdin>,
    rng: Rng,
    w: u16,
    h: u16,
    patterns: Vec<Vec<Cell>>,
    start: Instant,
    frames: u64,
    summary: Option<(u64, f64, f64)>,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut term = Terminal::stdio();
        term.make_raw()?;
        let mut screen = Screen::new(term.output(), term.window_size().unwrap_or_default());
        screen.set_alt_screen(true)?;
        screen.set_cursor_visible(false)?;
        screen.flush()?;

        let events = EventSource::new(term.input())?;
        let mut rng = Rng::new(seed_from_clock());
        let (w, h) = (screen.width(), screen.height());
        let patterns = build_patterns(w, h, &mut rng);

        Ok(Self {
            term,
            screen,
            events,
            rng,
            w,
            h,
            patterns,
            start: Instant::now(),
            frames: 0,
            summary: None,
        })
    }

    fn render(&mut self) -> std::io::Result<()> {
        let pattern = &self.patterns[(self.frames as usize) % NUM_PATTERNS];
        for y in 0..self.h {
            for x in 0..self.w {
                let idx = y as usize * self.w as usize + x as usize;
                self.screen.set_cell((x, y), &pattern[idx]);
            }
        }
        self.screen.present()
    }

    fn run(&mut self) -> std::io::Result<()> {
        loop {
            let mut quit = false;
            while let Some(ev) = {
                if self.events.poll(Some(std::time::Duration::ZERO))? {
                    self.events.try_read()
                } else {
                    None
                }
            } {
                match ev {
                    Event::KeyPress(Key {
                        code: KeyCode::Escape,
                        ..
                    }) => quit = true,
                    Event::KeyPress(Key {
                        code: KeyCode::Char('c'),
                        modifiers,
                        ..
                    }) if modifiers.contains(KeyModifiers::CTRL) => quit = true,
                    Event::Resize(ws) => {
                        self.screen.resize(ws.col, ws.row);
                        self.w = self.screen.width();
                        self.h = self.screen.height();
                        self.patterns = build_patterns(self.w, self.h, &mut self.rng);
                    }
                    _ => {}
                }
            }
            if quit {
                break;
            }

            self.render()?;
            self.frames += 1;
        }

        let elapsed = self.start.elapsed().as_secs_f64();
        let fps = if elapsed > 0.0 {
            self.frames as f64 / elapsed
        } else {
            0.0
        };
        self.summary = Some((self.frames, elapsed, fps));

        Ok(())
    }

    fn stop(&mut self) -> std::io::Result<()> {
        self.screen.reset()?;
        self.screen.flush()?;
        self.term.restore()
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some((frames, elapsed, fps)) = self.summary {
            println!("Frames: {frames} in {elapsed:.2}s — {fps:.0} FPS");
        }
    }
}

fn main() -> std::io::Result<()> {
    let mut app = App::start()?;
    let result = app.run();
    app.stop()?;
    result
}

fn seed_from_clock() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15)
}
