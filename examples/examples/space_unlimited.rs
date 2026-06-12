//! Uncapped variant of the [`space`] example.
//!
//! Renders the animated grayscale starfield as fast as the renderer can
//! push frames out. Input runs on a dedicated thread that forwards
//! events through a channel so the render loop never blocks waiting on
//! the keyboard. Useful for measuring raw renderer throughput.
//!
//! Run with `cargo run --release --example space_unlimited`. Press `q`
//! or `Ctrl-C` to quit.

use std::io::{self, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use uncurses::SurfaceMut;
use uncurses::cell::Cell;
use uncurses::color::Color;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, Source};
use uncurses::screen::{Options, Screen};
use uncurses::style::Style;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses::text::WrapMode;

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

enum InputMsg {
    Resize(u16, u16),
    Quit,
}

fn main() -> io::Result<()> {
    let state = enable_raw_mode(stdin(), stdout())?;
    let size = get_window_size(stdout()).unwrap_or_default();
    let mut screen = Screen::with_options(
        stdout(),
        Options {
            size: (size.col, size.row),
            ..Default::default()
        },
    );

    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(false)?;
    screen.flush()?;

    let (tx, rx) = mpsc::channel::<InputMsg>();
    let input_handle = thread::Builder::new()
        .name("input".into())
        .spawn(move || input_loop(tx))
        .expect("spawn input thread");

    let mut rng = Rng::new(seed_from_clock());
    let mut field = Field::new();
    let mut fps = Fps::new();
    let mut frame_count: u32 = 0;
    let mut quit = false;

    while !quit {
        loop {
            match rx.try_recv() {
                Ok(InputMsg::Quit) => {
                    quit = true;
                    break;
                }
                Ok(InputMsg::Resize(cols, rows)) => {
                    screen.resize(cols, rows);
                    field = Field::new();
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    quit = true;
                    break;
                }
            }
        }
        if quit {
            break;
        }

        frame_count = frame_count.wrapping_add(1);
        let t0 = Instant::now();
        draw(&mut screen, &mut field, &mut rng, &fps, frame_count);
        let t1 = Instant::now();
        screen.render()?;
        let t2 = Instant::now();
        screen.flush()?;
        let t3 = Instant::now();
        fps.record(t1 - t0, t2 - t1, t3 - t2);
    }

    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state)?;

    // The input thread exits on its next read once stdin returns EOF or
    // the user releases the next key; we do not wait indefinitely.
    drop(input_handle);
    Ok(())
}

fn input_loop(tx: mpsc::Sender<InputMsg>) {
    let mut events = match Source::new(stdin()) {
        Ok(r) => r,
        Err(_) => return,
    };
    loop {
        match events.read() {
            Ok(ev) => match ev {
                Event::KeyPress(Key {
                    code: KeyCode::Char('q'),
                    modifiers,
                    ..
                }) if modifiers.is_empty() => {
                    let _ = tx.send(InputMsg::Quit);
                    return;
                }
                Event::KeyPress(Key {
                    code: KeyCode::Char('c'),
                    modifiers,
                    ..
                }) if modifiers.contains(KeyModifiers::CTRL) => {
                    let _ = tx.send(InputMsg::Quit);
                    return;
                }
                Event::Resize(ws) if tx.send(InputMsg::Resize(ws.col, ws.row)).is_err() => {
                    return;
                }
                _ => {}
            },
            Err(_) => return,
        }
    }
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
            let cell = glyph_cell.clone().style(Style::default().fg(fg).bg(bg));
            screen.set_cell((x, y), &cell);
        }
    }

    screen.clear_rect(uncurses::Rect::new(0, 0, width, 1));
    let header = "space (unlimited) — press q to quit";
    screen.set_str((0, 0), &truncate(header, width), WrapMode::Truncate);

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
