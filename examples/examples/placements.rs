//! Stage opaque ANSI bytes, cover them with text, move them, and remove them.

use std::io::{self, Write};

use uncurses::{
    buffer::SurfaceMut,
    event::Event,
    layout::Rect,
    program::Program,
    screen::{PlacementOptions, Screen},
    style::Style,
    text::TextSurface,
};

fn paint<W: Write>(screen: &mut Screen<W>, step: usize) -> io::Result<()> {
    screen.clear();
    screen.set_str((0, 0), "ANSI placements", Style::default().bold());
    screen.set_str((0, 1), "Space: next stage. Q: quit.", Style::EMPTY);
    let size = screen.size();
    if size.width < 24 || size.height < 7 {
        screen.remove_placement(1);
        screen.set_str((0, 2), "Resize to at least 24x7.", Style::EMPTY);
        return screen.render();
    }
    for y in 2..size.height - 1 {
        screen.set_str((0, y), &".".repeat(usize::from(size.width)), Style::EMPTY);
    }
    if step < 3 {
        let x = if step == 2 { 12 } else { 2 };
        let options = PlacementOptions {
            draw: b"\x1b[1;31mPLACEMENT".to_vec(),
            erase: b"\x1b[9X".to_vec(),
            ..Default::default()
        };
        screen.set_placement(1, Rect::new(x, 3, 9, 1), &options)?;
        if step > 0 {
            screen.set_str((x + 3, 3), "TOP", Style::EMPTY);
        }
    } else {
        screen.remove_placement(1);
    }
    screen.set_str(
        (0, size.height - 1),
        ["Original", "Overlay", "Moved", "Removed"][step],
        Style::EMPTY,
    );
    screen.set_cursor_position((0, size.height - 1));
    screen.render()
}

fn main() -> io::Result<()> {
    if std::env::args().any(|argument| argument == "--dump") {
        let mut screen = Screen::new(Vec::new(), (40, 10));
        screen.set_fullscreen(true);
        for step in 0..4 {
            paint(&mut screen, step)?;
            print!("{step}\t");
            for byte in screen.writer_mut().drain(..) {
                print!("{byte:02x}");
            }
            println!();
        }
        return Ok(());
    }
    let mut program = Program::stdio()?;
    let result = (|| {
        program.init()?;
        program.enter_alt_screen()?;
        let mut step = 0;
        paint(program.screen_mut(), step)?;
        loop {
            match program.read_event()? {
                Event::KeyPress(key) if key.matches_any(["q", "esc", "ctrl+c"]) => break,
                Event::KeyPress(key) if key.matches("space") => {
                    step = (step + 1) % 4;
                    paint(program.screen_mut(), step)?;
                }
                Event::Resize(_) => {
                    program.autoresize()?;
                    paint(program.screen_mut(), step)?;
                }
                _ => {}
            }
        }
        Ok(())
    })();
    result.and(program.finish())
}
