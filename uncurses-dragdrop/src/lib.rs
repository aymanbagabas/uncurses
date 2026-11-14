//! Event-driven Kitty drag and drop.
//!
//! A [`DragDrop`] controller lives alongside the application's terminal event
//! loop. It handles MIME indices, protocol messages, base64, and wire chunks.
//! The application owns its data and decides when to accept or complete a drop.
//!
//! Feed raw uncurses events into [`DragDrop::handle_event`]. Incoming content
//! arrives as [`Event::Data`] followed by [`Event::DataEnd`]. Write those bytes
//! to a file, process them directly, or collect them in a buffer.
//! [`DragDrop::send`] and [`DragDrop::send_from`] transmit complete bodies.
//!
//! Confirm terminal support with uncurses' capability query before registration.
//! Output methods write without flushing. Flush the terminal writer after each
//! batch. Call [`DragDrop::close`] before terminal teardown.
//!
//! ```
//! use uncurses_dragdrop::{DragDrop, DragDropOptions};
//!
//! let mut controller = DragDrop::new(DragDropOptions::default());
//! let mut output = Vec::new();
//! controller.enable_drops(&mut output, &["text/plain"])?;
//! controller.enable_drags(&mut output)?;
//! controller.close(&mut output)?;
//! # Ok::<(), uncurses_dragdrop::Error>(())
//! ```

mod controller;
mod incoming;
mod outgoing;
mod path;
mod protocol;
mod types;

pub use controller::DragDrop;
pub use path::path_to_uri;
pub use types::{
    Directory, DragDropOptions, DropOffer, Entry, EntryKind, Error, Event, FileEntry, MachineId,
    Operation, Operations, Position, RemoteError, ThumbnailFormat,
};

#[cfg(test)]
mod tests;
