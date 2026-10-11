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
use uncurses::event::{Event, ProgramStatusCapabilities};
use uncurses::program::{Program, ProgramOptions};

fn main() -> io::Result<()> {
    if std::env::args().any(|arg| arg == "--query") {
        let caps = query_support()?;
        println!("OSC 7501 support confirmed: {}", caps.is_some());
        if let Some(caps) = caps {
            println!("Accepted states: {}", caps.states.join(", "));
            println!("Accepted kinds: {}", caps.kinds.join(", "));
        }
    }
    let mut out = io::stdout().lock();
    let mut report = ProgramStatus::new(ProgramStatusState::WORKING);
    report.app = Some("uncurses-example");
    report.progress = Some(25);
    report.message = Some("Build started");
    write_program_status(&mut out, &report)?;
    writeln!(out, "Working: Build started")?;
    out.flush()?;

    let mut child = ProgramStatus::new(ProgramStatusState::BLOCKED);
    child.id = Some("build/test");
    child.title = Some("Tests");
    child.kind = Some(ProgramStatusKind::PERMISSION);
    child.message = Some("Example approval request");
    write_program_status(&mut out, &child)?;
    writeln!(out, "Blocked child: Example approval request")?;
    out.flush()?;

    let mut clear = ProgramStatus::new(ProgramStatusState::CLEAR);
    clear.id = child.id;
    write_program_status(&mut out, &clear)?;
    writeln!(out, "Cleared child: build/test")?;

    report.state = ProgramStatusState::DONE;
    report.progress = None;
    report.message = Some("Example complete");
    write_program_status(&mut out, &report)?;
    writeln!(out, "Done: Example complete (record survives exit)")?;

    // These extension names are illustrative. Preview their bytes rather
    // than send names the terminal might not advertise.
    let state_name = String::from("paused");
    let kind_name = String::from("confirm");
    let mut extension = ProgramStatus::new(ProgramStatusState::new(&state_name));
    let mut preview = Vec::new();
    write_program_status(&mut preview, &extension)?;
    extension.state = ProgramStatusState::BLOCKED;
    extension.kind = Some(ProgramStatusKind::new(&kind_name));
    write_program_status(&mut preview, &extension)?;
    writeln!(out, "Custom-name wire preview: {}", preview.escape_ascii())?;
    out.flush()
}

fn query_support() -> io::Result<Option<ProgramStatusCapabilities>> {
    let mut program = Program::stdio()?;
    let result: io::Result<Option<ProgramStatusCapabilities>> = (|| {
        program.init_with(ProgramOptions {
            query_program_status: true,
            ..ProgramOptions::default()
        })?;
        program.query_capabilities(&[])?;

        let mut caps = None;
        let deadline = Instant::now() + Duration::from_millis(300);
        'wait: while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            if !program.poll_event(Some(remaining))? {
                break;
            }
            while let Some(event) = program.try_read_event()? {
                match event {
                    Event::ProgramStatus(report) => caps = Some(report),
                    Event::PrimaryDeviceAttributes(_) => break 'wait,
                    _ => {}
                }
            }
        }
        Ok(caps)
    })();
    let finish = program.finish();
    let caps = result?;
    finish?;
    Ok(caps)
}
