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

For incoming drops, accept an offer and request one MIME type after the drop.
Write each decoded `Event::Data` chunk to your destination.
After `Event::DataEnd`, call `complete_drop` or `cancel_drop`.
The controller permits one receive request at a time.

For outgoing drags, offer MIME types and supply initial data with `send`.
Call `start_drag`, then answer `Event::DataRequested` with `send` or `send_from`.
Both methods send the complete body and its end marker.
`send_from` reads synchronously with bounded buffers.

Output methods write without a flush.
Flush responses promptly.
Call `close` and flush before the terminal session ends.
Attempt terminal restoration even when controller cleanup fails.

The application owns file access, previews, and copy or move policy.
Default options leave the machine and routing identities unset.
Remote file requests and remote URI responses return `Error::UnsupportedRemoteFiles`.
Cancel the exchange and clean up when this error occurs.

See the [guide](https://uncurses.org/docs/guides/drag-and-drop/) for the event loop
and the [API reference](https://uncurses.org/api/uncurses_dragdrop/) for method details.

## Example

The [controller example](../examples/examples/drag_drop_high_level.rs) receives
text copies with a bounded preview and offers text drags:

```sh
cargo run -p examples --example drag_drop_high_level
```

Drop text onto the window, or use Kitty's drag gesture to copy text out.
Press q or Ctrl-C to exit.
The [raw example](../examples/examples/drag_drop.rs) demonstrates protocol messages and a thumbnail.

## Install

Depend on the crate directly from git:

```toml
[dependencies]
uncurses-dragdrop = { git = "https://github.com/aymanbagabas/uncurses" }
```

## License

MIT. See [LICENSE](../LICENSE).
