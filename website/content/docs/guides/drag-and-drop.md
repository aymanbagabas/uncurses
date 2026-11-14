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
Use a deadline and limit each `Program::poll_event` wait to the time that remains.
If the deadline expires, report unconfirmed support, keep both roles disabled, and restore the terminal with `Program::finish`.
See [Querying the terminal]({{< relref "querying-the-terminal.md" >}}) for query details.

Synchronous program reads observe replies automatically.
For async reads, call `program.observe_event(&event)?`.

## Use the controller

When the primary device attributes event confirms the end of the query batch,
register the roles your application needs if support was confirmed.
Construct the controller in the caller so it remains available after registration errors:

```rust
use std::io::Write;
use uncurses_dragdrop::DragDrop;

fn register(
    controller: &mut DragDrop,
    out: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    controller.enable_drops(out, &["text/plain"])?;
    controller.enable_drags(out)?;
    out.flush()?;
    Ok(())
}
```

On every exit path, attempt `close`, flush the cleanup output, and restore the terminal.
This includes failures from registration and its final flush.

Default options leave `routing_id` and `machine_id` unset.
Configure these identities only when your environment needs them.
See the [API reference](/api/uncurses_dragdrop/) for their formats and routing rules.

The default limits bound control payloads, retained names, entries per drop, and open directories.
Set a limit field to `None` for unlimited operation.
Fields omitted with `..DragDropOptions::default()` retain their default values.
The [options reference](/api/uncurses_dragdrop/struct.DragDropOptions.html) lists each default.

The bidirectional example also uses these options for filesystem resources.
It bounds queued destination paths with `max_path_bytes` and retained local handles with `max_directories`.
Exceeding a filesystem limit cancels the affected transfer, removes partial output, and keeps the example available.
Incoming controller limits cancel both roles because the error does not identify the affected exchange.

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

Decode each terminal event once with `handle_event`.
Pass each returned event to the receive and offer handlers.
Accept a supported offer, then request data after `Event::Drop`.
The controller tracks MIME indices and decodes data across message boundaries.

```rust
use std::io::Write;
use uncurses_dragdrop::{DragDrop, Event, Operation, Operations};

fn receive_text(
    controller: &mut DragDrop,
    event: &Event,
    out: &mut impl Write,
    destination: &mut Vec<u8>,
) -> Result<(), Box<dyn std::error::Error>> {
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
        Event::Drop(_) => {
            destination.clear();
            controller.request(out, "text/plain")?;
        }
        Event::Data(bytes) => {
            if bytes.len() > (1024 * 1024usize).saturating_sub(destination.len()) {
                destination.clear();
                controller.cancel_drop(out)?;
                out.flush()?;
                return Err("text drop exceeds 1 MiB".into());
            }
            destination.extend_from_slice(bytes);
        }
        Event::DataEnd => {
            controller.complete_drop(out, Operation::Copy)?;
        }
        Event::DropCanceled => destination.clear(),
        Event::DropFailed(error) => {
            destination.clear();
            return Err(error.to_string().into());
        }
        _ => {}
    }
    Ok(())
}
```

Keep one receive request active at a time.
This helper collects up to 1 MiB and clears partial bytes when a drop is canceled or fails.
For larger content, stream into a temporary file and remove it on cancellation or failure.
`DataEnd` ends one request, including a URI or directory listing.
Finish every desired request and copy promised files before confirming the drop.
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
    event: &Event,
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
            controller.send_from(out, mime_type, &mut Cursor::new(TEXT))?;
        }
        Event::DragFailed(error) => return Err(format!("{error}").into()),
        _ => {}
    }
    Ok(())
}
```

Both `send` and `send_from` send a complete body and its end marker.
`send_from` reads synchronously with bounded buffers.
Output methods do not flush, so flush responses before the next input wait.
This dispatcher calls both helpers and cancels the incoming drop on decode errors:

```rust
use std::io::Write;
use uncurses::event::Event as TerminalEvent;
use uncurses_dragdrop::DragDrop;

fn dispatch_terminal_event(
    controller: &mut DragDrop,
    event: &TerminalEvent,
    out: &mut impl Write,
    destination: &mut Vec<u8>,
) -> Result<(), Box<dyn std::error::Error>> {
    let events = match controller.handle_event(event) {
        Ok(events) => events,
        Err(error) => {
            destination.clear();
            controller.cancel_drop(out)?;
            out.flush()?;
            return Err(error.into());
        }
    };
    for event in events {
        receive_text(controller, &event, out, destination)?;
        offer_text(controller, &event, out)?;
    }
    out.flush()?;
    Ok(())
}
```

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
| `EntryKind::Directory(directory)` | `Listed` events contain child names and entries |

Preserve directory names as bytes until your application chooses a filesystem policy.
Append directory children to the queue and request them one at a time.
Release each `Directory` with `close_directory` after its direct children finish.
Release an empty directory at its `DataEnd`.
Call `complete_drop` after all required data reaches its destination.
Cancel the drop if a request or destination fails.

Pass the opaque `Entry` supplied by `Listed` back to `request`.
The controller validates the entry's owner and listing.
Closing a directory invalidates its child entries, while nested directories remain open until explicitly closed.
Raw URI bodies stream without retaining full records.
The default `max_record_bytes` limit bounds each retained name to 64 KiB.
After a raw `Entry::UriList` request reaches `DataEnd`, use `root_entry` to obtain a requestable root:

```rust
use std::io::Write;
use uncurses_dragdrop::DragDrop;

fn request_first_root(
    controller: &mut DragDrop,
    out: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let entry = controller.root_entry(0).ok_or("the URI list has no first root")?;
    controller.request(out, entry)?;
    Ok(())
}
```

Indices start at zero and exclude comments and empty lines.
The method returns `None` before completion or for an index outside the list.
`Entry::Text` and `Entry::Custom(mime)` select other MIME bodies.
Existing MIME strings also work with `request`, `send`, and `send_from`.

Your application controls storage, byte limits, and path validation.
Treat incoming names, URIs, and symlink targets as untrusted data.
Keep writes inside the chosen destination and decide whether to create symlinks.
The controller transfers bytes; your application performs filesystem operations.

### Offer files and directory trees

On `DragRequested`, call `offer_paths` with local paths.
The method resolves parent directories, preserves final symlinks, and inspects every source before protocol output.
A trailing separator requests a directory, including a symlink's target directory.
It offers `text/uri-list`, supplies its initial data, and returns root entries in supplied order.
Associate these entries with your application sources:

```rust
use std::{collections::HashMap, io::Write, path::PathBuf};
use uncurses_dragdrop::{DragDrop, Entry, Error, Operations};

fn offer_paths(
    controller: &mut DragDrop,
    out: &mut impl Write,
    paths: &[PathBuf],
) -> Result<HashMap<Entry, PathBuf>, Error> {
    let entries = controller.offer_paths(out, paths, Operations::COPY)?;
    let sources = entries.into_iter().zip(paths.iter().cloned()).collect();
    controller.start_drag(out)?;
    out.flush()?;
    Ok(sources)
}
```

Add any thumbnails before `start_drag`.
Use `offer_files` for application-supplied URIs, with `/` at the end of directory URIs.
Use `path_to_uri` when your application needs a native path's URI separately.
Applications that stream a raw URI list can use `FileEntry::root_index` to correlate requested roots.
That accessor returns a zero-based URI position.

For remote transfers, `EntryRequested` reports a terminal root request.
Call `next_send` to select the next eligible response.
Use `send` or `send_from` for file contents and `send_symlink` for a native link target.
Inspect entries with `std::fs::symlink_metadata` to preserve symlinks.

`send_directory` accepts native names, including values from `std::fs::DirEntry::file_name`.
It returns child entries in listing order.
Associate those entries with the child paths, then check `next_send` again.
The controller selects children in breadth-first order and finishes a complete tree before the next root.
For a small directory with known contents:

```rust
use std::io::Write;
use uncurses_dragdrop::{DragDrop, Entry};

fn send_tree(
    controller: &mut DragDrop,
    out: &mut impl Write,
    root: Entry,
) -> Result<(), Box<dyn std::error::Error>> {
    let children = controller.send_directory(out, root, ["hello.txt", "link"])?;
    controller.send(out, &children[0], b"Hello!\n")?;
    controller.send_symlink(out, &children[1], "hello.txt")?;
    out.flush()?;
    Ok(())
}
```

For arbitrary trees, follow the [bidirectional example](https://github.com/aymanbagabas/uncurses/blob/main/examples/examples/drag_drop.rs).
It pairs returned entries with paths and uses `next_send` between terminal events.
Check local work before blocking for input because directory children do not require new terminal requests.
Each body remains synchronous; a slow file or terminal can block within a send.

Keep root associations until the outgoing exchange ends because roots can be requested repeatedly.
Remove completed child associations when the application no longer needs them.
Clear outgoing associations on completion, failure, or explicit cancellation.
An incoming `DropCanceled` affects only incoming state.

Native names preserve Unix bytes.
Other platforms report unsupported filename conversions instead of replacing characters.
Use `send_directory_bytes` and `send_symlink_bytes` for uninterpreted protocol data.

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
Call `select_thumbnail` after `Event::DragStarted`.
If thumbnail input fails before any output, use `cancel_drag` to release the prepared offer.
If a thumbnail continuation is already on the wire, restore the terminal and discard the controller.
In that case, further output methods, including `cancel_drag` and `close`, return `Error::OutputFailed`.

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

Run the controller example for both directions:

```sh
cargo run -p examples --example drag_drop -- Cargo.toml uncurses-dragdrop/src
```

The example accepts files, directory trees, plain text, HTML, PNG, and JPEG.
It supports the `text/plain;charset=utf-8` and `UTF8_STRING` text aliases.
Incoming content gets a bounded, escaped byte preview.
HTML stays inert; image bodies remain binary data.

Press f for supplied files, t for text, h for HTML, p for PNG, or j for JPEG.
The text, HTML, and image choices offer generated sample content.
Supplied files retain their contents and names, including images inside nested directories.
Without source paths, the first outgoing selection is text.
Press 1 for an image thumbnail, 2 for text, or 0 for none.
Content and thumbnail selections apply to the next drag.
Press q or Ctrl-C to exit.
The capability query has a two-second deadline.
An exit request during that query drains replies through the device attributes reply or the deadline, without registration.

To save incoming drops, supply a receive directory:

```sh
cargo run -p examples --example drag_drop -- --receive-dir ./received Cargo.toml
```

Each drop gets a new `drop-*` directory.
File drops preserve their names and directory structure.
Other drops use `text.txt`, `content.html`, `image.png`, or `image.jpg`.
The example removes partial data on cancellation, replacement, failure, or exit.
Completed drops remain on disk.
It rejects duplicate names within a drop and child paths that are not single filenames.
It decodes each source URI's final path segment and validates the filename for the destination platform.
POSIX and Windows source URIs use the same receive path.

On Unix, saved symlinks retain relative targets without following them.
Targets that lexically escape the drop directory cause an error.
Leading `..` components can reach existing parent directories inside the drop.
The example rejects `..` after a named component because another received symlink can change that component's destination.
Other platforms report an error for saved symlinks.
Treat saved links and HTML as untrusted content when opening them elsewhere.
The example copies file URIs that name a file or directory.
Root URIs without a safe file name remain in the escaped log and are skipped.
File access and validation errors cancel the affected transfer and leave the example available.
Terminal output failures and other protocol errors stop the example after cleanup.
Outgoing traversal retains open directory handles and opens children without following symbolic links.
Directory listings include regular files, directories, and symlinks.
Each opened file must be a regular file before transmission.
Each body transfer is synchronous, so a large transfer can delay keyboard input.

For SSH, start the example on the application host from a local Kitty terminal:

```sh
ssh -t user@host 'cd /path/to/uncurses && cargo run -p examples --example drag_drop -- --receive-dir ./received Cargo.toml uncurses-dragdrop/src'
```

Replace the host and checkout path with your own.
The checkout, source paths, and receive directory belong to the application host.
Drop local files onto the terminal to save them on the SSH host.
Drag offered remote files into a local file manager to copy them back.
The example configures `MachineId::current()` explicitly.
Kitty compares host identities and requests file contents when they differ.
Use a terminal connection that passes OSC 72 messages in both directions.
Multiplexers can require additional passthrough configuration.

Run the low-level text example to explore the raw protocol:

```sh
cargo run -p examples --example drag_drop_raw
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

The `drag_drop_raw` example offers text with an RGBA thumbnail and answers later data requests:

```sh
cargo run -p examples --example drag_drop_raw
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
