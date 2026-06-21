//! Mouse tracking: report clicks, motion, and the scroll wheel.
//!
//! Enables mouse reporting through [`ScreenOptions`] and paints whatever
//! the pointer is doing: a marker where it last moved, the last button
//! pressed, and a running tally of wheel ticks. A small example of mixing
//! input and rendering around a single feature.
//!
//! Run with `cargo run --example mouse`. Move and click in the window;
//! press `q` or `Ctrl-C` to quit.

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::color::BasicColor;
use uncurses::event::{Event, Key, MouseButton};
use uncurses::screen::{MousePreference, Screen, ScreenOptions};
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

#[derive(Default)]
struct State {
    pointer: Option<(u16, u16)>,
    last_button: Option<MouseButton>,
    wheel: i32,
}

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    // `motion: true` asks for move events (not just press/release); the
    // screen negotiates the best mode and encoding the terminal supports.
    screen.init_with(ScreenOptions {
        mouse: Some(MousePreference {
            motion: true,
            pixels: false,
        }),
        ..ScreenOptions::default()
    })?;
    screen.enter_alt_screen()?;
    screen.hide_cursor()?;

    let result = run(&mut screen);
    screen.finish()?;
    result
}

fn run(screen: &mut Screen<Stdin, Stdout>) -> std::io::Result<()> {
    let quit: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
    let mut state = State::default();
    render(screen, &state);

    loop {
        let event = screen.read_event()?;
        match event {
            Event::KeyPress(ref k) if quit.contains(k) => break,
            Event::MouseMove(m) => state.pointer = Some((m.x, m.y)),
            Event::MouseClick(m) => {
                state.pointer = Some((m.x, m.y));
                state.last_button = Some(m.button);
            }
            Event::MouseWheel(m) => {
                state.pointer = Some((m.x, m.y));
                match m.button {
                    MouseButton::WheelUp => state.wheel += 1,
                    MouseButton::WheelDown => state.wheel -= 1,
                    _ => {}
                }
            }
            Event::Resize(ws) => screen.resize((ws.col, ws.row)),
            _ => continue,
        }
        render(screen, &state);
    }
    Ok(())
}

fn render(screen: &mut Screen<Stdin, Stdout>, state: &State) {
    screen.clear();
    let w = screen.width();
    let h = screen.height();

    let dim = Style::default().fg(BasicColor::BrightBlack);
    screen.set_str((0, 0), "Move and click. q quits.", dim);

    let info = format!(
        "button: {:<10} wheel: {:+}",
        state
            .last_button
            .map_or_else(|| "none".to_string(), |b| format!("{b:?}")),
        state.wheel,
    );
    screen.set_str((0, 1), &info, Style::default());

    if let Some((x, y)) = state.pointer
        && x < w
        && y < h
    {
        let marker = Style::default()
            .bold()
            .fg(BasicColor::Black)
            .bg(BasicColor::BrightYellow);
        screen.set_str((x, y), "▢", marker);
    }

    let _ = screen.present();
}
