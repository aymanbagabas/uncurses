//! Uncapped variant of the [`ratatui_space`] example.
//!
//! Renders the animated grayscale starfield through ratatui as fast as
//! the renderer can push frames out. Input runs on a dedicated thread
//! that forwards events through a channel so the render loop never
//! blocks waiting on the keyboard. Useful for measuring raw renderer
//! throughput through the ratatui backend.
//!
//! Run with `cargo run --release --example ratatui_space_unlimited`.
//! Press `q` or `Ctrl-C` to quit.

use std::io::{self, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Stylize};
use ratatui::widgets::{Paragraph, Widget};
use uncurses::event::{Event, EventSource, Key, KeyCode, KeyModifiers};
use uncurses::screen::Screen;
use uncurses::terminal::{get_window_size, make_raw_mode, set_state, stdin, stdout};
use uncurses_ratatui::UncursesBackend;

struct Fps {
    frames: u32,
    last: Instant,
    value: Option<f32>,
    draw_ns: u128,
    stats: Option<StageStats>,
}

#[derive(Copy, Clone)]
struct StageStats {
    draw_us: f32,
}

impl Fps {
    fn new() -> Self {
        Self {
            frames: 0,
            last: Instant::now(),
            value: None,
            draw_ns: 0,
            stats: None,
        }
    }

    fn record(&mut self, draw: Duration) {
        self.frames += 1;
        self.draw_ns += draw.as_nanos();
        let elapsed = self.last.elapsed();
        if elapsed >= Duration::from_secs(1) && self.frames > 2 {
            let n = f64::from(self.frames);
            self.value = Some(self.frames as f32 / elapsed.as_secs_f32());
            self.stats = Some(StageStats {
                draw_us: (self.draw_ns as f64 / n / 1000.0) as f32,
            });
            self.frames = 0;
            self.draw_ns = 0;
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

struct StarfieldWidget<'a> {
    field: &'a Field,
    frame_count: u32,
}

impl Widget for StarfieldWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.field.width == 0 || self.field.pixel_height == 0 {
            return;
        }
        let shift = self.frame_count as usize;
        for (yi, y) in (area.top()..area.bottom()).enumerate() {
            let py = yi * 2;
            for (xi, x) in (area.left()..area.right()).enumerate() {
                let sx = (xi + shift) % self.field.width as usize;
                let fg = self.field.at(sx as u16, py as u16);
                let bg = self.field.at(sx as u16, (py + 1) as u16);
                buf[(x, y)].set_char('\u{2580}').set_fg(fg).set_bg(bg);
            }
        }
    }
}

struct StatsWidget<'a> {
    fps: &'a Fps,
}

impl Widget for StatsWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if let Some(value) = self.fps.value {
            let text = if let Some(s) = self.fps.stats {
                format!("{value:.1} fps  draw {:.0}µs", s.draw_us)
            } else {
                format!("{value:.1} fps")
            };
            Paragraph::new(text)
                .alignment(Alignment::Right)
                .render(area, buf);
        }
    }
}

enum InputMsg {
    Resize,
    Quit,
}

fn main() -> io::Result<()> {
    let stdin = stdin();
    let stdout = stdout();
    let raw_state = make_raw_mode(stdin, stdout)?;
    let result = run();
    set_state(stdin, stdout, &raw_state)?;
    result
}

fn run() -> io::Result<()> {
    let stdout = stdout();
    let size = get_window_size(stdout).unwrap_or_default();
    let mut screen = Screen::new(stdout, (size.col, size.row));
    screen.set_alt_screen(true);
    screen.set_cursor_visible(false);

    let mut terminal = Terminal::new(UncursesBackend::new(screen))?;

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
                Ok(InputMsg::Resize) => {
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
        terminal.draw(|frame| {
            let area = frame.area();
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(0)])
                .split(area);
            let header = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(0), Constraint::Length(36)])
                .split(layout[0]);

            Paragraph::new("space (unlimited) — press q to quit")
                .bold()
                .render(header[0], frame.buffer_mut());
            StatsWidget { fps: &fps }.render(header[1], frame.buffer_mut());

            let body = layout[1];
            field.ensure(body.width, body.height, &mut rng);
            StarfieldWidget {
                field: &field,
                frame_count,
            }
            .render(body, frame.buffer_mut());
        })?;
        let t1 = Instant::now();
        fps.record(t1 - t0);
    }

    let screen = terminal.backend_mut().screen_mut();
    screen.reset();
    screen.flush()?;
    drop(input_handle);
    Ok(())
}

fn input_loop(tx: mpsc::Sender<InputMsg>) {
    let stdin = stdin();
    let mut events = match EventSource::new(stdin) {
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
                Event::Resize(_) if tx.send(InputMsg::Resize).is_err() => {
                    return;
                }
                _ => {}
            },
            Err(_) => return,
        }
    }
}

fn seed_from_clock() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15)
}
