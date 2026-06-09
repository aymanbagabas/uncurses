//! Win32 console input mode decoder.
//!
//! Decodes `CSI Pn ; ... _` parameter packets produced by
//! `ENABLE_VIRTUAL_TERMINAL_INPUT` / win32-input-mode on Windows
//! consoles. Each packet carries one INPUT_RECORD-style key event with
//! virtual-key code, scan code, unicode codepoint, key-down flag,
//! control-key state, and a repeat count.

use super::Decoder;
use super::result::ParseResult;
use super::util::Params;
use crate::event::{Event, Key, KeyCode, KeyModifiers};

const WIN32_RIGHT_ALT_PRESSED: u32 = 0x0001;
const WIN32_LEFT_ALT_PRESSED: u32 = 0x0002;
const WIN32_RIGHT_CTRL_PRESSED: u32 = 0x0004;
const WIN32_LEFT_CTRL_PRESSED: u32 = 0x0008;
const WIN32_SHIFT_PRESSED: u32 = 0x0010;
const WIN32_NUMLOCK_ON: u32 = 0x0020;
const WIN32_SCROLLLOCK_ON: u32 = 0x0040;
const WIN32_CAPSLOCK_ON: u32 = 0x0080;
const WIN32_ENHANCED_KEY: u32 = 0x0100;

impl Decoder {
    /// Convert a single win32-input-mode CSI sequence
    /// (`CSI vk;sc;ch;kd;cks;rep _`) into a `KeyPress` /
    /// `KeyRelease` event. `wRepeatCount > 1` queues additional copies into `pending`
    /// so they are emitted on subsequent `parse` iterations.
    ///
    /// `vk == 0` denotes either a UTF-16 surrogate fragment (when the
    /// console is in VT input mode) or a synthesized text-only event;
    /// high surrogates are buffered keyed by `bKeyDown` and combined
    /// with the matching low surrogate before emission.
    pub(super) fn dispatch_win32_input(&self, params: Params<'_>, consumed: usize) -> ParseResult {
        let vk = params.get_or(0, 0) as u16;
        let _sc = params.get_or(1, 0) as u16;
        let ch = params.get_or(2, 0);
        let kd = params.get_or(3, 0) != 0;
        let cks = params.get_or(4, 0);
        let rep = params.get_or(5, 1) as u16;

        let mods = win32_translate_mods(cks);

        // vk == 0: text-only / surrogate fragment.
        if vk == 0 {
            let kd_idx = if kd { 1 } else { 0 };
            let mut high = self.win32_high_surrogate.get();
            // High surrogate: buffer and consume the sequence with no event.
            if (0xd800..=0xdbff).contains(&ch) {
                high[kd_idx] = Some(ch as u16);
                self.win32_high_surrogate.set(high);
                return ParseResult::None(consumed);
            }
            // Low surrogate: combine with the previously-buffered high half.
            let code = if (0xdc00..=0xdfff).contains(&ch) {
                if let Some(hi) = high[kd_idx].take() {
                    self.win32_high_surrogate.set(high);
                    let combined = 0x1_0000 + (((hi as u32) - 0xd800) << 10) + ((ch) - 0xdc00);
                    char::from_u32(combined).unwrap_or('\u{fffd}')
                } else {
                    char::from_u32(ch).unwrap_or('\u{fffd}')
                }
            } else {
                // Drop any stale high surrogate of the same direction.
                if high[kd_idx].is_some() {
                    high[kd_idx] = None;
                    self.win32_high_surrogate.set(high);
                }
                char::from_u32(ch).unwrap_or('\u{fffd}')
            };

            let mut key = Key::new(KeyCode::Char(code)).with_modifiers(mods);
            if !code.is_control() {
                key = key.with_text(code.to_string());
            }
            crate::event::key::normalize_shift_case(&mut key);
            return self.emit_win32_key(key, kd, rep, consumed);
        }

        // Modifier keys can lose their direction bit on release; fall back
        // to the previously-recorded control-key state to recover it.
        let prev_cks = self.win32_last_cks.get();
        let mut base_code = win32_vk_to_keycode(vk, cks, prev_cks);
        let mut text: Option<String> = None;

        // Numpad digits surface as their digit when NumLock-on shifted them.
        if (vk_consts::NUMPAD0..=vk_consts::NUMPAD9).contains(&vk) {
            let digit = (b'0' + ((vk - vk_consts::NUMPAD0) as u8)) as char;
            text = Some(digit.to_string());
        }
        let arith = match vk {
            vk_consts::MULTIPLY => Some('*'),
            vk_consts::ADD => Some('+'),
            vk_consts::SEPARATOR => Some(','),
            vk_consts::SUBTRACT => Some('-'),
            vk_consts::DECIMAL => Some('.'),
            vk_consts::DIVIDE => Some('/'),
            _ => None,
        };
        if let Some(c) = arith {
            text = Some(c.to_string());
        }

        // AltGr (left ctrl + right alt) on non-US layouts produces printable
        // text — strip the Ctrl/Alt modifiers and surface the composed glyph.
        let altgr_mask = WIN32_LEFT_CTRL_PRESSED | WIN32_RIGHT_ALT_PRESSED;
        let alt_gr = (cks & altgr_mask) == altgr_mask;
        let mut effective_mods = mods;
        // Lock keys are reported as separate KeyCode variants, not modifiers.
        let cks_no_locks = cks & !(WIN32_NUMLOCK_ON | WIN32_SCROLLLOCK_ON);

        let print_code = char::from_u32(ch).unwrap_or('\u{fffd}');
        if !print_code.is_control() {
            // Promote alpha vk codes into their typed character so callers
            // that match on `KeyCode::Char` get the actual glyph.
            base_code = KeyCode::Char(print_code);
            let printable = cks_no_locks == 0
                || cks_no_locks == WIN32_SHIFT_PRESSED
                || cks_no_locks == WIN32_CAPSLOCK_ON
                || cks_no_locks == (WIN32_SHIFT_PRESSED | WIN32_CAPSLOCK_ON)
                || alt_gr;
            if printable {
                text = Some(print_code.to_string());
                if alt_gr {
                    effective_mods.remove(KeyModifiers::CTRL);
                    effective_mods.remove(KeyModifiers::ALT);
                }
            }
        }

        let mut key = Key::new(base_code).with_modifiers(effective_mods);
        if let Some(t) = text {
            key = key.with_text(t);
        }
        crate::event::key::normalize_shift_case(&mut key);

        self.win32_last_cks.set(cks);
        self.emit_win32_key(key, kd, rep, consumed)
    }

    pub(super) fn emit_win32_key(
        &self,
        key: Key,
        kd: bool,
        rep: u16,
        consumed: usize,
    ) -> ParseResult {
        let mk_event = |k: Key| -> Event {
            if kd {
                Event::KeyPress(k)
            } else {
                Event::KeyRelease(k)
            }
        };
        let first = mk_event(key.clone());
        if rep > 1 {
            let mut q = self.pending.borrow_mut();
            for _ in 1..rep {
                q.push_back(mk_event(key.clone()));
            }
        }
        ParseResult::Event(first, consumed)
    }
}

fn win32_translate_mods(cks: u32) -> KeyModifiers {
    let mut m = KeyModifiers::empty();
    if cks & (WIN32_LEFT_CTRL_PRESSED | WIN32_RIGHT_CTRL_PRESSED) != 0 {
        m |= KeyModifiers::CTRL;
    }
    if cks & (WIN32_LEFT_ALT_PRESSED | WIN32_RIGHT_ALT_PRESSED) != 0 {
        m |= KeyModifiers::ALT;
    }
    if cks & WIN32_SHIFT_PRESSED != 0 {
        m |= KeyModifiers::SHIFT;
    }
    if cks & WIN32_CAPSLOCK_ON != 0 {
        m |= KeyModifiers::CAPS_LOCK;
    }
    if cks & WIN32_NUMLOCK_ON != 0 {
        m |= KeyModifiers::NUM_LOCK;
    }
    if cks & WIN32_SCROLLLOCK_ON != 0 {
        m |= KeyModifiers::SCROLL_LOCK;
    }
    m
}

#[allow(dead_code, non_upper_case_globals)]
mod vk_consts {
    pub const BACK: u16 = 0x08;
    pub const TAB: u16 = 0x09;
    pub const RETURN: u16 = 0x0d;
    pub const SHIFT: u16 = 0x10;
    pub const CONTROL: u16 = 0x11;
    pub const MENU: u16 = 0x12;
    pub const PAUSE: u16 = 0x13;
    pub const CAPITAL: u16 = 0x14;
    pub const ESCAPE: u16 = 0x1b;
    pub const SPACE: u16 = 0x20;
    pub const PRIOR: u16 = 0x21;
    pub const NEXT: u16 = 0x22;
    pub const END: u16 = 0x23;
    pub const HOME: u16 = 0x24;
    pub const LEFT: u16 = 0x25;
    pub const UP: u16 = 0x26;
    pub const RIGHT: u16 = 0x27;
    pub const DOWN: u16 = 0x28;
    pub const SELECT: u16 = 0x29;
    pub const SNAPSHOT: u16 = 0x2c;
    pub const INSERT: u16 = 0x2d;
    pub const DELETE: u16 = 0x2e;
    pub const LWIN: u16 = 0x5b;
    pub const RWIN: u16 = 0x5c;
    pub const APPS: u16 = 0x5d;
    pub const NUMPAD0: u16 = 0x60;
    pub const NUMPAD9: u16 = 0x69;
    pub const MULTIPLY: u16 = 0x6a;
    pub const ADD: u16 = 0x6b;
    pub const SEPARATOR: u16 = 0x6c;
    pub const SUBTRACT: u16 = 0x6d;
    pub const DECIMAL: u16 = 0x6e;
    pub const DIVIDE: u16 = 0x6f;
    pub const F1: u16 = 0x70;
    pub const F24: u16 = 0x87;
    pub const NUMLOCK: u16 = 0x90;
    pub const SCROLL: u16 = 0x91;
    pub const LSHIFT: u16 = 0xa0;
    pub const RSHIFT: u16 = 0xa1;
    pub const LCONTROL: u16 = 0xa2;
    pub const RCONTROL: u16 = 0xa3;
    pub const LMENU: u16 = 0xa4;
    pub const RMENU: u16 = 0xa5;
    pub const VOLUME_MUTE: u16 = 0xad;
    pub const VOLUME_DOWN: u16 = 0xae;
    pub const VOLUME_UP: u16 = 0xaf;
    pub const MEDIA_NEXT_TRACK: u16 = 0xb0;
    pub const MEDIA_PREV_TRACK: u16 = 0xb1;
    pub const MEDIA_STOP: u16 = 0xb2;
    pub const MEDIA_PLAY_PAUSE: u16 = 0xb3;
    pub const OEM_1: u16 = 0xba;
    pub const OEM_PLUS: u16 = 0xbb;
    pub const OEM_COMMA: u16 = 0xbc;
    pub const OEM_MINUS: u16 = 0xbd;
    pub const OEM_PERIOD: u16 = 0xbe;
    pub const OEM_2: u16 = 0xbf;
    pub const OEM_3: u16 = 0xc0;
    pub const OEM_4: u16 = 0xdb;
    pub const OEM_5: u16 = 0xdc;
    pub const OEM_6: u16 = 0xdd;
    pub const OEM_7: u16 = 0xde;
}

fn win32_vk_to_keycode(vk: u16, cks: u32, last_cks: u32) -> KeyCode {
    use vk_consts as v;
    match vk {
        v::BACK => KeyCode::Backspace,
        v::TAB => KeyCode::Tab,
        v::RETURN => KeyCode::Enter,
        v::SHIFT => {
            // Recover left/right shift identity from the current control-key
            // state (or the previous one on release).
            let pick = |s: u32| {
                if s & WIN32_SHIFT_PRESSED != 0 {
                    if s & WIN32_ENHANCED_KEY != 0 {
                        KeyCode::RightShift
                    } else {
                        KeyCode::LeftShift
                    }
                } else {
                    KeyCode::LeftShift
                }
            };
            if cks & WIN32_SHIFT_PRESSED != 0 {
                pick(cks)
            } else {
                pick(last_cks)
            }
        }
        v::CONTROL => {
            let pick = |s: u32| {
                if s & WIN32_LEFT_CTRL_PRESSED != 0 {
                    Some(KeyCode::LeftCtrl)
                } else if s & WIN32_RIGHT_CTRL_PRESSED != 0 {
                    Some(KeyCode::RightCtrl)
                } else {
                    None
                }
            };
            pick(cks)
                .or_else(|| pick(last_cks))
                .unwrap_or(KeyCode::LeftCtrl)
        }
        v::MENU => {
            let pick = |s: u32| {
                if s & WIN32_LEFT_ALT_PRESSED != 0 {
                    Some(KeyCode::LeftAlt)
                } else if s & WIN32_RIGHT_ALT_PRESSED != 0 {
                    Some(KeyCode::RightAlt)
                } else {
                    None
                }
            };
            pick(cks)
                .or_else(|| pick(last_cks))
                .unwrap_or(KeyCode::LeftAlt)
        }
        v::PAUSE => KeyCode::Pause,
        v::CAPITAL => KeyCode::CapsLock,
        v::ESCAPE => KeyCode::Escape,
        v::SPACE => KeyCode::Space,
        v::PRIOR => KeyCode::PageUp,
        v::NEXT => KeyCode::PageDown,
        v::END => KeyCode::End,
        v::HOME => KeyCode::Home,
        v::LEFT => KeyCode::Left,
        v::UP => KeyCode::Up,
        v::RIGHT => KeyCode::Right,
        v::DOWN => KeyCode::Down,
        v::SNAPSHOT => KeyCode::PrintScreen,
        v::INSERT => KeyCode::Insert,
        v::DELETE => KeyCode::Delete,
        v::LWIN => KeyCode::LeftSuper,
        v::RWIN => KeyCode::RightSuper,
        v::APPS => KeyCode::Menu,
        v::NUMLOCK => KeyCode::NumLock,
        v::SCROLL => KeyCode::ScrollLock,
        v::LSHIFT => KeyCode::LeftShift,
        v::RSHIFT => KeyCode::RightShift,
        v::LCONTROL => KeyCode::LeftCtrl,
        v::RCONTROL => KeyCode::RightCtrl,
        v::LMENU => KeyCode::LeftAlt,
        v::RMENU => KeyCode::RightAlt,
        v::VOLUME_MUTE => KeyCode::VolumeMute,
        v::VOLUME_DOWN => KeyCode::VolumeDown,
        v::VOLUME_UP => KeyCode::VolumeUp,
        v::MEDIA_NEXT_TRACK => KeyCode::MediaNext,
        v::MEDIA_PREV_TRACK => KeyCode::MediaPrev,
        v::MEDIA_STOP => KeyCode::MediaStop,
        v::MEDIA_PLAY_PAUSE => KeyCode::MediaPlayPause,
        v::MULTIPLY => KeyCode::KpMultiply,
        v::ADD => KeyCode::KpAdd,
        v::SEPARATOR => KeyCode::KpSeparator,
        v::SUBTRACT => KeyCode::KpSubtract,
        v::DECIMAL => KeyCode::KpDecimal,
        v::DIVIDE => KeyCode::KpDivide,
        v::OEM_1 => KeyCode::Char(';'),
        v::OEM_PLUS => KeyCode::Char('+'),
        v::OEM_COMMA => KeyCode::Char(','),
        v::OEM_MINUS => KeyCode::Char('-'),
        v::OEM_PERIOD => KeyCode::Char('.'),
        v::OEM_2 => KeyCode::Char('/'),
        v::OEM_3 => KeyCode::Char('`'),
        v::OEM_4 => KeyCode::Char('['),
        v::OEM_5 => KeyCode::Char('\\'),
        v::OEM_6 => KeyCode::Char(']'),
        v::OEM_7 => KeyCode::Char('\''),
        x if (v::F1..=v::F24).contains(&x) => KeyCode::F((x - v::F1 + 1) as u8),
        x if (v::NUMPAD0..=v::NUMPAD9).contains(&x) => match x - v::NUMPAD0 {
            0 => KeyCode::Kp0,
            1 => KeyCode::Kp1,
            2 => KeyCode::Kp2,
            3 => KeyCode::Kp3,
            4 => KeyCode::Kp4,
            5 => KeyCode::Kp5,
            6 => KeyCode::Kp6,
            7 => KeyCode::Kp7,
            8 => KeyCode::Kp8,
            _ => KeyCode::Kp9,
        },
        x if (b'0' as u16..=b'9' as u16).contains(&x) => KeyCode::Char(x as u8 as char),
        x if (b'A' as u16..=b'Z' as u16).contains(&x) => {
            // Lowercase the alpha key — Shift/Caps adjustment happens in
            // the caller via the unicode `ch` field.
            KeyCode::Char((x as u8 + 32) as char)
        }
        _ => KeyCode::Char('\u{0}'),
    }
}
