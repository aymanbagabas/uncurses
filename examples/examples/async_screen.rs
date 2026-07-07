//! [`Screen`] driven by an async [`EventStream`], all on one tokio task.
//!
//! Before, mixing a rendering [`Screen`] with async input meant parking the
//! screen on its own thread and shuffling events through channels (see
//! `async_arcade`). Now [`Screen::event_stream`] hands you a real
//! `futures_core::Stream` over the screen's own decoder, so terminal input and
//! any other async work (here: a frame timer) merge in a single
//! `tokio::select!`, and the same task renders. No app-owned helper thread, no channels.
//!
//! The stream is pure: reading an event does not touch capability tracking.
//! Feed each event back through [`Screen::observe_event`] so resize handling
//! and the discovery-driven defaults (mouse, keyboard, in-band resize) still
//! apply. That one line is the whole contract.
//!
//! Requires the `async` feature (on by default for the examples crate):
//! `cargo run --example async_screen`. Press `q`, `Esc`, or `Ctrl-C` to quit.

use std::time::Duration;

use tokio_stream::StreamExt;

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::color::Color;
use uncurses::event::{Event, Key};
use uncurses::screen::{Screen, ScreenOptions};
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

/// ~60 fps: smooth, and idles cheaply because `select!` only wakes on a tick
/// or a real key, never a busy loop.
const FRAME: Duration = Duration::from_millis(16);

struct Ball {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
}

impl Ball {
    /// Advance one frame, bouncing off the walls of a `w`x`h` arena.
    fn step(&mut self, w: u16, h: u16) {
        self.x += self.vx;
        self.y += self.vy;
        if self.x <= 0.0 || self.x >= (w.saturating_sub(1)) as f32 {
            self.vx = -self.vx;
            self.x = self.x.clamp(0.0, (w.saturating_sub(1)) as f32);
        }
        if self.y <= 0.0 || self.y >= (h.saturating_sub(1)) as f32 {
            self.vy = -self.vy;
            self.y = self.y.clamp(0.0, (h.saturating_sub(1)) as f32);
        }
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init_with(ScreenOptions::default())?;
    screen.enter_alt_screen()?;
    screen.hide_cursor()?;

    let result = run(&mut screen).await;

    // Always restore the terminal, even if the loop erred.
    let finish = screen.finish();
    result.and(finish)
}

async fn run(screen: &mut Screen<Stdin, Stdout>) -> std::io::Result<()> {
    let quit_keys: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());

    // The async input stream over the screen's own decoder. Owned, so it does
    // not borrow the screen: render and observe freely while it is live.
    let mut events = screen.event_stream();
    let mut ticker = tokio::time::interval(FRAME);

    let mut ball = Ball {
        x: 4.0,
        y: 2.0,
        vx: 0.9,
        vy: 0.5,
    };
    let mut frames: u64 = 0;
    let mut last_key = String::from("(none)");

    loop {
        tokio::select! {
            // Terminal input, genuinely async: no reactor block.
            maybe = events.next() => {
                let Some(ev) = maybe else { break };
                let ev = ev?;
                // Keep capability tracking alive on the pure stream.
                screen.observe_event(&ev)?;
                match ev {
                    Event::KeyPress(ref key) if quit_keys.contains(key) => break,
                    Event::KeyPress(ref key) => last_key = key.to_string(),
                    Event::Resize(ws) => screen.resize((ws.col, ws.row)),
                    _ => {}
                }
            }
            // The frame timer, ticking concurrently with input.
            _ = ticker.tick() => {
                ball.step(screen.width(), screen.height());
                frames += 1;
            }
        }

        draw(screen, &ball, frames, &last_key);
        screen.render()?;
    }
    Ok(())
}

fn draw(screen: &mut Screen<Stdin, Stdout>, ball: &Ball, frames: u64, last_key: &str) {
    screen.clear();
    let w = screen.width();
    let h = screen.height();

    let hud = Style::default().fg(Color::BrightBlack);
    screen.set_str(
        (0, 0),
        &format!("async Screen + EventStream • frame {frames} • last key: {last_key}"),
        hud.clone(),
    );
    screen.set_str((0, h.saturating_sub(1)), "q / Esc / Ctrl-C: quit", hud);

    if w >= 1 && h >= 1 {
        let bx = ball.x.round().clamp(0.0, (w - 1) as f32) as u16;
        let by = ball.y.round().clamp(0.0, (h - 1) as f32) as u16;
        let dot = Style::default().fg(Color::BrightCyan).bold();
        screen.set_str((bx, by), "●", dot);
    }
}
