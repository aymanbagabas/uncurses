//! Query terminal colors through Program options and print the replies.
//!
//! Run with `cargo run --example query_colors`. Initialization configures the
//! options; `query_capabilities` sends the requests. The example waits up to
//! 300 milliseconds. Colors without a reply remain unknown.

use std::io;
use std::time::{Duration, Instant};

use uncurses::event::Event;
use uncurses::program::{Program, ProgramOptions};
use uncurses::terminal::{Stdin, Stdout};

fn main() -> io::Result<()> {
    let mut program = Program::stdio()?;
    program.init_with(ProgramOptions {
        query_foreground_color: true,
        query_background_color: true,
        query_cursor_color: true,
        query_palette_colors: (0..16).collect(),
        ..ProgramOptions::default()
    })?;

    let result = query_colors(&mut program);
    let caps = program.capabilities().clone();
    program.finish()?;
    result?;

    println!("foreground color: {:?}", caps.foreground_color());
    println!("background color: {:?}", caps.background_color());
    println!("cursor color: {:?}", caps.cursor_color());
    for (index, color) in caps.palette() {
        println!("palette {index}: {color:?}");
    }
    Ok(())
}

fn query_colors(program: &mut Program<Stdin, Stdout>) -> io::Result<()> {
    program.query_capabilities(&[])?;
    let deadline = Instant::now() + Duration::from_millis(300);
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        if !program.poll_event(Some(remaining))? {
            break;
        }
        while let Some(event) = program.try_read_event()? {
            if matches!(event, Event::PrimaryDeviceAttributes(_)) {
                return Ok(());
            }
        }
    }
    Ok(())
}
