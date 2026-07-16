//! Pointer shapes: turn the mouse into a hand while hovering hyperlinks.
//!
//! Draws a few OSC 8 hyperlinks and tracks the mouse with any-event motion
//! reporting. While the pointer is over a link the mouse cursor is switched to
//! the `"pointer"` (hand) shape via `OSC 22` ([`Screen::set_pointer_shape`]);
//! moving off every link resets it to the terminal default. The shape is only
//! touched when the hover state changes, so we don't spam `OSC 22` on every
//! motion event.
//!
//! Needs a terminal that supports OSC 22 pointer shapes and OSC 8 hyperlinks
//! (kitty, foot, Ghostty, WezTerm, ...). Terminals without OSC 22 just ignore
//! the shape change.
//!
//! Run with `cargo run --example pointer_shape`. Hover the links; click opens
//! them if your terminal makes hyperlinks clickable. Press `q` or `Ctrl-C` to
//! quit.

use std::io;

use uncurses::buffer::SurfaceMut;
use uncurses::color::Color;
use uncurses::event::{Event, Key};
use uncurses::screen::{MouseTracking, Screen, ScreenOptions};
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

const LINKS: [(&str, &str); 3] = [
    (
        "uncurses on GitHub",
        "https://github.com/aymanbagabas/uncurses",
    ),
    (
        "What are OSC 8 hyperlinks?",
        "https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda",
    ),
    (
        "kitty pointer shapes",
        "https://sw.kovidgoyal.net/kitty/pointer-shapes/",
    ),
];

/// A placed, measured hyperlink and the cell row it occupies.
struct Link {
    label: &'static str,
    url: &'static str,
    x: u16,
    y: u16,
    w: u16,
}

impl Link {
    fn contains(&self, x: u16, y: u16) -> bool {
        y == self.y && x >= self.x && x < self.x + self.w
    }
}

fn main() -> io::Result<()> {
    let mut screen = Screen::stdio()?;
    // `MOTION` enables any-event tracking so we get hover moves with no button
    // held, which is what tells us when the pointer enters or leaves a link.
    screen.init_with(ScreenOptions {
        mouse: Some(MouseTracking::MOTION),
        ..ScreenOptions::default()
    })?;
    screen.enter_alt_screen()?;
    screen.hide_cursor()?;

    let result = run(&mut screen);
    // finish() resets the pointer shape as part of teardown, so a hand left
    // hovering at exit is restored for us.
    screen.finish()?;
    result
}

fn run(screen: &mut Screen<Stdin, Stdout>) -> io::Result<()> {
    let quit: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());

    // Fixed layout; measure each label with the renderer's own width policy so
    // the hit region matches what's drawn.
    let links: Vec<Link> = LINKS
        .iter()
        .enumerate()
        .map(|(i, &(label, url))| Link {
            label,
            url,
            x: 4,
            y: 3 + i as u16 * 2,
            w: screen.str_width(label),
        })
        .collect();

    let mut pointer: Option<(u16, u16)> = None;
    let mut hovering = false;
    render(screen, &links);

    loop {
        let event = screen.read_event()?;
        screen.observe_event(&event)?;
        match event {
            Event::KeyPress(ref k) if quit.contains(k) => break,
            Event::MouseMove(m) | Event::MouseClick(m) => pointer = Some((m.x, m.y)),
            Event::Resize(ws) => screen.resize((ws.col, ws.row)),
            _ => continue,
        }

        let now_hovering = pointer.is_some_and(|(x, y)| links.iter().any(|l| l.contains(x, y)));
        if now_hovering != hovering {
            hovering = now_hovering;
            if hovering {
                screen.set_pointer_shape("pointer")?;
            } else {
                screen.reset_pointer_shape()?;
            }
        }

        render(screen, &links);
    }
    Ok(())
}

fn render(screen: &mut Screen<Stdin, Stdout>, links: &[Link]) {
    screen.clear();

    let dim = Style::default().fg(Color::BrightBlack);
    screen.set_str(
        (0, 0),
        "Hover a link: the mouse becomes a hand. q quits.",
        dim,
    );

    let link_style = Style::default().fg(Color::BrightBlue).underline();
    for l in links {
        screen.set_str((l.x, l.y), l.label, link_style.clone().link(l.url, ""));
    }

    let _ = screen.render();
}
