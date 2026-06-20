//! Async input: drive the event loop with `.await` instead of blocking.
//!
//! With the `async` feature, [`Screen::events`] hands back a
//! [`futures_core::Stream`] of events. This is the same decode-and-react
//! loop as the blocking examples, but it `await`s the next event, so it
//! drops into any async runtime (tokio here) and leaves room for other
//! tasks, timers, or I/O between keystrokes.
//!
//! Run with `cargo run --example async_input`. Type to echo keys; press
//! `q` or `Ctrl-C` to quit.

use tokio_stream::StreamExt;

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::color::BasicColor;
use uncurses::event::{Event, Key};
use uncurses::screen::{Screen, ScreenOptions};
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init_with(ScreenOptions::default())?;
    screen.enter_alt_screen()?;
    screen.hide_cursor()?;

    let result = run(&mut screen).await;
    screen.finish()?;
    result
}

async fn run(screen: &mut Screen<Stdin, Stdout>) -> std::io::Result<()> {
    let quit: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
    let mut typed = String::new();
    render(screen, &typed);

    // `events()` borrows the screen only for one `next().await`; in edition
    // 2024 the temporary drops before the loop body, so the body is free to
    // draw through `screen` again.
    while let Some(event) = screen.events().next().await {
        match event? {
            Event::KeyPress(ref k) if quit.contains(k) => break,
            Event::KeyPress(Key {
                code: uncurses::event::KeyCode::Char(c),
                ..
            }) => typed.push(c),
            Event::KeyPress(Key {
                code: uncurses::event::KeyCode::Backspace,
                ..
            }) => {
                typed.pop();
            }
            Event::Resize(ws) => screen.resize((ws.col, ws.row)),
            _ => continue,
        }
        render(screen, &typed);
    }
    Ok(())
}

fn render(screen: &mut Screen<Stdin, Stdout>, typed: &str) {
    screen.clear();
    let dim = Style::default().fg(BasicColor::BrightBlack.into());
    screen.set_str((0, 0), "Async echo. Type away; q quits.", dim);

    let text = Style::default().fg(BasicColor::BrightGreen.into());
    let h = screen.height();
    screen.set_str((0, h / 2), typed, text);

    let _ = screen.present();
}
