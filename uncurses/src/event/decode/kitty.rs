//! Kitty keyboard protocol support.
//!
//! See: <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>
//!
//! Format: `CSI code:shifted:base ; mods:event_type ; text-codepoints u`
//!
//! Each semicolon-separated parameter may have colon-separated sub-parameters.

use crate::ansi::params::{Group, Params};
use crate::event::decode::util::{csi_kitty_phase, key_event_for_phase};
use crate::event::{Event, Key, KeyCode, KeyModifiers};

/// Decode a Kitty keyboard CSI u sequence into an [`Event`].
///
/// `params` is the raw parameter body; each `;`-separated group may
/// carry colon-separated sub-parameters.
pub fn decode_kitty_key(params: Params<'_>, _intermediates: &[u8]) -> Option<Event> {
    let code_sub = params.group(0)?;
    if code_sub.is_empty() {
        return None;
    }

    let mod_sub = params.group(1).unwrap_or(Group::EMPTY);
    let text_sub = params.group(2).unwrap_or(Group::EMPTY);

    let keycode = code_sub.first()?;
    let shifted = code_sub.nth(1).filter(|&c| c != 0);
    let base = code_sub.nth(2).filter(|&c| c != 0);

    let modifiers = decode_kitty_modifiers(mod_sub.first().unwrap_or(1));
    let phase = csi_kitty_phase(params);

    let code = kitty_keycode_to_keycode(keycode)?;

    let protocol_text: Option<String> = if !text_sub.is_empty() {
        let s: String = text_sub
            .iter()
            .filter_map(|cp| cp.and_then(char::from_u32))
            .collect();
        if s.is_empty() { None } else { Some(s) }
    } else {
        None
    };

    let mut key = Key::new(code, modifiers);
    if let Some(c) = shifted.and_then(char::from_u32) {
        key.shifted_key = Some(c);
        // Reset any text `Key::new` auto-derived from the un-shifted
        // code so `normalize()` below can re-derive it from the
        // protocol-reported shifted glyph (e.g. Shift+2 → '@').
        key.text = None;
    }
    if let Some(c) = base.and_then(char::from_u32) {
        key.base_key = Some(c);
    }
    if let Some(t) = protocol_text {
        key.text = Some(t);
    }
    // Re-canonicalize: the protocol-reported shifted glyph may now
    // back-fill `text` for inputs without a case variant (e.g. Shift+2
    // producing '@'), which `Key::new` couldn't infer up front.
    key.normalize();

    // Match legacy `CSI Z` semantics: Shift+Tab is reported as BackTab
    // with no Shift modifier, regardless of the encoding path used.
    if key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT) {
        key.code = KeyCode::BackTab;
        key.modifiers.remove(KeyModifiers::SHIFT);
    }

    Some(key_event_for_phase(key, phase))
}

fn decode_kitty_modifiers(m: u32) -> KeyModifiers {
    let m = m.saturating_sub(1); // Kitty uses 1-based modifier encoding
    let mut mods = KeyModifiers::empty();
    if m & 1 != 0 {
        mods |= KeyModifiers::SHIFT;
    }
    if m & 2 != 0 {
        mods |= KeyModifiers::ALT;
    }
    if m & 4 != 0 {
        mods |= KeyModifiers::CTRL;
    }
    if m & 8 != 0 {
        mods |= KeyModifiers::SUPER;
    }
    if m & 16 != 0 {
        mods |= KeyModifiers::HYPER;
    }
    if m & 32 != 0 {
        mods |= KeyModifiers::META;
    }
    if m & 64 != 0 {
        mods |= KeyModifiers::CAPS_LOCK;
    }
    if m & 128 != 0 {
        mods |= KeyModifiers::NUM_LOCK;
    }
    mods
}

fn kitty_keycode_to_keycode(code: u32) -> Option<KeyCode> {
    // Special-case codepoints that have dedicated KeyCode variants.
    match code {
        9 => return Some(KeyCode::Tab),
        13 => return Some(KeyCode::Enter),
        27 => return Some(KeyCode::Escape),
        32 => return Some(KeyCode::Space),
        127 => return Some(KeyCode::Backspace),
        _ => {}
    }

    // Functional/modifier/keypad block (Kitty private use range).
    if let Some(kc) = match code {
        57344 => Some(KeyCode::Escape),
        57345 => Some(KeyCode::Enter),
        57346 => Some(KeyCode::Tab),
        57347 => Some(KeyCode::Backspace),
        57348 => Some(KeyCode::Insert),
        57349 => Some(KeyCode::Delete),
        57350 => Some(KeyCode::Left),
        57351 => Some(KeyCode::Right),
        57352 => Some(KeyCode::Up),
        57353 => Some(KeyCode::Down),
        57354 => Some(KeyCode::PageUp),
        57355 => Some(KeyCode::PageDown),
        57356 => Some(KeyCode::Home),
        57357 => Some(KeyCode::End),
        57358 => Some(KeyCode::CapsLock),
        57359 => Some(KeyCode::ScrollLock),
        57360 => Some(KeyCode::NumLock),
        57361 => Some(KeyCode::PrintScreen),
        57362 => Some(KeyCode::Pause),
        57363 => Some(KeyCode::Menu),
        // F1..F35: 57364..=57398
        57364..=57398 => Some(KeyCode::F((code - 57364 + 1) as u8)),
        // Keypad numeric/operators: 57399..=57427
        57399 => Some(KeyCode::Kp0),
        57400 => Some(KeyCode::Kp1),
        57401 => Some(KeyCode::Kp2),
        57402 => Some(KeyCode::Kp3),
        57403 => Some(KeyCode::Kp4),
        57404 => Some(KeyCode::Kp5),
        57405 => Some(KeyCode::Kp6),
        57406 => Some(KeyCode::Kp7),
        57407 => Some(KeyCode::Kp8),
        57408 => Some(KeyCode::Kp9),
        57409 => Some(KeyCode::KpDecimal),
        57410 => Some(KeyCode::KpDivide),
        57411 => Some(KeyCode::KpMultiply),
        57412 => Some(KeyCode::KpSubtract),
        57413 => Some(KeyCode::KpAdd),
        57414 => Some(KeyCode::KpEnter),
        57415 => Some(KeyCode::KpEqual),
        57416 => Some(KeyCode::KpSeparator),
        57417 => Some(KeyCode::KpLeft),
        57418 => Some(KeyCode::KpRight),
        57419 => Some(KeyCode::KpUp),
        57420 => Some(KeyCode::KpDown),
        57421 => Some(KeyCode::KpPageUp),
        57422 => Some(KeyCode::KpPageDown),
        57423 => Some(KeyCode::KpHome),
        57424 => Some(KeyCode::KpEnd),
        57425 => Some(KeyCode::KpInsert),
        57426 => Some(KeyCode::KpDelete),
        57427 => Some(KeyCode::KpBegin),
        // Media keys: 57428..=57440
        57428 => Some(KeyCode::MediaPlay),
        57429 => Some(KeyCode::MediaPause),
        57430 => Some(KeyCode::MediaPlayPause),
        57431 => Some(KeyCode::MediaReverse),
        57432 => Some(KeyCode::MediaStop),
        57433 => Some(KeyCode::MediaFastForward),
        57434 => Some(KeyCode::MediaRewind),
        57435 => Some(KeyCode::MediaNext),
        57436 => Some(KeyCode::MediaPrev),
        57437 => Some(KeyCode::MediaRecord),
        57438 => Some(KeyCode::VolumeDown),
        57439 => Some(KeyCode::VolumeUp),
        57440 => Some(KeyCode::VolumeMute),
        // Modifier keys: 57441..=57454
        57441 => Some(KeyCode::LeftShift),
        57442 => Some(KeyCode::LeftCtrl),
        57443 => Some(KeyCode::LeftAlt),
        57444 => Some(KeyCode::LeftSuper),
        57445 => Some(KeyCode::LeftHyper),
        57446 => Some(KeyCode::LeftMeta),
        57447 => Some(KeyCode::RightShift),
        57448 => Some(KeyCode::RightCtrl),
        57449 => Some(KeyCode::RightAlt),
        57450 => Some(KeyCode::RightSuper),
        57451 => Some(KeyCode::RightHyper),
        57452 => Some(KeyCode::RightMeta),
        57453 => Some(KeyCode::IsoLevel3Shift),
        57454 => Some(KeyCode::IsoLevel5Shift),
        _ => None,
    } {
        return Some(kc);
    }

    // Printable Unicode codepoints map to Char.
    char::from_u32(code).map(KeyCode::Char)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a raw parameter body from groups of u32 values.
    /// `&[&[97]]` → `b"97"`; `&[&[97, 65, 97], &[2]]` → `b"97:65:97;2"`.
    fn raw(groups: &[&[u32]]) -> Vec<u8> {
        let mut out = Vec::new();
        for (gi, g) in groups.iter().enumerate() {
            if gi > 0 {
                out.push(b';');
            }
            for (si, v) in g.iter().enumerate() {
                if si > 0 {
                    out.push(b':');
                }
                out.extend_from_slice(v.to_string().as_bytes());
            }
        }
        out
    }

    fn decode(groups: &[&[u32]]) -> Event {
        let body = raw(groups);
        decode_kitty_key(Params::from_raw(&body), &[]).unwrap()
    }

    #[test]
    fn test_decode_simple_char() {
        let ev = decode(&[&[97]]);
        let key = ev.as_key().unwrap();
        assert_eq!(key.code, KeyCode::Char('a'));
        assert_eq!(key.modifiers, KeyModifiers::empty());
    }

    #[test]
    fn test_decode_with_modifiers() {
        let ev = decode(&[&[97], &[5]]);
        let key = ev.as_key().unwrap();
        assert_eq!(key.code, KeyCode::Char('a'));
        assert!(key.modifiers.contains(KeyModifiers::CTRL));
    }

    #[test]
    fn test_decode_release() {
        let ev = decode(&[&[97], &[1, 3]]);
        assert!(matches!(ev, Event::KeyRelease(_)));
    }

    #[test]
    fn test_decode_repeat() {
        let ev = decode(&[&[97], &[1, 2]]);
        assert!(matches!(ev, Event::KeyRepeat(_)));
    }

    #[test]
    fn test_decode_with_shifted_and_base() {
        let ev = decode(&[&[97, 65, 97], &[2]]);
        let key = ev.as_key().unwrap();
        assert_eq!(key.shifted_key, Some('A'));
        assert_eq!(key.base_key, Some('a'));
    }

    #[test]
    fn test_decode_with_text_codepoints() {
        let ev = decode(&[&[97], &[1], &[72, 105]]);
        let key = ev.as_key().unwrap();
        assert_eq!(key.text.as_deref(), Some("Hi"));
    }

    #[test]
    fn test_decode_plain_lowercase_populates_text() {
        // No REPORT_ASSOCIATED_TEXT param group, no shifted/base —
        // text still surfaces the typed glyph for printable input.
        let ev = decode(&[&[97]]);
        let key = ev.as_key().unwrap();
        assert_eq!(key.code, KeyCode::Char('a'));
        assert_eq!(key.text.as_deref(), Some("a"));
    }

    #[test]
    fn test_decode_shift_digit_uses_reported_shifted_glyph_for_text() {
        // Shift+2 on US: keycode 50 ('2'), shifted 64 ('@'), mods Shift.
        // Without the terminal-reported shifted glyph we couldn't know
        // text should be "@" because '2' has no case variant.
        let ev = decode(&[&[50, 64], &[2]]);
        let key = ev.as_key().unwrap();
        assert_eq!(key.code, KeyCode::Char('2'));
        assert_eq!(key.shifted_key, Some('@'));
        assert_eq!(key.text.as_deref(), Some("@"));
    }

    #[test]
    fn test_decode_ctrl_letter_no_text() {
        // Ctrl+a: text must stay None (Ctrl suppresses typed input).
        let ev = decode(&[&[97], &[5]]);
        let key = ev.as_key().unwrap();
        assert_eq!(key.code, KeyCode::Char('a'));
        assert!(key.modifiers.contains(KeyModifiers::CTRL));
        assert!(key.text.is_none());
    }

    #[test]
    fn test_decode_f1() {
        let ev = decode(&[&[57364]]);
        assert_eq!(ev.as_key().unwrap().code, KeyCode::F(1));
    }

    #[test]
    fn test_decode_f12() {
        let ev = decode(&[&[57375]]);
        assert_eq!(ev.as_key().unwrap().code, KeyCode::F(12));
    }

    #[test]
    fn test_decode_enter() {
        let ev = decode(&[&[13]]);
        assert_eq!(ev.as_key().unwrap().code, KeyCode::Enter);
    }

    #[test]
    fn test_decode_escape() {
        let ev = decode(&[&[27]]);
        assert_eq!(ev.as_key().unwrap().code, KeyCode::Escape);
    }
}
