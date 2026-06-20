//! Truecolor gradient: 24-bit color that degrades gracefully.
//!
//! Fills the window with a smooth hue sweep, one true-color background per
//! column. You write [`Color::Rgb`] values and the renderer downsamples
//! them to whatever the terminal's color [`Profile`](uncurses::color::Profile)
//! allows: exact on a true-color terminal, quantized to 256 or 16 colors
//! elsewhere, dropped entirely with no color. Same code, every terminal.
//!
//! Run with `cargo run --example gradient`. Resize to watch it reflow;
//! press `q` or `Ctrl-C` to quit.

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::cell::Cell;
use uncurses::color::{BasicColor, Color};
use uncurses::event::{Event, Key};
use uncurses::layout::Rect;
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init()?;
    screen.enter_alt_screen()?;
    screen.hide_cursor()?;

    let result = run(&mut screen);
    screen.finish()?;
    result
}

fn run(screen: &mut Screen<Stdin, Stdout>) -> std::io::Result<()> {
    let quit: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
    render(screen);

    loop {
        match screen.read_event()? {
            Event::KeyPress(ref k) if quit.contains(k) => break,
            Event::Resize(ws) => {
                screen.resize((ws.col, ws.row));
                render(screen);
            }
            _ => {}
        }
    }
    Ok(())
}

fn render(screen: &mut Screen<Stdin, Stdout>) {
    let w = screen.width();
    let h = screen.height();
    if w == 0 || h == 0 {
        return;
    }

    // One full-height bar per column, its background swept across the hue
    // wheel. The blank glyph carries only the background color.
    for x in 0..w {
        let hue = x as f32 / w as f32 * 360.0;
        let (r, g, b) = hsv_to_rgb(hue, 0.85, 1.0);
        let cell = Cell::narrow(" ").style(Style::default().bg(Color::Rgb(r, g, b)));
        screen.fill_rect(Rect::new(x, 0, 1, h), &cell);
    }

    let label = Style::default()
        .bold()
        .fg(BasicColor::Black.into())
        .bg(BasicColor::BrightWhite.into());
    screen.set_str((2, 1), " 24-bit gradient — q to quit ", label);

    let _ = screen.present();
}

/// Minimal HSV-to-RGB for `s`, `v` in `0.0..=1.0` and `h` in degrees.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let hp = (h / 60.0).rem_euclid(6.0);
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to = |f: f32| ((f + m) * 255.0).round() as u8;
    (to(r), to(g), to(b))
}
