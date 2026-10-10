//! Report working, blocked, done, and clear states with OSC 7501.
//!
//! Run with `cargo run -p examples --example program_status`.
//! The example writes reports without a support query, so it also works
//! with redirected output. Supporting terminals choose how to show them.
//! Add `-- --query` to detect support through Program options first.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use uncurses::ansi::program_status::{
    ProgramStatus, ProgramStatusKind, ProgramStatusState, write_program_status,
};
use uncurses::event::Event;
use uncurses::program::{Program, ProgramOptions};

fn main() -> io::Result<()> {
    if std::env::args().any(|arg| arg == "--query") {
        println!("OSC 7501 support confirmed: {}", query_support()?);
    }
    let mut out = io::stdout().lock();
    let mut report = ProgramStatus::new(ProgramStatusState::Working);
    report.app = Some("uncurses-example");
    report.progress = Some(25);
    report.message = Some("Build started");
    write_program_status(&mut out, &report)?;
    writeln!(out, "Working: Build started")?;
    out.flush()?;

    let mut child = ProgramStatus::new(ProgramStatusState::Blocked);
    child.id = Some("build/test");
    child.title = Some("Tests");
    child.kind = Some(ProgramStatusKind::Permission);
    child.message = Some("Example approval request");
    write_program_status(&mut out, &child)?;
    writeln!(out, "Blocked child: Example approval request")?;
    out.flush()?;

    let mut clear = ProgramStatus::new(ProgramStatusState::Clear);
    clear.id = child.id;
    write_program_status(&mut out, &clear)?;
    writeln!(out, "Cleared child: build/test")?;

    report.state = ProgramStatusState::Done;
    report.progress = None;
    report.message = Some("Example complete");
    write_program_status(&mut out, &report)?;
    writeln!(out, "Done: Example complete (record survives exit)")?;
    out.flush()
}

fn query_support() -> io::Result<bool> {
    let mut program = Program::stdio()?;
    let result: io::Result<bool> = (|| {
        program.init_with(ProgramOptions {
            query_program_status: true,
            ..ProgramOptions::default()
        })?;
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
        Ok(program.capabilities().program_status())
    })();
    let finish = program.finish();
    let supported = result?;
    finish?;
    Ok(supported)
}
