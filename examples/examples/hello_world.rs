//! Minimal fullscreen session with explicit teardown on success and error.

use uncurses::event::Event;
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    let result = (|| {
        program.init()?;
        program.enter_alt_screen()?;

        let screen = program.screen_mut();
        screen.set_str((0, 0), "hello, uncurses!", Style::new());
        screen.render()?;

        while !matches!(program.read_event()?, Event::KeyPress(_)) {}
        Ok(())
    })();
    let finish = program.finish();
    result.and(finish)
}
