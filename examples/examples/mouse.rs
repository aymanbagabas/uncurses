//! Mouse tracking: report clicks, motion, and the scroll wheel.
//!
//! Enables mouse reporting through [`ProgramOptions`] and paints whatever
//! the pointer is doing: a marker where it last moved, the last button
//! pressed, and a running tally of wheel ticks. A small example of mixing
//! input and rendering around a single feature.
//!
//! Run with `cargo run --example mouse`. Move and click in the window;
//! press `q` or `Ctrl-C` to quit.

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::color::Color;
use uncurses::event::{Event, Key, MouseButton};
use uncurses::program::{MouseTracking, Program, ProgramOptions};
use uncurses::screen::Screen;
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
    let mut program = Program::stdio()?;
    // `motion: true` asks for move events (not just press/release); the
    // screen negotiates the best mode and encoding the terminal supports.
    program.init_with(ProgramOptions {
        mouse: Some(MouseTracking::MOTION),
        ..ProgramOptions::default()
    })?;
    program.enter_alt_screen()?;
    program.hide_cursor()?;

    let result = run(&mut program);
    program.finish()?;
    result
}

fn run(program: &mut Program<Stdin, Stdout>) -> std::io::Result<()> {
    let quit: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
    let mut state = State::default();
    render(program.screen_mut(), &state);

    loop {
        let event = program.read_event()?;
        program.observe_event(&event)?;
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
            Event::Resize(ws) => program.screen_mut().resize((ws.col, ws.row)),
            _ => continue,
        }
        render(program.screen_mut(), &state);
    }
    Ok(())
}

fn render(screen: &mut Screen<Stdout>, state: &State) {
    screen.clear();
    let w = screen.width();
    let h = screen.height();

    let dim = Style::default().fg(Color::BrightBlack);
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
            .fg(Color::Black)
            .bg(Color::BrightYellow);
        screen.set_str((x, y), "▢", marker);
    }

    let _ = screen.render();
}
