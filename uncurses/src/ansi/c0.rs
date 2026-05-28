//! C0 control characters (0x00..=0x1F).
//!
//! These are the single-byte ASCII control codes. The two-byte `ESC X`
//! introducer sequences (e.g. `ESC [` for CSI) are 7-bit fallback encodings
//! and live in the writer modules alongside the sequences they produce —
//! they are *not* the same as the corresponding C1 control byte (see
//! [`crate::ansi::c1`]).

/// Null.
pub const NUL: u8 = 0x00;
/// Start of Heading.
pub const SOH: u8 = 0x01;
/// Start of Text.
pub const STX: u8 = 0x02;
/// End of Text.
pub const ETX: u8 = 0x03;
/// End of Transmission.
pub const EOT: u8 = 0x04;
/// Enquiry.
pub const ENQ: u8 = 0x05;
/// Acknowledge.
pub const ACK: u8 = 0x06;
/// Bell.
pub const BEL: u8 = 0x07;
/// Backspace.
pub const BS: u8 = 0x08;
/// Horizontal Tab.
pub const HT: u8 = 0x09;
/// Line Feed.
pub const LF: u8 = 0x0A;
/// Vertical Tab.
pub const VT: u8 = 0x0B;
/// Form Feed.
pub const FF: u8 = 0x0C;
/// Carriage Return.
pub const CR: u8 = 0x0D;
/// Shift Out.
pub const SO: u8 = 0x0E;
/// Shift In.
pub const SI: u8 = 0x0F;
/// Data Link Escape.
pub const DLE: u8 = 0x10;
/// Device Control 1 (XON).
pub const DC1: u8 = 0x11;
/// Device Control 2.
pub const DC2: u8 = 0x12;
/// Device Control 3 (XOFF).
pub const DC3: u8 = 0x13;
/// Device Control 4.
pub const DC4: u8 = 0x14;
/// Negative Acknowledge.
pub const NAK: u8 = 0x15;
/// Synchronous Idle.
pub const SYN: u8 = 0x16;
/// End of Transmission Block.
pub const ETB: u8 = 0x17;
/// Cancel.
pub const CAN: u8 = 0x18;
/// End of Medium.
pub const EM: u8 = 0x19;
/// Substitute.
pub const SUB: u8 = 0x1A;
/// Escape.
pub const ESC: u8 = 0x1B;
/// File Separator.
pub const FS: u8 = 0x1C;
/// Group Separator.
pub const GS: u8 = 0x1D;
/// Record Separator.
pub const RS: u8 = 0x1E;
/// Unit Separator.
pub const US: u8 = 0x1F;

/// Locking Shift 0 (alias for [`SI`]).
pub const LS0: u8 = SI;
/// Locking Shift 1 (alias for [`SO`]).
pub const LS1: u8 = SO;
