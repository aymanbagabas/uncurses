//! Pointer shapes: turn the mouse into a hand while hovering hyperlinks.
//!
//! Draws a few OSC 8 hyperlinks and tracks the mouse with any-event motion
//! reporting. While the pointer is over a link the mouse cursor is switched to
//! the `"pointer"` (hand) shape via `OSC 22` ([`Program::set_pointer_shape`]);
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
use uncurses::program::{MouseTracking, Program, ProgramOptions};
use uncurses::screen::Screen;
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
    let mut program = Program::stdio()?;
    // `MOTION` enables any-event tracking so we get hover moves with no button
    // held, which is what tells us when the pointer enters or leaves a link.
    program.init_with(ProgramOptions {
        mouse: Some(MouseTracking::MOTION),
        ..ProgramOptions::default()
    })?;
    program.enter_alt_screen()?;
    program.hide_cursor()?;

    let result = run(&mut program);
    // finish() resets the pointer shape as part of teardown, so a hand left
    // hovering at exit is restored for us.
    program.finish()?;
    result
}

fn run(program: &mut Program<Stdin, Stdout>) -> io::Result<()> {
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
            w: program.screen_mut().str_width(label),
        })
        .collect();

    let mut pointer: Option<(u16, u16)> = None;
    let mut hovering = false;
    render(program.screen_mut(), &links);

    loop {
        let event = program.read_event()?;
        match event {
            Event::KeyPress(ref k) if quit.contains(k) => break,
            Event::MouseMove(m) | Event::MouseClick(m) => pointer = Some((m.x, m.y)),
            Event::Resize(ws) => program.screen_mut().resize((ws.col, ws.row)),
            _ => continue,
        }

        let now_hovering = pointer.is_some_and(|(x, y)| links.iter().any(|l| l.contains(x, y)));
        if now_hovering != hovering {
            hovering = now_hovering;
            if hovering {
                program.set_pointer_shape("pointer")?;
            } else {
                program.reset_pointer_shape()?;
            }
        }

        render(program.screen_mut(), &links);
    }
    Ok(())
}

fn render(screen: &mut Screen<Stdout>, links: &[Link]) {
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
