# uncurses-dragdrop

A typed controller for Kitty drag and drop, built on [uncurses](../uncurses/).
`DragDrop` tracks each exchange and decodes incoming data in chunks.
Your application selects MIME types, handles bytes, and confirms the result.
Kitty 0.47 or newer supports the protocol.

## Usage

Call `Program::query_capabilities(uncurses::ansi::drag_drop::REQUEST_DRAG_DROP)` before registration.
Wait for `uncurses::event::Event::PrimaryDeviceAttributes`, then check support and enable the desired roles.
Set a deadline and limit each `Program::poll_event` wait to the time that remains.
If the deadline expires, report unconfirmed support, keep both roles disabled, and restore the terminal with `Program::finish`.
Feed terminal events to `DragDrop::handle_event`.
Handle the returned events in your application.
If `handle_event` returns an error, discard partial destination data and queued entries.
Cancel the affected exchange and flush the output.
When the error does not identify a role, call both `cancel_drop` and `cancel_drag`.
Decode errors can occur without a `DropFailed` event.

Construct the controller in the caller and pass it to the registration helper:

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

The caller retains the controller if registration or its flush fails.
Attempt `close`, flush the cleanup output, and restore the terminal on every exit path.

For incoming drops, accept an offer and request one entry after the drop.
Write each decoded `Event::Data` chunk to your destination.
`Event::DataEnd` ends one request, including a URI or directory listing.
Finish every desired request and copy any promised files before calling `complete_drop`.
Call `cancel_drop` if the application cannot finish the copy.
The controller permits one receive request at a time.

A new drag can replace an unfinished drop.
The controller emits `Event::DropCanceled` before the next hover, drop, or leave notification.
Discard partial application data and queued entries on `DropCanceled` and `DropFailed`.
The terminal releases the old drop's resources; process the next notification after local cleanup.

For outgoing drags, wait for the terminal gesture notification, `Event::DragRequested`.
Then offer MIME types and supply initial data with `send`.
Call `start_drag` and wait for `Event::DragStarted` to confirm startup.
Answer later `Event::DataRequested` notifications with `send` or `send_from`.
On success, both methods send the complete body and its end marker.
`send_from` reads synchronously with bounded buffers.
If a source read fails, use `fail_data` to end a MIME response with an error.
For file responses, abort the exchange with `fail_drag` or `cancel_drag`.
Cancel a failed initial offer before startup.
Terminal output errors require terminal recovery instead of another response.
Use `Entry::Text`, `Entry::UriList`, or a MIME string to select the content.

For file drops, call `list` after the drop.
`Event::Listed` supplies each URI and its requestable `Entry`.
Queue these entries until the listing reaches `Event::DataEnd`.
Then request an entry to receive its kind, data, or directory listing.
Process requests sequentially and release each `Directory` with `close_directory`.
Release it when its direct children finish; open child directories have independent lifetimes.
File contents and symlink targets arrive through `Data` and `DataEnd`.
After a raw `Entry::UriList` request completes, `root_entry(index)` supplies each root by its zero-based index.

After `Event::DragRequested`, call `offer_paths` with local `Path` or `PathBuf` values.
It inspects each path and returns root entries in supplied order.
Associate these entries with application sources, then call `start_drag`.
Use `offer_files` when the application supplies file URIs directly.

`next_send` selects each pending file response in protocol order.
Use `send` or `send_from` for file contents.
Use `send_symlink` with a path from `std::fs::read_link`.
Use `send_directory` with names from `std::fs::DirEntry::file_name`.
Associate its returned child entries with their source paths.
The controller selects children in breadth-first order and completes each tree before the next root.
Check pending work before blocking for another terminal event.

Entries belong to their controller, direction, and listing.
Keep root associations until the outgoing exchange ends because roots can be requested repeatedly.
Discard completed child associations and clear outgoing state on completion, failure, or cancellation.
Incoming `DropCanceled` cleanup preserves independent outgoing work.
The application owns traversal and supplies all data.

Native filename helpers preserve Unix bytes and report unsupported conversions on other platforms.
Use `send_directory_bytes` and `send_symlink_bytes` for uninterpreted protocol names and targets.

Upload drag thumbnails before `start_drag` with `add_thumbnail` or `add_thumbnail_from`.
The controller supports RGB, RGBA, PNG, and text.
After `Event::DragStarted`, use `select_thumbnail` to select an uploaded image or remove the current image.

Output methods write without a flush.
Flush responses promptly.
For large transfers through a `Screen`, flush its staged output first.
Then pass `screen.writer_mut()` to the controller and flush that writer afterward.
This keeps the screen's output buffer from retaining the complete transfer.
Call `close` and flush before the terminal session ends.
Attempt terminal restoration even when controller cleanup fails.

The application owns file access, previews, and copy or move policy.
Default options leave the machine and routing identities unset.
Use `MachineId::current()?` to derive the application host's identity.
Assign the result to `DragDropOptions::machine_id` as `Some(id)`.
The method reads the operating system's machine ID and applies the protocol's HMAC-SHA256 hash.
It returns an error if the source is unavailable or empty.
The terminal uses the identity to choose local file access or remote transfer.
The application owns destination paths, filename validation, and cleanup.

`DragDropOptions::default()` sets control and record bounds of 64 KiB, 10,000 entries per drop, and 128 open directories.
It also supplies a 4 KiB destination path limit for filesystem adapters.
`max_pending_requests` limits unanswered outgoing MIME and root requests to 128 combined.
Application-supplied directory children are separate from this request limit.
The bidirectional example enforces the path limit and uses the directory limit for local handles.
Set any limit field to `None` to disable that limit.
Fields omitted with `..DragDropOptions::default()` retain their default values.

See the [guide](https://uncurses.org/docs/guides/drag-and-drop/) for the event loop
and the [API reference](https://uncurses.org/api/uncurses_dragdrop/) for method details.

## Examples

The [bidirectional example](../examples/examples/drag_drop.rs) receives drops
and offers drags for files, directory trees, text, HTML, PNG, and JPEG:

```sh
cargo run -p examples --example drag_drop -- Cargo.toml uncurses-dragdrop/src
```

Drop content onto the window to inspect a bounded, escaped byte preview.
Use Kitty's drag gesture to copy the selected content out.
The example offers supplied paths first, or generated text when run without paths.

Press f for files, t for text, h for HTML, p for PNG, or j for JPEG.
Press 1 for an image thumbnail, 2 for text, or 0 for none.
These selections apply to the next drag.
Press q or Ctrl-C to exit.

To save incoming drops:

```sh
cargo run -p examples --example drag_drop -- --receive-dir ./received Cargo.toml
```

Each drop gets a fresh `drop-*` directory.
Files retain their names and directory structure.
Raw MIME bodies use `text.txt`, `content.html`, `image.png`, or `image.jpg`.
HTML stays inert, and previews escape control bytes.
Canceled or failed drops remove partial destination data.
Completed drops remain on disk.
The receiver processes directory children before pending siblings, in their listed order.
It skips root URIs that cannot supply a valid local filename, including HTTPS links.
Completion confirms the selected file copies, not a copy of every listed URI.

The example rejects duplicate destination names and unsafe child paths.
On Unix, it saves relative symlinks without following them and rejects targets that lexically escape the drop directory.
It also rejects `..` after a named component to prevent escapes through other received symlinks.
Other platforms report an error for saved symlinks.
Treat saved links and HTML as untrusted content when opening them in another application.
File access and validation errors cancel the affected transfer, remove partial output, and leave the example available.
Terminal output failures and other protocol errors stop the example after cleanup.
Controller limits cancel both roles because the error does not identify the affected exchange.

Run on an SSH host from a local Kitty terminal:

```sh
ssh -t user@host 'cd /path/to/uncurses && cargo run -p examples --example drag_drop -- --receive-dir ./received Cargo.toml uncurses-dragdrop/src'
```

The checkout, source paths, and receive directory belong to the SSH host.
Drop local files onto the terminal to copy them to that host.
Drag offered remote files into a local file manager to copy them back.
The example uses `MachineId::current()` so Kitty can select remote transfer.
Each body transfer is synchronous.

The [raw example](../examples/examples/drag_drop_raw.rs) demonstrates text transfers
through the low-level protocol interface:

```sh
cargo run -p examples --example drag_drop_raw
```

Print the current host's hashed identity with the [machine ID example](../examples/examples/machine_id.rs):

```sh
cargo run -p examples --example machine_id
```

The [limits example](../examples/examples/drag_drop_limits.rs) simulates repeated terminal requests with a one-request limit:

```sh
cargo run -p examples --example drag_drop_limits
```

Its output shows the second request's error and successful drag cancellation.

## Install

Depend on the crate directly from git:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
uncurses-dragdrop = { git = "https://github.com/aymanbagabas/uncurses" }
```

## License

MIT. See [LICENSE](../LICENSE).
