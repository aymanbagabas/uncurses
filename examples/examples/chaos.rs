//! Stress test: pre-generates 100 frame patterns of random ANSI 16
//! glyphs/colors/attrs and renders them as fast as possible. Reports
//! FPS on exit. `Esc` or `Ctrl-C` quits.

use std::io::Write;
use std::time::Instant;

use uncurses::cell::Cell;
use uncurses::color::{BasicColor, Color};
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, Source};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};

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
        let mut style = Style::EMPTY.with_fg(fg).with_bg(bg);
        style = match attr {
            Attr::None => style,
            Attr::Bold => style.bold(),
            Attr::Italic => style.italic(),
            Attr::Reverse => style.reverse(),
        };
        cells.push(Cell::new(glyph, 1).with_style(style));
    }
    cells
}

fn build_patterns(width: u16, height: u16, rng: &mut Rng) -> Vec<Vec<Cell>> {
    (0..NUM_PATTERNS)
        .map(|_| build_pattern(width, height, rng))
        .collect()
}

fn main() -> std::io::Result<()> {
    let state = enable_raw_mode(stdin(), stdout())?;
    let size = get_window_size(stdout()).unwrap_or_default();
    let mut screen = Screen::new(stdout()).with_size(size.col, size.row);
    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(false)?;
    screen.flush()?;

    let mut events = Source::new(stdin())?;
    let mut rng = Rng::new(seed_from_clock());
    let (mut w, mut h) = (screen.width(), screen.height());
    let mut patterns = build_patterns(w, h, &mut rng);

    let start = Instant::now();
    let mut frames: u64 = 0;
    let mut quit = false;

    while !quit {
        while let Some(ev) = {
            if events.poll(Some(std::time::Duration::ZERO))? {
                events.try_read()
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
                    screen.resize(ws.col, ws.row);
                    w = screen.width();
                    h = screen.height();
                    patterns = build_patterns(w, h, &mut rng);
                }
                _ => {}
            }
        }
        if quit {
            break;
        }

        let pattern = &patterns[(frames as usize) % NUM_PATTERNS];
        for y in 0..h {
            for x in 0..w {
                let idx = y as usize * w as usize + x as usize;
                screen.set_cell((x, y), &pattern[idx]);
            }
        }
        screen.render()?;
        screen.flush()?;
        frames += 1;
    }

    let elapsed = start.elapsed().as_secs_f64();
    let fps = if elapsed > 0.0 {
        frames as f64 / elapsed
    } else {
        0.0
    };

    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state)?;
    println!("Frames: {frames} in {elapsed:.2}s — {fps:.0} FPS");
    Ok(())
}

fn seed_from_clock() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15)
}
