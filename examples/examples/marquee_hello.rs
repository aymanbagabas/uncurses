//! Marquee "Hello, World!" on the middle-left of the screen.
//!
//! Enters the alternate screen and slides a single "Hello, World!" across the
//! left half of the screen at the vertical midpoint. The terminal cursor is
//! parked on the line just below the marquee at column 0. Pressing *any* key
//! quits.
//!
//! Run with `cargo run --example marquee_hello`.

use std::time::Duration;

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::color::Color;
use uncurses::event::Event;
use uncurses::program::{Program, ProgramOptions};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

/// The marquee text that scrolls through the window. The trailing gap keeps
/// the text readable as it wraps around.
const TEXT: &str = "Hello, World! ";
/// One animation step per frame.
const FRAME_TIME: Duration = Duration::from_millis(120);

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    program.init_with(ProgramOptions::default())?;
    program.enter_alt_screen()?;
    // The cursor stays visible: it is the resting point below the marquee.
    program.show_cursor()?;

    let result = run(&mut program);

    program.finish()?;
    result
}

fn run(program: &mut Program<Stdin, Stdout>) -> std::io::Result<()> {
    let glyphs: Vec<char> = TEXT.chars().collect();
    let mut offset: usize = 0;
    program.set_title("Marquee Hello")?;

    loop {
        let cursor_row = draw(program.screen_mut(), &glyphs, offset);

        // Park the cursor below the marquee at column 0 after every frame.
        // The diff render would otherwise leave the physical cursor at the end
        // of the marquee write; staging it here lets render() place it as part
        // of the frame.
        program.screen_mut().set_cursor_position((0, cursor_row));
        program.screen_mut().render()?;

        // Block up to one frame for input; any key press exits.
        if program.poll_event(Some(FRAME_TIME))? {
            let ev = program.read_event()?;
            program.observe_event(&ev)?;
            match ev {
                Event::KeyPress(_) => break,
                Event::Resize(ws) => program.screen_mut().resize((ws.col, ws.row)),
                _ => {}
            }
        }

        offset = (offset + 1) % glyphs.len();
    }

    Ok(())
}

fn draw(screen: &mut Screen<Stdout>, glyphs: &[char], offset: usize) -> u16 {
    screen.clear();

    let w = screen.width();
    let h = screen.height();
    if w == 0 || h == 0 {
        return 0;
    }

    // The marquee window is exactly the width of the text; characters scroll
    // through it like a ticker. Anchored to the left at the vertical midpoint.
    let n = glyphs.len();
    let window_w = (n as u16).min(w) as usize;
    let row = h / 2;

    let line: String = (0..window_w)
        .map(|col| glyphs[(offset + col) % n])
        .collect();

    let style = Style::default().fg(Color::BrightCyan).bold();
    screen.set_str((0, row), &line, style);

    // The cursor belongs on the line directly below the marquee, column 0.
    (row + 1).min(h.saturating_sub(1))
}
