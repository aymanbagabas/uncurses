//! C1 control characters (0x80..=0x9F).
//!
//! These are the single-byte 8-bit C1 controls. Each one has a 7-bit fallback
//! encoding as `ESC X` where X = byte - 0x40 (e.g. CSI = 0x9B has fallback
//! `ESC [` = `0x1B 0x5B`). The 7-bit forms are not the same constant — they
//! are two-byte sequences emitted by writer helpers; if you need them, use
//! the appropriate writer module rather than concatenating bytes manually.

/// Padding Character.
pub const PAD: u8 = 0x80;
/// High Octet Preset.
pub const HOP: u8 = 0x81;
/// Break Permitted Here.
pub const BPH: u8 = 0x82;
/// No Break Here.
pub const NBH: u8 = 0x83;
/// Index.
pub const IND: u8 = 0x84;
/// Next Line.
pub const NEL: u8 = 0x85;
/// Start of Selected Area.
pub const SSA: u8 = 0x86;
/// End of Selected Area.
pub const ESA: u8 = 0x87;
/// Horizontal Tab Set.
pub const HTS: u8 = 0x88;
/// Horizontal Tab with Justify.
pub const HTJ: u8 = 0x89;
/// Vertical Tab Set.
pub const VTS: u8 = 0x8A;
/// Partial Line Down.
pub const PLD: u8 = 0x8B;
/// Partial Line Up.
pub const PLU: u8 = 0x8C;
/// Reverse Index.
pub const RI: u8 = 0x8D;
/// Single Shift 2.
pub const SS2: u8 = 0x8E;
/// Single Shift 3.
pub const SS3: u8 = 0x8F;
/// Device Control String.
pub const DCS: u8 = 0x90;
/// Private Use 1.
pub const PU1: u8 = 0x91;
/// Private Use 2.
pub const PU2: u8 = 0x92;
/// Set Transmit State.
pub const STS: u8 = 0x93;
/// Cancel Character.
pub const CCH: u8 = 0x94;
/// Message Waiting.
pub const MW: u8 = 0x95;
/// Start of Protected Area.
pub const SPA: u8 = 0x96;
/// End of Protected Area.
pub const EPA: u8 = 0x97;
/// Start of String.
pub const SOS: u8 = 0x98;
/// Single Graphic Character Introducer.
pub const SGCI: u8 = 0x99;
/// Single Character Introducer.
pub const SCI: u8 = 0x9A;
/// Control Sequence Introducer.
pub const CSI: u8 = 0x9B;
/// String Terminator.
pub const ST: u8 = 0x9C;
/// Operating System Command.
pub const OSC: u8 = 0x9D;
/// Privacy Message.
pub const PM: u8 = 0x9E;
/// Application Program Command.
pub const APC: u8 = 0x9F;
