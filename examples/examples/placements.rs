//! Prepare visible ANSI fragments, cover them with text, move them, and remove them.

use std::io::{self, Write};
use std::sync::Arc;

use uncurses::{
    buffer::SurfaceMut,
    event::Event,
    layout::Rect,
    program::Program,
    screen::{PlacementContext, PlacementEncoder, PlacementOptions, PlacementOutput, Screen},
    style::Style,
    text::TextSurface,
};

#[derive(Debug)]
struct AsciiLabel(&'static str);

impl PlacementEncoder for AsciiLabel {
    fn prepare(&self, context: PlacementContext) -> io::Result<PlacementOutput> {
        if !self.0.is_ascii()
            || usize::from(context.area.width) != self.0.len()
            || context.area.height != 1
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the label must fit one row",
            ));
        }
        let mut draw = b"\x1b[1;31m".to_vec();
        draw.extend_from_slice(
            &self.0.as_bytes()[usize::from(context.clip.x)..usize::from(context.clip.right())],
        );
        Ok(PlacementOutput {
            draw,
            ..Default::default()
        })
    }
}

fn paint<W: Write>(
    screen: &mut Screen<W>,
    step: usize,
    options: &PlacementOptions,
) -> io::Result<()> {
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
        screen.set_placement(1, Rect::new(x, 3, 9, 1), options)?;
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
    if std::env::args().any(|argument| argument == "--commit" || argument == "--dump-commit") {
        let mut screen = Screen::new(Vec::new(), (40, 4));
        screen.set_placement(
            1,
            Rect::new(0, 0, 19, 1),
            &PlacementOptions {
                encoder: Some(Arc::new(AsciiLabel("Committed placement"))),
                ..Default::default()
            },
        )?;
        screen.commit_rows(1)?;
        screen.set_str((0, 0), "Retained frame", Style::EMPTY);
        screen.render()?;
        screen.commit_rows(screen.size().height)?;
        if std::env::args().any(|argument| argument == "--dump-commit") {
            print!("committed\t");
            for byte in screen.writer() {
                print!("{byte:02x}");
            }
            println!();
        } else {
            let mut out = io::stdout().lock();
            out.write_all(screen.writer())?;
            out.flush()?;
        }
        return Ok(());
    }
    let options = PlacementOptions {
        encoder: Some(Arc::new(AsciiLabel("PLACEMENT"))),
        ..Default::default()
    };
    if std::env::args().any(|argument| argument == "--dump") {
        let mut screen = Screen::new(Vec::new(), (40, 10));
        screen.set_fullscreen(true);
        for step in 0..4 {
            paint(&mut screen, step, &options)?;
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
        paint(program.screen_mut(), step, &options)?;
        loop {
            match program.read_event()? {
                Event::KeyPress(key) if key.matches_any(["q", "esc", "ctrl+c"]) => break,
                Event::KeyPress(key) if key.matches("space") => {
                    step = (step + 1) % 4;
                    paint(program.screen_mut(), step, &options)?;
                }
                Event::Resize(_) => {
                    program.autoresize()?;
                    paint(program.screen_mut(), step, &options)?;
                }
                _ => {}
            }
        }
        Ok(())
    })();
    result.and(program.finish())
}
