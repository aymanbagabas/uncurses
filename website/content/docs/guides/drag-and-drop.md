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

Process replies in the normal event loop.
The trailing primary device attributes reply marks the end of the query batch.
`program.capabilities().drag_drop()` becomes `true` after a support reply.
On `Event::PrimaryDeviceAttributes`, check that value and enable the desired roles once.
Continue handling keyboard input, resize events, and rendering while the query is pending.
See [Querying the terminal]({{< relref "querying-the-terminal.md" >}}) for query details.

Synchronous program reads observe replies automatically.
For async reads, call `program.observe_event(&event)?`.

## Use the controller

When the primary device attributes event confirms the end of the query batch,
register the roles your application needs if support was confirmed:

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

### Identify the application host

Use `MachineId::current()` to derive the identity from the operating system:

```rust
use uncurses_dragdrop::{DragDrop, DragDropOptions, MachineId};

fn controller_with_identity() -> std::io::Result<DragDrop> {
    Ok(DragDrop::new(DragDropOptions {
        machine_id: Some(MachineId::current()?),
        ..DragDropOptions::default()
    }))
}
```

The method applies the protocol's HMAC-SHA256 hash before it returns the identity.
Over SSH, it identifies the remote application host.
The terminal compares this identity with its own to determine whether files require remote transfer.
The method returns an error if the operating system's source is unavailable or empty.
See [`MachineId::current`](/api/uncurses_dragdrop/struct.MachineId.html#method.current) for each platform's source.
Set this option explicitly for programs that transfer files across hosts.
The same code works over SSH and with a local terminal.

Print the identity with:

```sh
cargo run -p examples --example machine_id
```

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

A new drag can replace an unfinished drop.
`handle_event` emits `Event::DropCanceled` before the new hover or drop notification.
Discard partial destination data, queued entries, and stored directory handles before processing the next notification.
The terminal releases the replaced drop's resources.
Keep this cleanup local because the controller already tracks the new offer.

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

For large transfers, write directly to the screen's underlying writer.
Flush staged screen output first to preserve output order:

```rust
use std::io::{Read, Write};
use uncurses::program::Program;
use uncurses::terminal::{Stdin, Stdout};
use uncurses_dragdrop::{DragDrop, Entry};

fn send_text(
    program: &mut Program<Stdin, Stdout>,
    controller: &mut DragDrop,
    source: &mut impl Read,
) -> Result<(), Box<dyn std::error::Error>> {
    let screen = program.screen_mut();
    screen.flush()?;
    controller.send_from(screen.writer_mut(), Entry::Text, source)?;
    screen.writer_mut().flush()?;
    Ok(())
}
```

The caller supplies a source after an offer or data request permits the send.
This avoids retaining the complete body in the screen's output buffer.
It remains a synchronous transfer.

### Receive files and directories

Register `text/uri-list` to receive file drops.
Accept a supported operation during hover, then call `list(out)` after `Drop`.
An empty MIME slice in `accept` preserves the current MIME selection.

`UriList { remote }` identifies whether the terminal considers the files remote.
Each `Listed { entry, name }` supplies a requestable entry and the original URI bytes.
The controller skips URI comments and empty lines.
Use `request(out, Entry::UriList)` instead when your application needs the raw URI list.

Keep a queue of listed entries.
After `DataEnd`, request one entry from that queue.
The response starts with `Entry(kind)`:

| Kind | Response |
| --- | --- |
| `EntryKind::File` | `Data` chunks contain file bytes |
| `EntryKind::Symlink` | `Data` chunks contain the link target |
| `EntryKind::Directory { handle }` | `Listed` events contain child names and entries |

Preserve directory names as bytes until your application chooses a filesystem policy.
Append directory children to the queue and request them one at a time.
Release each directory handle with `close_directory` after its children finish.
Call `complete_drop` after all required data reaches its destination.
Cancel the drop if a request or destination fails.

`Entry::UriEntry(index)` selects a root URI.
`Entry::DirectoryEntry { handle, index }` selects a listed child.
Both indices start at one.
Prefer the `Entry` supplied by `Listed` over constructing indices yourself.
`Entry::Text` and `Entry::Custom(mime)` select other MIME bodies.
Existing MIME strings also work with `request`, `send`, and `send_from`.

Your application controls storage, byte limits, and path validation.
Treat incoming names, URIs, and symlink targets as untrusted data.
Keep writes inside the chosen destination and decide whether to create symlinks.
The controller transfers bytes; your application performs filesystem operations.

### Offer files and directory trees

On `DragRequested`, call `offer_files(out, uris, operations)`.
Supply file URIs and end directory URIs with `/`.
This method offers `text/uri-list` and supplies its initial data.
You can also use `offer` followed by `send(out, Entry::UriList, bytes)`.
Add any thumbnails, then call `start_drag`.

For files on another host, `EntryRequested { entry }` selects a root URI.
Answer with `send` or `send_from` for a file.
Use `send_symlink` for the link target instead of following the link.
Use `send_directory` for a directory's child names.
Its returned handle identifies that directory in subsequent child sends.

Send children in breadth-first order.
Complete the entire tree before answering the next root request.
The terminal requests roots; the application sends every descendant.
For example, after a directory root request:

```rust
use std::io::Write;
use uncurses_dragdrop::{DragDrop, Entry};

fn send_tree(
    controller: &mut DragDrop,
    out: &mut impl Write,
    root: Entry,
) -> Result<(), Box<dyn std::error::Error>> {
    let handle = controller.send_directory(out, root, &[b"hello.txt", b"link"])?;
    controller.send(
        out,
        Entry::DirectoryEntry { handle, index: 1 },
        b"Hello!\n",
    )?;
    controller.send_symlink(
        out,
        Entry::DirectoryEntry { handle, index: 2 },
        b"hello.txt",
    )?;
    out.flush()?;
    Ok(())
}
```

Use `fail_drag(out, error)` for a fatal remote file error.
Use `fail_data` for a failed MIME request.
If a source read fails, report failure or cancel the drag before further sends.
If protocol output fails, restore the terminal and discard the controller.

### Add drag thumbnails

Call `add_thumbnail` with bytes, or `add_thumbnail_from` with a reader, before `start_drag`.
Both methods return a zero-based image index.
The first image is the initial thumbnail.
Use `select_thumbnail(out, Some(index))` to change it.
Use `select_thumbnail(out, None)` to remove it while the drag continues.

```rust
use std::io::Write;
use uncurses_dragdrop::{DragDrop, ThumbnailFormat};

fn add_preview(
    controller: &mut DragDrop,
    out: &mut impl Write,
) -> Result<u32, Box<dyn std::error::Error>> {
    let index = controller.add_thumbnail(
        out,
        ThumbnailFormat::Rgba { width: 16, height: 16 },
        &[0x88; 16 * 16 * 4],
    )?;
    Ok(index)
}
```

`ThumbnailFormat` supports RGB, RGBA, PNG, and text.
Supply image dimensions, or a text scale and opacity.
Raw RGB and RGBA byte counts must match their dimensions.
See the [thumbnail reference](/api/uncurses_dragdrop/enum.ThumbnailFormat.html) for format details.

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
Report a move only after the destination accepts all data and your application completes the required source handling.

### Run the interactive examples

Run the text example for both roles, a bounded preview, and exit cleanup:

```sh
cargo run -p examples --example drag_drop_high_level
```

Press 1 for an image thumbnail, 2 for text, or 0 for none.
The selection applies to the next drag.
Press q or Ctrl-C to exit.

Run the file example with source files or directories:

```sh
cargo run -p examples --example drag_drop_files -- Cargo.toml uncurses-dragdrop/src
```

Drop files onto its window to inspect listings and count received bytes.
The example consumes incoming data without saving files.
Use Kitty's drag gesture to copy the supplied paths out.
The example sends complete directory trees and preserves symlink targets.
It uses the same thumbnail keys as the text example.
Run without source arguments to receive drops only.

For SSH, start the example on the application host from a local Kitty terminal:

```sh
ssh -t user@host 'cd /path/to/uncurses && cargo run -p examples --example drag_drop_files -- Cargo.toml uncurses-dragdrop/src'
```

Replace the host and checkout path with your own.
The checkout and source paths must exist on the application host.
The file example configures `MachineId::current()` explicitly.
Kitty compares host identities and requests file contents when they differ.
Use a terminal connection that passes OSC 72 messages in both directions.
Multiplexers can require additional passthrough configuration.

Run the scripted file exchange without a terminal:

```sh
cargo run -p examples --example drag_drop_files -- --scripted
```

This mode uses in-memory sources to demonstrate protocol events and all four thumbnail formats.

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
