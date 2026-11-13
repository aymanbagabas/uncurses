# uncurses-dragdrop

A typed controller for Kitty drag and drop, built on [uncurses](../uncurses/).
`DragDrop` tracks each exchange and decodes incoming data in chunks.
Your application selects MIME types, handles bytes, and confirms the result.
Kitty 0.47 or newer supports the protocol.

## Usage

Probe support through `Program` before registration.
Wait for `Event::PrimaryDeviceAttributes` in the normal event loop, then check support and enable the desired roles.
Feed terminal events to `DragDrop::handle_event`.
Handle the returned events in your application.

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

For incoming drops, accept an offer and request one entry after the drop.
Write each decoded `Event::Data` chunk to your destination.
After `Event::DataEnd`, call `complete_drop` or `cancel_drop`.
The controller permits one receive request at a time.

A new drag can replace an unfinished drop.
The controller emits `Event::DropCanceled` before the new hover or drop notification.
Discard partial application data and queued entries when this occurs.
The terminal releases the old drop's resources; process the new offer after local cleanup.

For outgoing drags, offer MIME types and supply initial data with `send`.
Call `start_drag`, then answer `Event::DataRequested` with `send` or `send_from`.
Both methods send the complete body and its end marker.
`send_from` reads synchronously with bounded buffers.
Use `Entry::Text`, `Entry::UriList`, or a MIME string to select the content.

For file drops, call `list` after the drop.
`Event::Listed` supplies each URI and its requestable `Entry`.
Request an entry to receive its kind, data, or directory listing.
Process requests sequentially and release directory handles with `close_directory`.
File contents and symlink targets arrive through `Data` and `DataEnd`.

For outgoing files, call `offer_files` with file URIs.
Answer `EntryRequested` with `send`, `send_from`, `send_symlink`, or `send_directory`.
Send each directory's complete tree in breadth-first order before the next root.
The application supplies the file data and names.

Upload drag thumbnails before `start_drag` with `add_thumbnail` or `add_thumbnail_from`.
The controller supports RGB, RGBA, PNG, and text.
Use `select_thumbnail` to select an uploaded image or remove the current image.

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
The application owns destination paths, filename validation, limits, and cleanup.

See the [guide](https://uncurses.org/docs/guides/drag-and-drop/) for the event loop
and the [API reference](https://uncurses.org/api/uncurses_dragdrop/) for method details.

## Examples

The [controller example](../examples/examples/drag_drop_high_level.rs) receives
text copies with a bounded preview and offers text drags:

```sh
cargo run -p examples --example drag_drop_high_level
```

Drop text onto the window, or use Kitty's drag gesture to copy text out.
Press q or Ctrl-C to exit.
Press 1 for an image thumbnail, 2 for text, or 0 for none.
The selection applies to the next drag.
Both this example and the [raw example](../examples/examples/drag_drop.rs) include a thumbnail.

Run the interactive [file example](../examples/examples/drag_drop_files.rs):

```sh
cargo run -p examples --example drag_drop_files -- Cargo.toml uncurses-dragdrop/src
```

Drop files onto the window to inspect their entries and byte counts.
The example reads received data without saving it.
Use Kitty's drag gesture to copy the supplied source paths out.
It reads directory trees in breadth-first order and sends symlink targets without following them.
Run without source paths to receive drops only.
The same thumbnail keys and exit keys work in both interactive examples.

Run the file example on an SSH host from a local Kitty terminal:

```sh
ssh -t user@host 'cd /path/to/uncurses && cargo run -p examples --example drag_drop_files -- Cargo.toml uncurses-dragdrop/src'
```

The checkout and source paths belong to the SSH host.
The file example uses `MachineId::current()` to identify that host.
Kitty transfers file contents when the host identities differ.

The in-memory demonstration remains available without a terminal:

```sh
cargo run -p examples --example drag_drop_files -- --scripted
```

Print the current host's hashed identity with the [machine ID example](../examples/examples/machine_id.rs):

```sh
cargo run -p examples --example machine_id
```

## Install

Depend on the crate directly from git:

```toml
[dependencies]
uncurses-dragdrop = { git = "https://github.com/aymanbagabas/uncurses" }
```

## License

MIT. See [LICENSE](../LICENSE).
