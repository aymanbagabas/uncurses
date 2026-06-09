//! Animated grayscale starfield demo, capped at 60 FPS.
//!
//! A scrolling field of half-block "stars" rendered at one cell per
//! two vertical pixels via the `▀` glyph (background = lower pixel).
//! The frame loop runs at ~60 Hz: between frames the loop blocks on
//! `Source::poll` so input is consumed without busy-waiting.
//!
//! Run with `cargo run --release --example space`. Press `q` or
//! `Ctrl-C` to quit.

use std::io::Write;
use std::time::{Duration, Instant};

use uncurses::SurfaceMut;
use uncurses::cell::Cell;
use uncurses::color::Color;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, Source};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses::text::WrapMode;

const FRAME: Duration = Duration::from_micros(16_667); // ~60 FPS
const GLYPH: &str = "\u{2580}";

struct Fps {
    frames: u32,
    last: Instant,
    value: Option<f32>,
}

impl Fps {
    fn new() -> Self {
        Self {
            frames: 0,
            last: Instant::now(),
            value: None,
        }
    }

    fn tick(&mut self) {
        self.frames += 1;
        let elapsed = self.last.elapsed();
        if elapsed >= Duration::from_secs(1) && self.frames > 2 {
            self.value = Some(self.frames as f32 / elapsed.as_secs_f32());
            self.frames = 0;
            self.last = Instant::now();
        }
    }
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

    fn jitter(&mut self, bound: f32) -> f32 {
        let n = (self.next_u64() & 0xFFFF) as f32 / 65536.0;
        (n * 2.0 - 1.0) * bound
    }
}

struct Field {
    width: u16,
    pixel_height: u16,
    colors: Vec<Color>,
}

impl Field {
    fn new() -> Self {
        Self {
            width: 0,
            pixel_height: 0,
            colors: Vec::new(),
        }
    }

    fn ensure(&mut self, width: u16, height: u16, rng: &mut Rng) {
        let pixel_height = height.saturating_mul(2);
        if width == self.width && pixel_height == self.pixel_height {
            return;
        }
        self.width = width;
        self.pixel_height = pixel_height;
        let len = width as usize * pixel_height as usize;
        self.colors.clear();
        self.colors.reserve(len);
        let ph = pixel_height as f32;
        for y in 0..pixel_height {
            let depth = (pixel_height - y) as f32 / ph;
            let base = depth * depth;
            for _ in 0..width {
                let v = (base + rng.jitter(0.1)).clamp(0.0, 1.0);
                let b = (v * 255.0).round() as u8;
                self.colors.push(Color::Rgb(b, b, b));
            }
        }
    }

    fn at(&self, x: u16, y: u16) -> Color {
        let idx = y as usize * self.width as usize + x as usize;
        self.colors[idx]
    }
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
    let mut field = Field::new();
    let mut fps = Fps::new();
    let mut frame_count: u32 = 0;
    let mut next_frame = Instant::now() + FRAME;
    let mut quit = false;
    let mut needs_redraw = true;

    while !quit {
        let now = Instant::now();
        let remaining = next_frame.saturating_duration_since(now);

        if !remaining.is_zero() && events.poll(Some(remaining))? {
            while let Some(ev) = events.try_read() {
                match ev {
                    Event::KeyPress(Key {
                        code: KeyCode::Char('q'),
                        modifiers,
                        ..
                    }) if modifiers.is_empty() => quit = true,
                    Event::KeyPress(Key {
                        code: KeyCode::Char('c'),
                        modifiers,
                        ..
                    }) if modifiers.contains(KeyModifiers::CTRL) => quit = true,
                    Event::Resize(ws) => {
                        screen.resize(ws.col, ws.row);
                        field = Field::new();
                        needs_redraw = true;
                    }
                    _ => {}
                }
            }
        }

        if Instant::now() >= next_frame {
            next_frame += FRAME;
            let now = Instant::now();
            if next_frame < now {
                next_frame = now + FRAME;
            }
            frame_count = frame_count.wrapping_add(1);
            fps.tick();
            needs_redraw = true;
        }

        if needs_redraw && !quit {
            draw(&mut screen, &mut field, &mut rng, &fps, frame_count);
            screen.render()?;
            screen.flush()?;
            needs_redraw = false;
        }
    }

    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state)?;
    Ok(())
}

fn draw<W: Write>(
    screen: &mut Screen<W>,
    field: &mut Field,
    rng: &mut Rng,
    fps: &Fps,
    frame_count: u32,
) {
    let width = screen.width();
    let height = screen.height();
    if width == 0 || height == 0 {
        return;
    }

    field.ensure(width, height, rng);

    let body_top: u16 = 1;
    if height <= body_top {
        return;
    }

    let shift = frame_count as usize;
    let glyph_cell = Cell::narrow(GLYPH);

    for y in body_top..height {
        let py = (y - body_top) as usize * 2;
        for x in 0..width {
            let sx = ((x as usize) + shift) % width as usize;
            let fg = field.at(sx as u16, py as u16);
            let bg = field.at(sx as u16, (py + 1) as u16);
            let cell = glyph_cell
                .clone()
                .with_style(Style::EMPTY.with_fg(fg).with_bg(bg));
            screen.set_cell((x, y), &cell);
        }
    }

    screen.clear_rect(uncurses::Rect::new(0, 0, width, 1));
    let header = "space — press q to quit";
    screen.set_str((0, 0), &truncate(header, width), WrapMode::Truncate);

    if let Some(value) = fps.value {
        let label = format!("{value:.1} fps");
        let label_w = label.chars().count() as u16;
        if label_w < width {
            screen.set_str((width - label_w, 0), &label, WrapMode::Truncate);
        }
    }
}

fn truncate(s: &str, width: u16) -> String {
    let max = width as usize;
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

fn seed_from_clock() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15)
}
