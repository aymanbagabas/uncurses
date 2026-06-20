//! C0 control bytes (`0x00..=0x1f`).
//!
//! ## Category
//!
//! C0 controls are the 7-bit single-byte control characters shared with ASCII:
//! BEL, BS, HT, LF, CR, ESC, and related communication controls.
//!
//! ## 7-bit escape conventions
//!
//! `ESC` (`0x1b`) introduces 7-bit encodings of higher-level controls such as
//! CSI (`ESC [`), OSC (`ESC ]`), and DCS (`ESC P`). Those two-byte introducers
//! are emitted by the writer modules for the sequence family they control; they
//! are not represented as distinct constants here.
//!
//! ## Mode interaction
//!
//! Most C0 bytes are mode-independent, though terminals may interpret line-feed
//! behavior according to line-feed/new-line mode ([`Mode::LINE_FEED_NEW_LINE`](crate::ansi::mode::Mode::LINE_FEED_NEW_LINE)).

/// Null control byte `0x00`; used as a padding or string terminator in some byte protocols.
pub const NUL: u8 = 0x00;
/// Start of Heading control byte `0x01`.
pub const SOH: u8 = 0x01;
/// Start of Text control byte `0x02`.
pub const STX: u8 = 0x02;
/// End of Text control byte `0x03`.
pub const ETX: u8 = 0x03;
/// End of Transmission control byte `0x04`.
pub const EOT: u8 = 0x04;
/// Enquiry control byte `0x05`; historically asks for terminal identification.
pub const ENQ: u8 = 0x05;
/// Acknowledge control byte `0x06`.
pub const ACK: u8 = 0x06;
/// Bell control byte `0x07`; also terminates many OSC strings emitted by this crate.
pub const BEL: u8 = 0x07;
/// Backspace control byte `0x08`; moves one column left in ordinary terminal output.
pub const BS: u8 = 0x08;
/// Horizontal Tab control byte `0x09`; advances to the next tab stop.
pub const HT: u8 = 0x09;
/// Line Feed control byte `0x0a`; moves down one line, with carriage behavior depending on terminal mode.
pub const LF: u8 = 0x0A;
/// Vertical Tab control byte `0x0b`.
pub const VT: u8 = 0x0B;
/// Form Feed control byte `0x0c`.
pub const FF: u8 = 0x0C;
/// Carriage Return control byte `0x0d`; moves to the start of the current line.
pub const CR: u8 = 0x0D;
/// Shift Out control byte `0x0e`; invokes G1 into GL until shifted back.
pub const SO: u8 = 0x0E;
/// Shift In control byte `0x0f`; invokes G0 into GL.
pub const SI: u8 = 0x0F;
/// Data Link Escape control byte `0x10`.
pub const DLE: u8 = 0x10;
/// Device Control 1 byte `0x11`; commonly XON in software flow control.
pub const DC1: u8 = 0x11;
/// Device Control 2 byte `0x12`.
pub const DC2: u8 = 0x12;
/// Device Control 3 byte `0x13`; commonly XOFF in software flow control.
pub const DC3: u8 = 0x13;
/// Device Control 4 byte `0x14`.
pub const DC4: u8 = 0x14;
/// Negative Acknowledge control byte `0x15`.
pub const NAK: u8 = 0x15;
/// Synchronous Idle control byte `0x16`.
pub const SYN: u8 = 0x16;
/// End of Transmission Block control byte `0x17`.
pub const ETB: u8 = 0x17;
/// Cancel control byte `0x18`; often aborts an in-progress escape sequence.
pub const CAN: u8 = 0x18;
/// End of Medium control byte `0x19`.
pub const EM: u8 = 0x19;
/// Substitute control byte `0x1a`; may also abort an in-progress escape sequence.
pub const SUB: u8 = 0x1A;
/// Escape control byte `0x1b`; introduces 7-bit escape sequences such as CSI (`ESC [`) and OSC (`ESC ]`).
pub const ESC: u8 = 0x1B;
/// File Separator control byte `0x1c`.
pub const FS: u8 = 0x1C;
/// Group Separator control byte `0x1d`.
pub const GS: u8 = 0x1D;
/// Record Separator control byte `0x1e`.
pub const RS: u8 = 0x1E;
/// Unit Separator control byte `0x1f`.
pub const US: u8 = 0x1F;

/// Locking Shift 0, alias for [`SI`] (`0x0f`); selects G0 for GL.
pub const LS0: u8 = SI;
/// Locking Shift 1, alias for [`SO`] (`0x0e`); selects G1 for GL.
pub const LS1: u8 = SO;
