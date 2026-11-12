use std::{fmt, io, str::FromStr};

bitflags::bitflags! {
    /// Operations an offer permits.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Operations: u8 {
        /// Copy the content.
        const COPY = 1;
        /// Move the content.
        const MOVE = 2;
    }
}

/// The application-selected operation for a drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Copy the content.
    Copy,
    /// Move the content.
    Move,
}

impl Operation {
    pub(crate) fn flag(self) -> Operations {
        match self {
            Self::Copy => Operations::COPY,
            Self::Move => Operations::MOVE,
        }
    }
}

/// A position in terminal cells, with pixel offsets within the cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    /// Zero-based column.
    pub column: u32,
    /// Zero-based row.
    pub row: u32,
    /// Signed horizontal pixel offset within the cell, as reported by the terminal.
    pub pixel_x: i32,
    /// Signed vertical pixel offset within the cell, as reported by the terminal.
    pub pixel_y: i32,
}

/// Content currently over the window or actually dropped into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropOffer {
    /// Location of the pointer.
    pub position: Position,
    /// Available MIME types, in the terminal's order.
    pub mime_types: Vec<String>,
    /// Operations permitted by the source.
    pub operations: Operations,
}

/// A version 1 protocol machine identity.
///
/// Parse `1:` followed by the 64 hexadecimal digits of the HMAC-SHA256 digest.
/// The protocol hashes the platform machine ID with the key
/// `tty-dnd-protocol-machine-id`. Supply that digest, rather than a raw UUID.
/// Machine identity lets the terminal distinguish local and remote file URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineId(String);

impl MachineId {
    /// Return the identity in its wire format.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for MachineId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Error> {
        let hash = value
            .strip_prefix("1:")
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or(Error::InvalidInput(
                "expected a version 1 machine ID digest",
            ))?;
        Ok(Self(format!("1:{}", hash.to_ascii_lowercase())))
    }
}

impl fmt::Display for MachineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Fixed identities shared by both roles of a controller.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DragDropOptions {
    /// Multiplexer routing ID. An absent ID uses the protocol default, zero.
    /// This identifies an endpoint, not an individual transfer.
    pub routing_id: Option<u32>,
    /// Optional hashed machine identity.
    pub machine_id: Option<MachineId>,
}

/// A terminal or application error in the protocol's symbolic form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteError {
    /// POSIX symbolic name, such as `EIO` or `ENOENT`.
    pub code: String,
    /// Optional escape-safe description.
    pub description: Option<String>,
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.code)?;
        if let Some(description) = &self.description {
            write!(f, ": {description}")?;
        }
        Ok(())
    }
}

impl std::error::Error for RemoteError {}

/// Application-facing notifications produced by a controller.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    /// A potential drop moved over the window.
    Hover(DropOffer),
    /// The potential drop left the window.
    Leave,
    /// The user released the content. Data can now be requested.
    Drop(DropOffer),
    /// Decoded bytes of the current incoming request.
    /// Boundaries can split UTF-8 characters and application records.
    Data(Vec<u8>),
    /// The current incoming body ended. Complete the drop or request more data.
    DataEnd,
    /// The terminal reported an incoming error.
    DropFailed(RemoteError),
    /// The user performed a drag gesture.
    DragRequested(Position),
    /// The terminal confirmed startup.
    DragStarted,
    /// A destination indicated its preferred MIME type.
    DragAccepted {
        /// The preferred MIME type.
        mime_type: String,
    },
    /// The destination changed its proposed operation.
    DragActionChanged {
        /// The proposed operation, or `None` for rejection.
        operation: Option<Operation>,
    },
    /// The user dropped the outgoing content onto a destination.
    DragDropped,
    /// A destination requested an offered MIME type.
    DataRequested {
        /// The requested MIME type.
        mime_type: String,
    },
    /// The outgoing exchange ended.
    ///
    /// This reports cancellation, not a confirmed final copy/move operation.
    /// Do not delete source files solely on the basis of this event.
    DragFinished {
        /// Whether the terminal reports user cancellation.
        canceled: bool,
    },
    /// The terminal rejected or aborted the outgoing exchange.
    DragFailed(RemoteError),
}

/// A local I/O, usage, or protocol failure.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A source read or terminal write failed.
    Io(io::Error),
    /// The operation is invalid in the current exchange state.
    InvalidState(&'static str),
    /// An application argument is invalid.
    InvalidInput(&'static str),
    /// An incoming message is malformed or inconsistent.
    Protocol(String),
    /// Remote file traversal or a remote URI response needs application support.
    /// Cancel the exchange or close the controller.
    UnsupportedRemoteFiles,
    /// A previous output failure left terminal state uncertain.
    /// Restore the terminal before constructing another controller.
    OutputFailed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::InvalidState(message) | Self::InvalidInput(message) => f.write_str(message),
            Self::Protocol(message) => write!(f, "invalid drag and drop message: {message}"),
            Self::UnsupportedRemoteFiles => f.write_str("remote file transfer is unsupported"),
            Self::OutputFailed => f.write_str("previous terminal output failed"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
