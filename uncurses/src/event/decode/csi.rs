//! CSI (Control Sequence Introducer) decoder.
//!
//! The largest single class of escape sequences. Format:
//! `ESC [` (or 8-bit `0x9B`) followed by optional parameter bytes
//! (`0x30..0x3f`), optional intermediate bytes (`0x20..0x2f`), and a
//! single final byte (`0x40..0x7e`).
//!
//! This module owns:
//!
//! * `parse_csi`    — walks the parameter / intermediate / final-byte
//!   skeleton and delegates to `dispatch_csi`.
//! * `dispatch_csi` — runs the state-dependent inline paths (X10/UTF-8
//!   mouse, Win32 input) that need access to `&Decoder` and bytes past
//!   the sequence, then hands off to user hooks and `recognize`.
//! * `recognize` / `recognize_tilde` — pure builtin decoders that turn
//!   a parsed CSI into the corresponding [`Event`], or `None` so a
//!   caller can fall back to `Event::UnknownCsi`.

use super::Decoder;
use super::DecoderFlags;
use super::kitty;
use super::result::ParseResult;
use super::util::{
    Params, csi_kitty_phase, csi_modifiers, intro_prefix_len, key_event_for_phase, key_with_mods,
    lookup_legacy_key, remap_tilde, tilde_code_to_keycode, xterm_modifiers,
};
use crate::event::mouse::{
    decode_sgr_mouse, decode_urxvt_mouse, decode_utf8_mouse, decode_x10_mouse,
};
use crate::event::{Event, Key, KeyCode, KeyModifiers};

impl Decoder {
    pub(super) fn parse_csi(&self, buf: &[u8]) -> ParseResult {
        let prefix_len = intro_prefix_len(buf[0]);
        // CSI body starts at buf[prefix_len..]; need at least the final byte.
        if buf.len() < prefix_len + 1 {
            return ParseResult::Incomplete;
        }

        // Find the final byte (0x40-0x7e)
        let mut i = prefix_len;
        let mut has_private = false;
        if i < buf.len() && (buf[i] == b'?' || buf[i] == b'<' || buf[i] == b'>' || buf[i] == b'=') {
            has_private = true;
            i += 1;
        }

        while i < buf.len() {
            let b = buf[i];
            if (0x40..=0x7e).contains(&b) {
                // Found final byte
                let seq = &buf[prefix_len..=i];
                let consumed = i + 1;
                return self.dispatch_csi(buf, seq, has_private, consumed);
            }
            // URxvt-style: `$` after digits/separators is treated as a final byte
            // (e.g. `\x1b[23$` for Shift+F11). Skip this when `$` is followed by
            // another final byte (e.g. DECRPM `\x1b[?1;1$y`), in which case `$`
            // is a real intermediate.
            if b == b'$' && !has_private {
                // URxvt: `\x1b[23$` (Shift+F11) — `$` is a final byte when
                // there's no private prefix. With a private prefix (e.g.
                // DECRPM `\x1b[?1;1$y`), `$` remains an intermediate.
                let seq = &buf[prefix_len..=i];
                let consumed = i + 1;
                return self.dispatch_csi(buf, seq, has_private, consumed);
            }
            if !((0x30..=0x3f).contains(&b) || b == b';' || b == b':') {
                // Intermediate byte
                if !(0x20..=0x2f).contains(&b) {
                    // Invalid
                    return ParseResult::None(i + 1);
                }
            }
            i += 1;
        }

        ParseResult::Incomplete
    }

    pub(super) fn dispatch_csi(
        &self,
        buf: &[u8],
        seq: &[u8],
        has_private: bool,
        consumed: usize,
    ) -> ParseResult {
        let view = super::handlers::split_csi(seq);
        let final_byte = view.final_byte;

        // X10 / UTF-8 mouse: CSI M followed by 3 raw bytes (or 3 UTF-8
        // codepoints in mode 1005). Needs access to the input buffer
        // beyond `consumed`, which can't be expressed via the hook API,
        // so it stays inline above hook dispatch. The byte-level
        // `params_raw.is_empty()` check avoids a parameter allocation
        // on every CSI.
        if !has_private && final_byte == b'M' && view.params_raw.is_empty() {
            if self.utf8_mouse {
                if let Some((event, n)) = decode_utf8_mouse(&buf[consumed..]) {
                    return ParseResult::Event(event, consumed + n);
                }
                return ParseResult::Incomplete;
            }
            if buf.len() >= consumed + 3 {
                let cb = buf[consumed];
                let cx = buf[consumed + 1];
                let cy = buf[consumed + 2];
                if let Some(event) = decode_x10_mouse(cb, cx, cy) {
                    return ParseResult::Event(event, consumed + 3);
                }
            }
            return ParseResult::Incomplete;
        }

        // Win32 input mode: CSI vk;sc;ch;kd;cks;rep _ . Mutates
        // `self.pending` so it stays inline above the hook layer. Pre-check
        // the body shape (5 `;` separators, no private prefix) before
        // allocating to parse the parameter list.
        if final_byte == b'_'
            && !has_private
            && view.params_raw.iter().filter(|&&b| b == b';').count() == 5
        {
            return self.dispatch_win32_input(view.params(), consumed);
        }

        // User hooks first (can override recognised defaults), then the
        // builtin recogniser, then `Event::UnknownCsi`. The lazy
        // [`Params`] walker is stateless and cheap to re-create across
        // hooks and the builtin recogniser.
        let raw_with_intro = &buf[..consumed];
        let evt = self
            .handlers
            .dispatch_csi(&view)
            .or_else(|| recognize(&view, raw_with_intro, self.flags))
            .unwrap_or_else(|| Event::UnknownCsi(raw_with_intro.to_vec()));
        ParseResult::Event(evt, consumed)
    }
}

/// Builtin CSI recogniser. Pure: produces an [`Event`] from a fully-parsed
/// CSI view, or `None` when the sequence isn't one this library knows
/// about. `raw_with_intro` is the original byte slice including the
/// `ESC [` introducer, used by the legacy-key lookup table.
fn recognize(
    view: &super::handlers::Csi<'_>,
    raw_with_intro: &[u8],
    flags: DecoderFlags,
) -> Option<Event> {
    let final_byte = view.final_byte;
    let params = view.params();
    // Currently the spec uses at most one intermediate; expose it as
    // `Option<u8>` to mirror the previous single-byte detection.
    let intermediate = view.intermediates.last().copied();
    let no_private = view.private.is_none();
    let no_intermediate = intermediate.is_none();

    // URxvt mouse: CSI Cb;Cx;Cy M  (no `<` prefix, semicolon params,
    // no intermediate, exactly 3 params)
    if no_private
        && no_intermediate
        && final_byte == b'M'
        && params.len() == 3
        && let Some(event) = decode_urxvt_mouse(params)
    {
        return Some(event);
    }

    // SGR mouse: CSI < Cb ; Cx ; Cy M/m. Coordinates are always reported
    // 0-based regardless of encoding; callers using SGR-Pixel (1016)
    // interpret the result as pixel offsets. Exactly 3 params, no
    // intermediate.
    if view.private == Some(b'<')
        && no_intermediate
        && (final_byte == b'M' || final_byte == b'm')
        && params.len() == 3
    {
        let is_release = final_byte == b'm';
        if let Some(event) = decode_sgr_mouse(params, is_release) {
            return Some(event);
        }
    }

    // Kitty keyboard: CSI ... u  (sub-parameter aware). No private byte,
    // no intermediate. Param count is variable.
    if final_byte == b'u'
        && no_private
        && no_intermediate
        && let Some(ev) = kitty::decode_kitty_key(view.params(), &[])
    {
        return Some(ev);
    }

    // Focus events: CSI I / CSI O. No private, no intermediate, no params.
    if final_byte == b'I' && no_private && no_intermediate && params.is_empty() {
        return Some(Event::FocusIn);
    }
    if final_byte == b'O' && no_private && no_intermediate && params.is_empty() {
        return Some(Event::FocusOut);
    }

    // Cursor position report: CSI row ; col R. No private, no
    // intermediate, exactly 2 params. Ambiguous with the modified-F3
    // form `CSI 1 ; <mod> R` when the cursor is at row 1 (column - 1
    // fits in the 4-bit modifier mask). In that case emit both events
    // as a Multi and let the consumer decide.
    if final_byte == b'R' && no_private && no_intermediate && params.len() == 2 {
        let row = params.get_or(0, 1).max(1);
        let col = params.get_or(1, 1).max(1);
        let cpr = Event::CursorPosition(crate::Position::new((col - 1) as u16, (row - 1) as u16));
        if row == 1 && col >= 1 && col - 1 <= 15 {
            let mods = xterm_modifiers(col as u16);
            let f3 = Event::KeyPress(Key::new(KeyCode::F(3), mods).normalized());
            return Some(Event::Multi(vec![f3, cpr]));
        }
        return Some(cpr);
    }

    // Light/dark color scheme report: CSI ? 997 ; {1|2} n.
    // Sent both as reply to `CSI ? 996 n` and unsolicited when DEC mode
    // 2031 is enabled. Exactly two params and no intermediate.
    if final_byte == b'n'
        && view.private == Some(b'?')
        && no_intermediate
        && params.len() == 2
        && params.get_or(0, 0) == 997
    {
        return match params.get_or(1, 0) {
            1 => Some(Event::DarkColorScheme),
            2 => Some(Event::LightColorScheme),
            _ => None,
        };
    }

    // DA1 response: CSI ? ... c. No intermediate.
    if final_byte == b'c' && view.private == Some(b'?') && no_intermediate {
        return Some(Event::PrimaryDeviceAttributes(
            params.iter().map(|g| g.first()).collect(),
        ));
    }

    // DA2 response: CSI > ... c. No intermediate.
    if final_byte == b'c' && view.private == Some(b'>') && no_intermediate {
        return Some(Event::SecondaryDeviceAttributes(
            params.iter().map(|g| g.first()).collect(),
        ));
    }

    // Mode report: CSI ? Ps ; Pm $ y (DECRPM) or CSI Ps ; Pm $ y
    // (RM-style). Intermediate `$`, exactly 2 params.
    if final_byte == b'y' && intermediate == Some(b'$') && params.len() == 2 {
        let mode_n = params.get_or(0, 0) as u16;
        let setting_n = params.get_or(1, 0) as u16;
        let mode = if no_private {
            crate::ansi::mode::Mode::Ansi(mode_n)
        } else {
            crate::ansi::mode::Mode::Dec(mode_n)
        };
        let setting = crate::ansi::mode::ModeSetting::from_value(setting_n);
        return Some(Event::ModeReport { mode, setting });
    }

    // Kitty keyboard protocol active-enhancements report: CSI ? flags u.
    // No intermediate, exactly 1 param.
    if final_byte == b'u' && view.private == Some(b'?') && no_intermediate && params.len() == 1 {
        let bits = params.get_or(0, 0) as u8;
        return Some(Event::KittyKeyboardEnhancements(
            crate::ansi::KittyKeyboardFlags::from_bits_truncate(bits),
        ));
    }

    // Window size in pixels: CSI 4 ; height ; width t. No private, no
    // intermediate, exactly 3 params.
    if final_byte == b't'
        && no_private
        && no_intermediate
        && params.len() == 3
        && params.get_or(0, 0) == 4
    {
        return Some(Event::WindowPixelSize {
            width: params.get_or(2, 0) as u16,
            height: params.get_or(1, 0) as u16,
        });
    }

    // Cell size in pixels: CSI 6 ; height ; width t. No private, no
    // intermediate, exactly 3 params.
    if final_byte == b't'
        && no_private
        && no_intermediate
        && params.len() == 3
        && params.get_or(0, 0) == 6
    {
        return Some(Event::CellPixelSize {
            width: params.get_or(2, 0) as u16,
            height: params.get_or(1, 0) as u16,
        });
    }

    // Window size in chars: CSI 8 ; height ; width t — reply to the
    // application's CSI 18 t query. This is a query reply, not a change
    // notification; surface it as such. No private, no intermediate,
    // exactly 3 params.
    if final_byte == b't'
        && no_private
        && no_intermediate
        && params.len() == 3
        && params.get_or(0, 0) == 8
    {
        return Some(Event::WindowCellSize {
            width: params.get_or(2, 0) as u16,
            height: params.get_or(1, 0) as u16,
        });
    }

    // In-band resize report (mode 2048):
    // CSI 48 ; height_chars ; width_chars ; height_pix ; width_pix t.
    // No private, no intermediate, exactly 5 params.
    if final_byte == b't'
        && no_private
        && no_intermediate
        && params.len() == 5
        && params.get_or(0, 0) == 48
    {
        return Some(Event::Resize(crate::terminal::size::Winsize {
            row: params.get_or(1, 0) as u16,
            col: params.get_or(2, 0) as u16,
            ypixel: params.get_or(3, 0) as u16,
            xpixel: params.get_or(4, 0) as u16,
        }));
    }

    // Generic window-op fallback for other CSI t variants. No private,
    // no intermediate.
    if final_byte == b't' && no_private && no_intermediate && !params.is_empty() {
        return Some(Event::WindowOp {
            op: params.get_or(0, 0),
            args: params.slice_from(1).iter().map(|g| g.first()).collect(),
        });
    }

    // modifyOtherKeys report: CSI > 4 ; Pn m. No intermediate, exactly
    // 2 params.
    if final_byte == b'm'
        && view.private == Some(b'>')
        && no_intermediate
        && params.len() == 2
        && params.get_or(0, 0) == 4
    {
        return Some(Event::ModifyOtherKeys(
            crate::event::ModifyOtherKeysMode::from_value(params.get_or(1, 0) as u8),
        ));
    }

    // Standard final-byte cursor / function-key dispatch. No private,
    // no intermediate.
    if no_private && no_intermediate {
        let key_code = match final_byte {
            b'A' => Some(KeyCode::Up),
            b'B' => Some(KeyCode::Down),
            b'C' => Some(KeyCode::Right),
            b'D' => Some(KeyCode::Left),
            b'E' => Some(KeyCode::KpBegin),
            b'H' => Some(KeyCode::Home),
            b'F' => Some(KeyCode::End),
            b'P' => Some(KeyCode::F(1)),
            b'Q' => Some(KeyCode::F(2)),
            b'S' => Some(KeyCode::F(4)),
            _ => None,
        };
        if let Some(kc) = key_code {
            let key = key_with_mods(kc, csi_modifiers(params));
            return Some(key_event_for_phase(key, csi_kitty_phase(params)));
        }
        match final_byte {
            b'Z' => {
                // Legacy single-byte spelling for Shift+Tab. Emit the
                // uniform `Tab + SHIFT` identity directly.
                let key = Key::new(KeyCode::Tab, KeyModifiers::SHIFT).normalized();
                return Some(key_event_for_phase(key, csi_kitty_phase(params)));
            }
            b'~' => {
                if let Some(ev) = recognize_tilde(params, flags) {
                    return Some(ev);
                }
            }
            _ => {}
        }
    }

    // Last resort before falling back to `Event::UnknownCsi`: the legacy
    // key table for terminals (like URxvt) that use the bare sequence as
    // the identifier.
    lookup_legacy_key(raw_with_intro, flags).map(Event::KeyPress)
}

/// Builtin handler for the `CSI Pn ~` family (PgUp, F-keys 5+,
/// bracketed-paste boundaries, …).
fn recognize_tilde(params: Params<'_>, flags: DecoderFlags) -> Option<Event> {
    let code = params.get_or(0, 0);

    // XTerm modifyOtherKeys-2: CSI 27 ; <modifier> ; <code> ~
    if code == 27 && params.len() >= 3 {
        let mods = xterm_modifiers(params.get_or(1, 1) as u16);
        let r = params.get_or(2, 0);
        let key_code = match r {
            0x08 => KeyCode::Backspace,
            0x09 => KeyCode::Tab,
            0x0d => KeyCode::Enter,
            0x1b => KeyCode::Escape,
            0x20 => KeyCode::Space,
            0x7f => KeyCode::Backspace,
            _ => match char::from_u32(r) {
                Some(c) => KeyCode::Char(c),
                None => return None,
            },
        };
        let key = Key::new(key_code, mods).normalized();
        return Some(Event::KeyPress(key));
    }

    let mods = csi_modifiers(params);

    if code == 200 {
        return Some(Event::PasteStart);
    }
    if code == 201 {
        return Some(Event::PasteEnd);
    }
    let key_code = u16::try_from(code)
        .ok()
        .and_then(tilde_code_to_keycode)
        .map(|kc| remap_tilde(code as u16, kc, flags))?;

    Some(key_event_for_phase(
        key_with_mods(key_code, mods),
        csi_kitty_phase(params),
    ))
}
