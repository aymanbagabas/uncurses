---
title: "Drag and drop"
weight: 12
---

Use Kitty's [OSC 72 protocol](https://sw.kovidgoyal.net/kitty/dnd-protocol/)
to receive desktop drops or offer drags from your application.
Kitty added the protocol in version 0.47.

Use `uncurses-dragdrop` for typed events and streamed data.
Use the raw uncurses interface when your application needs direct protocol control.
Your application selects MIME types, handles data, and completes each exchange.

## Detect support

Pass `REQUEST_DRAG_DROP` to the existing capability query:

```rust
use uncurses::ansi::drag_drop::REQUEST_DRAG_DROP;
use uncurses::program::Program;
use uncurses::terminal::{Stdin, Stdout};

fn request_support(program: &mut Program<Stdin, Stdout>) -> std::io::Result<()> {
    program.query_capabilities(REQUEST_DRAG_DROP)
}
```

Read replies with a bounded wait, as shown in
[Querying the terminal]({{< relref "querying-the-terminal.md" >}}).
The trailing primary device attributes reply marks the end of the query batch.
`program.capabilities().drag_drop()` becomes `true` after a support reply.
Until then, support remains unconfirmed.

Synchronous program reads observe replies automatically.
For async reads, call `program.observe_event(&event)?`.

## Use the controller

Create `DragDrop` after the capability probe confirms support.
Register the roles your application needs:

```rust
use std::io::Write;
use uncurses_dragdrop::{DragDrop, DragDropOptions};

fn register(out: &mut impl Write) -> Result<DragDrop, Box<dyn std::error::Error>> {
    let mut controller = DragDrop::new(DragDropOptions::default());
    controller.enable_drops(out, &["text/plain"])?;
    controller.enable_drags(out)?;
    out.flush()?;
    Ok(controller)
}
```

Default options leave `routing_id` and `machine_id` unset.
Configure these identities only when your environment needs them.
See the [API reference](/api/uncurses_dragdrop/) for their formats and routing rules.

### Receive text copies

Pass terminal events to `handle_event`.
Accept a supported offer, then request data after `Event::Drop`.
The controller tracks MIME indices and decodes data across message boundaries.

```rust
use std::io::Write;
use uncurses::event::Event as TerminalEvent;
use uncurses_dragdrop::{DragDrop, Event, Operation, Operations};

fn receive_text(
    controller: &mut DragDrop,
    event: &TerminalEvent,
    out: &mut impl Write,
    destination: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    for event in controller.handle_event(event)? {
        match event {
            Event::Hover(offer)
                if !offer.operations.contains(Operations::COPY)
                    || !offer.mime_types.iter().any(|mime| mime == "text/plain") =>
            {
                controller.reject(out)?;
            }
            Event::Drop(offer)
                if !offer.operations.contains(Operations::COPY)
                    || !offer.mime_types.iter().any(|mime| mime == "text/plain") =>
            {
                controller.cancel_drop(out)?;
            }
            Event::Hover(_) => controller.accept(out, &["text/plain"], Operation::Copy)?,
            Event::Drop(_) => controller.request(out, "text/plain")?,
            Event::Data(bytes) => destination.write_all(&bytes)?,
            Event::DataEnd => {
                destination.flush()?;
                controller.complete_drop(out, Operation::Copy)?;
            }
            Event::DropFailed(error) => return Err(format!("{error}").into()),
            _ => {}
        }
    }
    out.flush()?;
    Ok(())
}
```

Keep one receive request active at a time.
Send each `Data` chunk to a destination or a bounded preview.
`DataEnd` marks transfer completion, but your application must still confirm the result.
Call `complete_drop` only after the destination accepts the data.
Call `cancel_drop` if your application cannot finish the copy.

### Offer text

On `DragRequested`, offer the MIME types and supply initial data.
Call `start_drag` when the offer is ready.
Answer later data requests with the same content:

```rust
use std::io::{Cursor, Write};
use uncurses_dragdrop::{DragDrop, Event, Operations};

fn offer_text(
    controller: &mut DragDrop,
    event: Event,
    out: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    const TEXT: &[u8] = b"Hello from uncurses!\n";
    match event {
        Event::DragRequested(_) => {
            controller.offer(out, &["text/plain"], Operations::COPY)?;
            controller.send(out, "text/plain", TEXT)?;
            controller.start_drag(out)?;
        }
        Event::DataRequested { mime_type } => {
            controller.send_from(out, &mime_type, &mut Cursor::new(TEXT))?;
        }
        Event::DragFailed(error) => return Err(format!("{error}").into()),
        _ => {}
    }
    out.flush()?;
    Ok(())
}
```

Both `send` and `send_from` send a complete body and its end marker.
`send_from` reads synchronously with bounded buffers.
Output methods do not flush, so flush responses before the next input wait.
Dispatch the events from each `handle_event` call to both roles if your application supports both.

### Close the controller

Call `close`, then flush before `Program::pause` or `Program::finish`.
Register again after resume.
Attempt controller cleanup and terminal restoration even if an earlier operation fails:

```rust
use std::io::Write;
use uncurses::program::Program;
use uncurses::terminal::{Stdin, Stdout};
use uncurses_dragdrop::DragDrop;

fn finish(
    mut program: Program<Stdin, Stdout>,
    controller: &mut DragDrop,
) -> Result<(), Box<dyn std::error::Error>> {
    let cleanup = controller.close(program.screen_mut()).map_err(Into::into);
    let flush = program.screen_mut().flush().map_err(Into::into);
    let finish = program.finish().map_err(Into::into);
    cleanup.and(flush).and(finish)
}
```

Your application owns file access and copy or move policy.
Remote file requests and remote URI responses return `Error::UnsupportedRemoteFiles`.
Cancel the exchange and clean up when this error occurs.

Run the complete example for both roles, a bounded preview, and exit cleanup:

```sh
cargo run -p examples --example drag_drop_high_level
```

See the [controller reference](/api/uncurses_dragdrop/) for all events, methods, and errors.

## Use raw messages

The raw interface exposes `Event::DragDrop` and a message writer.
Your application owns the protocol state, MIME indices, and data encoding.
The following sections describe that path.

### Send messages

`write_drag_drop` writes one message without flushing.
Options use `:` separators. Supply wire-ready payloads, including base64 for binary data.
Chunk encoded payloads to keep each message within the protocol's 4,096-byte limit.

```rust
use std::io::{self, Write};
use uncurses::ansi::drag_drop::write_drag_drop;
use uncurses::program::Program;
use uncurses::terminal::{Stdin, Stdout};

fn register_drops(program: &mut Program<Stdin, Stdout>) -> io::Result<()> {
    write_drag_drop(program.screen_mut(), &["t=a"], b"text/plain")?;
    program.screen_mut().flush()
}
```

The application supplies escape-safe metadata and payloads.
Flush protocol responses immediately rather than waiting for the next frame.

### Receive a drop

After registration, motion events report the pointer position, allowed operations,
and offered MIME types. Reply with acceptance or rejection.
Registration alone does not accept a particular drop.

| Incoming type | Application action |
| --- | --- |
| `t=m` | Select a supported MIME type and an allowed operation |
| `t=m` with `x=-1:y=-1` | Clear the current offer |
| `t=M` | Request data using its one-based MIME index |
| `t=r` | Process the requested data |
| `t=R` | Handle the reported error |

An omitted MIME list in a motion event means unchanged.
The actual drop supplies the complete list.
Request data only after that drop.

The following wire examples accept a copy, request the first MIME entry, and
complete the copy. Send them at the corresponding stages of the exchange:

```rust
use uncurses::ansi::drag_drop::write_drag_drop;

fn main() -> std::io::Result<()> {
    let mut wire = Vec::new();
    write_drag_drop(&mut wire, &["t=m", "o=1"], b"text/plain")?;
    write_drag_drop(&mut wire, &["t=r", "x=1"], b"")?;
    write_drag_drop(&mut wire, &["t=r", "o=1"], b"")?;
    Ok(())
}
```

Use `t=m:o=0` to reject a potential drop.
Use `t=r:o=0` to cancel an active drop.
Report the operation your application actually performed.

### Process chunks

Each `Event::DragDrop { options, payload }` represents one complete OSC message.
The payload remains unchanged, including any base64 encoding.
The library handles messages split across input reads.
Your application handles transfers split across messages.

For a chunked message, retain the first chunk's metadata.
Later chunks can contain only `m` and `i`.
Query messages can arrive between chunks.
Preserve request indices and directory handles when responses alternate between requests.

`m=1` means more chunks follow.
`m=0` ends a message.
An **empty final data response** ends the transfer.
Do not treat every nonempty `m=0` response as transfer completion.

Decode base64 incrementally and accept optional padding on the final chunk.
Keep only incomplete base64 groups between chunks.
Stream large contents to the destination instead of collecting the whole transfer.

The event source uses a bounded 8 KiB buffer for individual sequences.
OSC bodies wait for their terminator across input reads and pauses.
The separate string idle timeout defaults to two seconds and resets on input.
On expiry, the source reports the incomplete sequence as `Event::Unknown`
and resumes normal input decoding.
The existing input buffer limit and overflow rules apply.

### Offer a drag

Register with `t=o:x=1`.
After a `t=o` gesture event, send the offered MIME list and allowed operations.
Supply initial data for common types, such as `text/plain`, before starting the drag.
Outbound MIME indices start at zero.

Send initial data with `t=p:x=0`.
Negative indices select thumbnails.
Start the prepared drag with `t=P:x=-1`.
Handle `t=e` status changes and data requests, and `t=E` results or errors.
The protocol specification documents thumbnail formats and remote entries.

The `drag_drop` example offers text with an RGBA thumbnail and answers later data requests:

```sh
cargo run -p examples --example drag_drop
```

It also receives text copies and displays an escaped preview.
It registers both roles, streams incoming data, and cancels pending operations on exit.

### Clean up and handle files

Complete or cancel active exchanges before unregistering.
Send `t=A` to disable drops and `t=o:x=2` to disable drag offers.
Flush these messages before `program.pause()` or `program.finish()`.
Register again after resume.
Your application owns registrations sent through the raw writer.

Files in a dropped URI list can have a temporary lifetime.
Copy promised files before completing the drop if you need continued access.
Your application owns file access and copy/move policy.

Supply a protocol-formatted machine ID to enable remote file handling.
The `i` metadata key supports reply routing through compatible multiplexers.
Output passthrough alone does not establish multiplexer support.
