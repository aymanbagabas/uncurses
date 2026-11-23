//! Measure East Asian Ambiguous width with the default capability queries.
//!
//! Run with `cargo run -p examples --example eaw_width` on a blank terminal row.
//! The probe writes U+2026, measures its cursor advance, then erases it.
//! The screen adopts the measured width for subsequent text.

use std::io;
use std::time::{Duration, Instant};

use uncurses::event::Event;
use uncurses::program::Program;

fn main() -> io::Result<()> {
    let mut program = Program::stdio()?;
    let result: io::Result<_> = (|| {
        program.init()?;
        program.query_capabilities(&[])?;
        let deadline = Instant::now() + Duration::from_millis(300);
        'wait: while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            if !program.poll_event(Some(remaining))? {
                break;
            }
            while let Some(event) = program.try_read_event()? {
                if matches!(event, Event::PrimaryDeviceAttributes(_)) {
                    break 'wait;
                }
            }
        }
        Ok(program.capabilities().eaw_width())
    })();
    let finish = program.finish();
    let width = result?;
    finish?;
    match width {
        Some(width) => println!("East Asian ambiguous width: {width} column(s)"),
        None => println!("East Asian ambiguous width: unknown"),
    }
    Ok(())
}
