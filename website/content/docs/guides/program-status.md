---
title: "Program status"
weight: 14
---

Use OSC 7501 to tell the terminal what your program is doing. Reports can
include progress, a message, and the reason that user input is required.
The terminal chooses how to present them. Unsupported terminals ignore them.

## Send a report

`Program::set_program_status` sends and flushes a complete report.

```rust
use uncurses::program::{Program, ProgramStatus, ProgramStatusState};

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    let result = (|| {
        program.init()?;
        let mut report = ProgramStatus::new(ProgramStatusState::Working);
        report.app = Some("my-build");
        report.progress = Some(25);
        report.message = Some("Compile libraries");
        program.set_program_status(&report)?;

        report.state = ProgramStatusState::Done;
        report.progress = None;
        report.message = Some("Build complete");
        program.set_program_status(&report)
    })();
    let finish = program.finish();
    result.and(finish)
}
```

Each report replaces its record completely. Repeat `app`, `title`, and
`message` on each update when they should remain present. Supply plain UTF-8
text; uncurses performs base64 encoding.

The writer rejects invalid identifiers, control characters, excessive text
lengths, and percentages above 100 before it writes any output. An invalid
report returns `io::ErrorKind::InvalidInput`. Rustdoc lists the exact limits.

## Report tasks and user input

An absent `id` selects the root record. Set `id` to a path such as
`build/test` for a child record. An absent `app` inherits from the nearest
ancestor that supplies one.

Use `ProgramStatusState::Blocked` when work needs user input.
`ProgramStatusKind` distinguishes `Permission`, `Question`, and `Auth`.
Only working and blocked records use `progress`. An absent percentage means
indeterminate progress.

Use `ProgramStatusState::Clear` to remove a record and its descendants.
A clear report without an ID removes all records in the terminal.

## Record lifetime

The terminal owns these records. Pause, resume, screen switches, and
`finish()` leave them unchanged. Done and error records survive process exit
and the next shell prompt so the user can find the result.

The terminal removes working and blocked records when the process exits or
the next shell prompt starts.

On cancellation, report `Idle`. Report `Done` or `Error` before exit when
you want the result to remain visible. OSC 9;4 progress is a separate API
with separate teardown behavior.

## Detect support explicitly

Detection is optional. `program.request_program_status()?` sends the query.
A supporting terminal replies with `Event::ProgramStatusSupported`.
Synchronous program reads observe the reply automatically and set
`program.capabilities().program_status()` to `true`.

For async streams, pass the reply through `program.observe_event(&event)?`.
Use a bounded wait with `poll_event` when you need a support decision.
Before a reply, the capability flag means only that support is unconfirmed.

For a query batch, set `ProgramOptions::query_program_status` to `true` in
`init_with`, then call `program.query_capabilities(&[])?`.
The option defaults to `false`. It selects the query; initialization sends
no queries. The final Primary DA reply marks the end of the batch.

The option controls discovery only. Reports and reply observation work
regardless of its value. For custom batches, the query bytes are available
as `uncurses::ansi::program_status::REQUEST_PROGRAM_STATUS`.

## Output without a session

`uncurses::ansi::program_status::write_program_status` writes to any
`std::io::Write`. It uses the same report type and leaves flushing to the
caller.

Run `cargo run -p examples --example program_status` for working, blocked,
clear, and done reports. The example also prints a description of each
report, so its output remains useful outside a supporting terminal.

Add `-- --query` to detect support through `ProgramOptions` first. This mode
requires a terminal and waits up to 300 milliseconds for replies.
It prints whether support was confirmed, then sends the same reports.

See the [OSC 7501 specification](https://www.superlogical.com/rex/docs/build/program-status)
for protocol details.
