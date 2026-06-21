//! C1 control bytes (`0x80..=0x9f`).
//!
//! ## Category
//!
//! C1 controls are the 8-bit single-byte forms of terminal controls such as DCS,
//! CSI, ST, OSC, PM, and APC.
//!
//! ## 7-bit versus 8-bit forms
//!
//! Each C1 byte has a 7-bit fallback spelling `ESC` followed by `byte - 0x40`.
//! For example, [`CSI`] is `0x9b` while the 7-bit CSI introducer is the two-byte
//! sequence `ESC [`.
//!
//! ```text
//! 8-bit:  9B        CSI
//! 7-bit:  1B 5B     ESC [
//!        ─┬ ─┬
//!        ESC final
//! ```
//!
//! ## Mode interaction
//!
//! The byte constants do not enable any terminal mode. The tokenizer recognizes
//! the C1 introducers directly and also recognizes their 7-bit `ESC` forms.

/// Padding Character C1 control byte `0x80`.
pub const PAD: u8 = 0x80;
/// High Octet Preset C1 control byte `0x81`.
pub const HOP: u8 = 0x81;
/// Break Permitted Here C1 control byte `0x82`.
pub const BPH: u8 = 0x82;
/// No Break Here C1 control byte `0x83`.
pub const NBH: u8 = 0x83;
/// Index C1 control byte `0x84`; 7-bit spelling is `ESC D`.
pub const IND: u8 = 0x84;
/// Next Line C1 control byte `0x85`; 7-bit spelling is `ESC E`.
pub const NEL: u8 = 0x85;
/// Start of Selected Area C1 control byte `0x86`.
pub const SSA: u8 = 0x86;
/// End of Selected Area C1 control byte `0x87`.
pub const ESA: u8 = 0x87;
/// Horizontal Tab Set C1 control byte `0x88`; 7-bit spelling is `ESC H`.
pub const HTS: u8 = 0x88;
/// Horizontal Tab with Justify C1 control byte `0x89`.
pub const HTJ: u8 = 0x89;
/// Vertical Tab Set C1 control byte `0x8a`.
pub const VTS: u8 = 0x8A;
/// Partial Line Down C1 control byte `0x8b`.
pub const PLD: u8 = 0x8B;
/// Partial Line Up C1 control byte `0x8c`.
pub const PLU: u8 = 0x8C;
/// Reverse Index C1 control byte `0x8d`; 7-bit spelling is `ESC M`.
pub const RI: u8 = 0x8D;
/// Single Shift 2 C1 control byte `0x8e`; 7-bit spelling is `ESC N`.
pub const SS2: u8 = 0x8E;
/// Single Shift 3 C1 control byte `0x8f`; 7-bit spelling is `ESC O`.
pub const SS3: u8 = 0x8F;
/// Device Control String introducer byte `0x90`; 7-bit spelling is `ESC P` and terminator is [`ST`].
pub const DCS: u8 = 0x90;
/// Private Use 1 C1 control byte `0x91`.
pub const PU1: u8 = 0x91;
/// Private Use 2 C1 control byte `0x92`.
pub const PU2: u8 = 0x92;
/// Set Transmit State C1 control byte `0x93`.
pub const STS: u8 = 0x93;
/// Cancel Character C1 control byte `0x94`.
pub const CCH: u8 = 0x94;
/// Message Waiting C1 control byte `0x95`.
pub const MW: u8 = 0x95;
/// Start of Protected Area C1 control byte `0x96`.
pub const SPA: u8 = 0x96;
/// End of Protected Area C1 control byte `0x97`.
pub const EPA: u8 = 0x97;
/// Start of String introducer byte `0x98`; 7-bit spelling is `ESC X`.
pub const SOS: u8 = 0x98;
/// Single Graphic Character Introducer C1 control byte `0x99`.
pub const SGCI: u8 = 0x99;
/// Single Character Introducer C1 control byte `0x9a`.
pub const SCI: u8 = 0x9A;
/// Control Sequence Introducer byte `0x9b`; 7-bit spelling is `ESC [`.
pub const CSI: u8 = 0x9B;
/// String Terminator byte `0x9c`; 7-bit spelling is `ESC \`.
pub const ST: u8 = 0x9C;
/// Operating System Command introducer byte `0x9d`; 7-bit spelling is `ESC ]`.
pub const OSC: u8 = 0x9D;
/// Privacy Message introducer byte `0x9e`; 7-bit spelling is `ESC ^`.
pub const PM: u8 = 0x9E;
/// Application Program Command introducer byte `0x9f`; 7-bit spelling is `ESC _`.
pub const APC: u8 = 0x9F;
