//! Uncapped variant of the [`space`] example.
//!
//! Renders the animated grayscale starfield as fast as the renderer can
//! push frames out. Input is drained opportunistically between frames so
//! the render loop never blocks waiting on the keyboard. Useful for
//! measuring raw renderer throughput.
//!
//! Run with `cargo run --release --example space_unlimited`. Press `q`
//! or `Ctrl-C` to quit.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::cell::Cell;
use uncurses::color::Color;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers};
use uncurses::program::Program;
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

const GLYPH: &str = "\u{2580}";

struct Fps {
    frames: u32,
    last: Instant,
    value: Option<f32>,
    draw_ns: u128,
    render_ns: u128,
    flush_ns: u128,
    stats: Option<StageStats>,
}

#[derive(Copy, Clone)]
struct StageStats {
    draw_us: f32,
    render_us: f32,
    flush_us: f32,
}

impl Fps {
    fn new() -> Self {
        Self {
            frames: 0,
            last: Instant::now(),
            value: None,
            draw_ns: 0,
            render_ns: 0,
            flush_ns: 0,
            stats: None,
        }
    }

    fn record(&mut self, draw: Duration, render: Duration, flush: Duration) {
        self.frames += 1;
        self.draw_ns += draw.as_nanos();
        self.render_ns += render.as_nanos();
        self.flush_ns += flush.as_nanos();
        let elapsed = self.last.elapsed();
        if elapsed >= Duration::from_secs(1) && self.frames > 2 {
            let n = f64::from(self.frames);
            self.value = Some(self.frames as f32 / elapsed.as_secs_f32());
            self.stats = Some(StageStats {
                draw_us: (self.draw_ns as f64 / n / 1000.0) as f32,
                render_us: (self.render_ns as f64 / n / 1000.0) as f32,
                flush_us: (self.flush_ns as f64 / n / 1000.0) as f32,
            });
            self.frames = 0;
            self.draw_ns = 0;
            self.render_ns = 0;
            self.flush_ns = 0;
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

/// Uncapped starfield app. `start` enters raw mode + alternate screen
/// and `run` renders frames as fast as possible while draining input.
struct App {
    program: Program<Stdin, Stdout>,
    rng: Rng,
    field: Field,
    fps: Fps,
    frame_count: u32,
}

impl App {
    fn start() -> io::Result<Self> {
        let mut program = Program::stdio()?;
        program.init()?;
        program.enter_alt_screen()?;
        program.hide_cursor()?;

        Ok(Self {
            program,
            rng: Rng::new(seed_from_clock()),
            field: Field::new(),
            fps: Fps::new(),
            frame_count: 0,
        })
    }

    fn render(&mut self) -> io::Result<()> {
        self.frame_count = self.frame_count.wrapping_add(1);
        let t0 = Instant::now();
        draw(
            self.program.screen_mut(),
            &mut self.field,
            &mut self.rng,
            &self.fps,
            self.frame_count,
        );
        let t1 = Instant::now();
        self.program.screen_mut().render()?;
        let t2 = Instant::now();
        self.program.screen_mut().flush()?;
        let t3 = Instant::now();
        self.fps.record(t1 - t0, t2 - t1, t3 - t2);
        Ok(())
    }

    fn run(&mut self) -> io::Result<()> {
        loop {
            // Drain pending input without blocking the render loop.
            while self.program.poll_event(Some(Duration::ZERO))? {
                let Some(ev) = self.program.try_read_event()? else {
                    break;
                };
                match ev {
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
                    Event::Resize(_) => {
                        self.program.autoresize()?;
                        self.field = Field::new();
                    }
                    _ => {}
                }
            }
            self.render()?;
        }
    }

    fn stop(self) -> io::Result<()> {
        self.program.finish()
    }
}

fn main() -> io::Result<()> {
    let mut app = App::start()?;
    let result = app.run();
    app.stop()?;
    result
}

fn draw(
    screen: &mut Screen<Stdout>,
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
            let cell = glyph_cell.clone().style(Style::default().fg(fg).bg(bg));
            screen.set_cell((x, y), &cell);
        }
    }

    screen.clear_rect(uncurses::layout::Rect::new(0, 0, width, 1));
    let header = "space (unlimited) — press q to quit";
    screen.set_str(
        (0, 0),
        &truncate(header, width),
        uncurses::style::Style::default(),
    );

    if let Some(value) = fps.value {
        let label = if let Some(s) = fps.stats {
            format!(
                "{value:.1} fps  draw {:.0}µs  render {:.0}µs  flush {:.0}µs",
                s.draw_us, s.render_us, s.flush_us,
            )
        } else {
            format!("{value:.1} fps")
        };
        let label_w = label.chars().count() as u16;
        if label_w < width {
            screen.set_str(
                (width - label_w, 0),
                &label,
                uncurses::style::Style::default(),
            );
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
