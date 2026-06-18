//! Windows-specific construction and event-pump for [`EventSource`].
//!
//! Provides the Windows `impl` block on the unified
//! [`super::source::EventSource`] type: [`EventSource::new`], the
//! `WaitForMultipleObjects`-backed [`EventSource::try_read`], and the
//! `INPUT_RECORD` → VT-byte serialiser. Resize events arrive as
//! `WINDOW_BUFFER_SIZE_EVENT` records on the input handle, so no
//! sibling pipe / signal plumbing is needed on this target.
//!
//! Encoding rules for [`Self::serialize_record`]:
//!
//! * `KEY_EVENT` with VT input mode active and a non-zero `UnicodeChar`:
//!   emit the UTF-8 encoding of the (possibly surrogate-paired) code
//!   point. Non-VT input emits the win32-input-mode sequence
//!   `CSI vk;sc;ch;kd;cks;rep _` for downstream handling.
//! * `MOUSE_EVENT`: emit an SGR mouse sequence.
//! * `WINDOW_BUFFER_SIZE_EVENT`: emit `CSI 48 ; rows ; cols ; 0 ; 0 t`
//!   (pixel dimensions are unavailable on this target). The decoder
//!   turns it into [`Event::Resize`].
//! * `FOCUS_EVENT`: emit `CSI I` (focus in) or `CSI O` (focus out).
//! * `MENU_EVENT`: ignored.

#![cfg(windows)]

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Console::{
    ENABLE_VIRTUAL_TERMINAL_INPUT, FOCUS_EVENT, GetConsoleMode, GetNumberOfConsoleInputEvents,
    INPUT_RECORD, KEY_EVENT, LEFT_ALT_PRESSED, LEFT_CTRL_PRESSED, MENU_EVENT, MOUSE_EVENT,
    MOUSE_HWHEELED, MOUSE_MOVED, MOUSE_WHEELED, RIGHT_ALT_PRESSED, RIGHT_CTRL_PRESSED,
    ReadConsoleInputW, SHIFT_PRESSED, WINDOW_BUFFER_SIZE_EVENT,
};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent};

use super::decode::{Decoder, DecoderFlags};
use super::source::{
    DEFAULT_BUFFER_CAPACITY, DEFAULT_ESC_TIMEOUT, DEFAULT_PASTE_IDLE_TIMEOUT, DeadlineKind,
    EventSource, Input, Observers, Waker,
};

const READ_BATCH: usize = 64;

/// Inner waker state. Owns a Win32 `Event` HANDLE that signals the
/// cancellation slot of [`WaitForMultipleObjects`]. The `HANDLE` value
/// is also copied into [`EventSource::wake_event`] so the wait array
/// can reference it without an `Arc` indirection; ownership remains
/// with the waker (its `Drop` calls [`CloseHandle`]).
pub(super) struct WindowsWakerInner {
    event: HANDLE,
}

impl WindowsWakerInner {
    pub(super) fn wake(&self) -> io::Result<()> {
        if unsafe { SetEvent(self.event) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for WindowsWakerInner {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.event);
        }
    }
}

// SAFETY: the underlying HANDLE is reference-counted by the kernel and
// SetEvent / CloseHandle are thread-safe.
unsafe impl Send for WindowsWakerInner {}
unsafe impl Sync for WindowsWakerInner {}

// SAFETY: the contained HANDLE in `EventSource::wake_event` is a kernel
// object, not a pointer into thread-local memory; the underlying read
// handle is also thread-safe to use.
unsafe impl<I> Send for EventSource<I> where I: Input {}

impl<I> EventSource<I>
where
    I: Input,
{
    /// Build a new source reading from `input`. Resize events arrive
    /// as `WINDOW_BUFFER_SIZE_EVENT` records on the input handle, so
    /// no separate winch plumbing is needed.
    ///
    /// Timeouts start at their documented defaults; override them with
    /// [`EventSource::with_esc_timeout`] and
    /// [`EventSource::with_paste_idle_timeout`].
    pub fn new(input: I) -> io::Result<Self> {
        let input_handle = {
            use std::os::windows::io::AsRawHandle;
            input.as_handle().as_raw_handle() as HANDLE
        };

        let wake_event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if wake_event.is_null() {
            return Err(io::Error::last_os_error());
        }
        let waker = Waker::from_windows_inner(Arc::new(WindowsWakerInner { event: wake_event }));

        let mut mode: u32 = 0;
        let vt_input = unsafe { GetConsoleMode(input_handle, &mut mode) } != 0
            && (mode & ENABLE_VIRTUAL_TERMINAL_INPUT) != 0;

        // Watch input and wake handles — in the fixed index order the
        // wait path relies on. `input_is_tty` is meaningless on Windows
        // and ignored by the poller there.
        use std::os::windows::io::RawHandle;
        let fds = [
            input_handle as *mut _ as RawHandle,
            wake_event as *mut _ as RawHandle,
        ];
        let poller = super::poll::new_poller(&fds, false)?;

        Ok(Self {
            input,
            parser: Decoder::new(DecoderFlags::empty()),
            pending: super::pending::Pending::with_capacity(DEFAULT_BUFFER_CAPACITY),
            esc_timeout: DEFAULT_ESC_TIMEOUT,
            esc_deadline: None,
            paste_idle_timeout: Some(DEFAULT_PASTE_IDLE_TIMEOUT),
            paste_deadline: None,
            queue: VecDeque::with_capacity(16),
            waker,
            handle_resize: true,
            observers: Observers::default(),
            poller,
            wake_event,
            vt_input,
            pending_high_surrogate: [None, None],
            last_size: None,
            last_mouse_buttons: 0,
        })
    }

    pub(super) fn pump(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        let (effective, kind) = self.effective_timeout(timeout);
        let mut ready = [false; 2];
        self.poller.poll(&mut ready, effective)?;
        self.ingest(ready, kind)
    }

    /// Act on a readiness result `[input, wake]` (the poller's fixed
    /// index order): surface a wake as `Interrupted`, drain ready console
    /// records, or run a deadline expiry when nothing was ready. Performs
    /// no blocking wait of its own, so it is safe to call under a lock
    /// after a separate lock-free [`Poller::poll`] — the path an
    /// [`super::EventStream`] reader thread takes. Resize arrives as a
    /// console record, so there is no separate winch fd here.
    pub(super) fn ingest(&mut self, ready: [bool; 2], kind: DeadlineKind) -> io::Result<()> {
        let input_ready = ready[0];
        let wake_ready = ready[1];

        if wake_ready {
            unsafe {
                ResetEvent(self.wake_event);
            }
            return Err(io::Error::new(io::ErrorKind::Interrupted, "wake"));
        }

        if input_ready {
            self.read_records()?;
        } else {
            match kind {
                DeadlineKind::Esc => self.expire_partial(),
                DeadlineKind::Paste => self.expire_paste(),
                DeadlineKind::None => {}
            }
        }

        Ok(())
    }

    /// Re-check readiness with a zero-timeout poll and ingest whatever is
    /// ready, all under the caller's lock — the reader-thread path (see
    /// the unix counterpart for the rationale). Deadline expiry is the
    /// caller's responsibility (see [`EventSource::expire`]).
    pub(super) fn drain_after_wait(&mut self) -> io::Result<()> {
        let mut ready = [false; 2];
        self.poller.poll(&mut ready, Some(Duration::ZERO))?;
        self.ingest(ready, DeadlineKind::None)
    }

    fn input_handle(&self) -> HANDLE {
        use std::os::windows::io::AsRawHandle;
        self.input.as_handle().as_raw_handle() as HANDLE
    }

    fn read_records(&mut self) -> io::Result<()> {
        let mut available: u32 = 0;
        if unsafe { GetNumberOfConsoleInputEvents(self.input_handle(), &mut available) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if available == 0 {
            return Ok(());
        }
        let to_read = (available as usize).min(READ_BATCH);
        let mut records: [INPUT_RECORD; READ_BATCH] = unsafe { std::mem::zeroed() };
        let mut read_n: u32 = 0;
        let ok = unsafe {
            ReadConsoleInputW(
                self.input_handle(),
                records.as_mut_ptr(),
                to_read as u32,
                &mut read_n,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }

        let mut bytes: Vec<u8> = Vec::with_capacity(read_n as usize * 8);
        for record in &records[..read_n as usize] {
            self.serialize_record(record, &mut bytes);
        }
        if !bytes.is_empty() {
            self.feed_bytes(&bytes);
        }
        Ok(())
    }

    fn feed_bytes(&mut self, bytes: &[u8]) {
        #[cfg(debug_assertions)]
        crate::trace::tee_input(bytes);
        let cap = self.pending.capacity();
        if self.pending.len().saturating_add(bytes.len()) > cap {
            self.pending.clear();
            self.esc_deadline = None;
            if bytes.len() > cap {
                return;
            }
        }
        self.pending.append(bytes);
        self.drain_parser();
    }

    fn serialize_record(&mut self, record: &INPUT_RECORD, buf: &mut Vec<u8>) {
        match record.EventType as u32 {
            KEY_EVENT => {
                let kev = unsafe { record.Event.KeyEvent };
                serialize_key_event(&kev, self.vt_input, &mut self.pending_high_surrogate, buf);
            }
            MOUSE_EVENT => {
                let mev = unsafe { record.Event.MouseEvent };
                serialize_mouse_event(&mev, &mut self.last_mouse_buttons, buf);
            }
            WINDOW_BUFFER_SIZE_EVENT => {
                let wev = unsafe { record.Event.WindowBufferSizeEvent };
                serialize_resize_event(&wev, &mut self.last_size, buf);
            }
            FOCUS_EVENT => {
                let fev = unsafe { record.Event.FocusEvent };
                serialize_focus_event(&fev, buf);
            }
            MENU_EVENT => {
                // ignore
            }
            _ => {}
        }
    }
}

fn serialize_key_event(
    kev: &windows_sys::Win32::System::Console::KEY_EVENT_RECORD,
    vt_input: bool,
    pending_high_surrogate: &mut [Option<u16>; 2],
    buf: &mut Vec<u8>,
) {
    let key_down = kev.bKeyDown != 0;
    let unicode_char = unsafe { kev.uChar.UnicodeChar };
    let kd_idx: usize = if key_down { 1 } else { 0 };

    if vt_input {
        if let Some(high) = pending_high_surrogate[kd_idx].take() {
            if let Some(Ok(c)) = char::decode_utf16([high, unicode_char]).next() {
                push_utf8(buf, c);
            }
        } else if (0xD800..=0xDBFF).contains(&unicode_char) {
            pending_high_surrogate[kd_idx] = Some(unicode_char);
        } else if (0xDC00..=0xDFFF).contains(&unicode_char) {
            // Lone low surrogate — drop.
        } else if key_down
            && unicode_char != 0
            && let Some(c) = char::from_u32(unicode_char as u32)
        {
            push_utf8(buf, c);
        }
    } else if kev.wVirtualKeyCode == 0 {
        if key_down
            && unicode_char != 0
            && let Some(c) = char::from_u32(unicode_char as u32)
        {
            push_utf8(buf, c);
        }
    } else {
        let _ = write!(
            buf,
            "\x1b[{};{};{};{};{};{}_",
            kev.wVirtualKeyCode,
            kev.wVirtualScanCode,
            unicode_char,
            if key_down { 1 } else { 0 },
            kev.dwControlKeyState,
            kev.wRepeatCount
        );
    }
}

fn serialize_mouse_event(
    mev: &windows_sys::Win32::System::Console::MOUSE_EVENT_RECORD,
    last_mouse_buttons: &mut u32,
    buf: &mut Vec<u8>,
) {
    let alt = (mev.dwControlKeyState & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED)) != 0;
    let ctrl = (mev.dwControlKeyState & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED)) != 0;
    let shift = (mev.dwControlKeyState & SHIFT_PRESSED) != 0;
    let wheel_delta = high_word_signed(mev.dwButtonState);

    let (cb, is_release, is_move) = match mev.dwEventFlags {
        0 => {
            let (button, release) = mouse_button_change(*last_mouse_buttons, mev.dwButtonState);
            (button, release, false)
        }
        MOUSE_MOVED => {
            let (button, _) = mouse_button_change(*last_mouse_buttons, mev.dwButtonState);
            (button, false, true)
        }
        MOUSE_WHEELED => {
            let cb = if wheel_delta > 0 { 64 } else { 65 };
            (Some(cb), false, false)
        }
        MOUSE_HWHEELED => {
            let cb = if wheel_delta > 0 { 67 } else { 66 };
            (Some(cb), false, false)
        }
        _ => (None, false, false),
    };
    *last_mouse_buttons = mev.dwButtonState;
    let Some(mut cb) = cb else { return };
    if is_move {
        cb |= 32;
    }
    if shift {
        cb |= 4;
    }
    if alt {
        cb |= 8;
    }
    if ctrl {
        cb |= 16;
    }
    let x = mev.dwMousePosition.X as i32 + 1;
    let y = mev.dwMousePosition.Y as i32 + 1;
    let final_byte = if is_release { 'm' } else { 'M' };
    let _ = write!(buf, "\x1b[<{cb};{x};{y}{final_byte}");
}

fn serialize_resize_event(
    wev: &windows_sys::Win32::System::Console::WINDOW_BUFFER_SIZE_RECORD,
    last_size: &mut Option<(i16, i16)>,
    buf: &mut Vec<u8>,
) {
    let (rows, cols) = (wev.dwSize.Y, wev.dwSize.X);
    if *last_size != Some((rows, cols)) {
        *last_size = Some((rows, cols));
        // Pixel dimensions are unavailable on this target.
        let _ = write!(buf, "\x1b[48;{rows};{cols};0;0t");
    }
}

fn serialize_focus_event(
    fev: &windows_sys::Win32::System::Console::FOCUS_EVENT_RECORD,
    buf: &mut Vec<u8>,
) {
    if fev.bSetFocus != 0 {
        buf.extend_from_slice(b"\x1b[I");
    } else {
        buf.extend_from_slice(b"\x1b[O");
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn push_utf8(buf: &mut Vec<u8>, c: char) {
    buf.extend_from_slice(c.encode_utf8(&mut [0u8; 4]).as_bytes());
}

fn high_word_signed(v: u32) -> i16 {
    ((v >> 16) & 0xFFFF) as i16
}

/// Diff the previous and current button bitmasks into a single
/// SGR-style button code (left=0, middle=1, right=2) plus a "released"
/// flag. If multiple buttons changed in one record, report the first
/// change in L/R/M order to match what most terminals emit.
fn mouse_button_change(prev: u32, current: u32) -> (Option<u8>, bool) {
    use windows_sys::Win32::System::Console::{
        FROM_LEFT_1ST_BUTTON_PRESSED, FROM_LEFT_2ND_BUTTON_PRESSED, RIGHTMOST_BUTTON_PRESSED,
    };
    let table: [(u32, u8); 3] = [
        (FROM_LEFT_1ST_BUTTON_PRESSED, 0),
        (RIGHTMOST_BUTTON_PRESSED, 2),
        (FROM_LEFT_2ND_BUTTON_PRESSED, 1),
    ];
    for (mask, code) in table.iter() {
        let was = (prev & mask) != 0;
        let now = (current & mask) != 0;
        if was != now {
            return (Some(*code), was);
        }
    }
    if current != 0 {
        for (mask, code) in table.iter() {
            if (current & mask) != 0 {
                return (Some(*code), false);
            }
        }
    }
    (None, false)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Console::{
        COORD, FOCUS_EVENT_RECORD, KEY_EVENT_RECORD, KEY_EVENT_RECORD_0, MOUSE_EVENT_RECORD,
        WINDOW_BUFFER_SIZE_RECORD,
    };

    #[test]
    fn surrogate_decoding_roundtrip() {
        // U+1F600 -> high D83D, low DE00.
        let c = char::decode_utf16([0xD83Du16, 0xDE00])
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(c as u32, 0x1F600);
        // A non-surrogate followed by a lone low surrogate does not pair.
        let mut iter = char::decode_utf16([0x0041u16, 0xDE00]);
        assert_eq!(iter.next().unwrap().unwrap(), 'A');
        assert!(iter.next().unwrap().is_err());
    }

    #[test]
    fn high_word_signed_extracts_signed_wheel_delta() {
        // 0x00780000 -> high word 0x0078 (+120, one notch up)
        assert_eq!(high_word_signed(0x0078_0000), 120);
        // 0xFF880000 -> high word 0xFF88 (-120, one notch down)
        assert_eq!(high_word_signed(0xFF88_0000), -120);
        assert_eq!(high_word_signed(0), 0);
    }

    #[test]
    fn push_utf8_encodes_ascii_and_supplementary() {
        let mut buf = Vec::new();
        push_utf8(&mut buf, 'A');
        assert_eq!(buf, b"A");

        let mut buf = Vec::new();
        push_utf8(&mut buf, '\u{1F600}');
        // 4-byte UTF-8 encoding of U+1F600
        assert_eq!(buf, [0xF0, 0x9F, 0x98, 0x80]);
    }

    #[test]
    fn mouse_button_change_press_and_release() {
        // Press left button (bit 0).
        let (cb, release) = mouse_button_change(0, 0b0000_0001);
        assert_eq!(cb, Some(0));
        assert!(!release);

        // Release left button.
        let (cb, release) = mouse_button_change(0b0000_0001, 0);
        assert_eq!(cb, Some(0));
        assert!(release);

        // Press right button (bit 1 in Windows mapping).
        let (cb, release) = mouse_button_change(0, 0b0000_0010);
        assert_eq!(cb, Some(2));
        assert!(!release);
    }

    #[test]
    fn mouse_button_change_no_change() {
        // No buttons held, no change -> nothing to report.
        assert_eq!(mouse_button_change(0, 0), (None, false));
        // Button held across two records (e.g. a MOUSE_MOVED drag) -> the
        // helper surfaces the currently-held button so the caller can tag
        // the drag with it; the diff itself is empty so `released` is false.
        assert_eq!(
            mouse_button_change(0b0000_0001, 0b0000_0001),
            (Some(0), false)
        );
    }

    fn key_record(
        key_down: bool,
        vk: u16,
        sc: u16,
        ch: u16,
        cks: u32,
        rep: u16,
    ) -> KEY_EVENT_RECORD {
        let mut uchar: KEY_EVENT_RECORD_0 = unsafe { std::mem::zeroed() };
        uchar.UnicodeChar = ch;
        KEY_EVENT_RECORD {
            bKeyDown: if key_down { 1 } else { 0 },
            wRepeatCount: rep,
            wVirtualKeyCode: vk,
            wVirtualScanCode: sc,
            uChar: uchar,
            dwControlKeyState: cks,
        }
    }

    #[test]
    fn key_event_vt_input_writes_utf8() {
        let mut buf = Vec::new();
        let mut pending = [None, None];
        let kev = key_record(true, 0x41, 0x1E, b'A' as u16, 0, 1);
        serialize_key_event(&kev, true, &mut pending, &mut buf);
        assert_eq!(buf, b"A");
    }

    #[test]
    fn key_event_vt_input_pairs_surrogates() {
        let mut buf = Vec::new();
        let mut pending = [None, None];

        // High surrogate alone -> buffered, no output.
        let high = key_record(true, 0, 0, 0xD83D, 0, 1);
        serialize_key_event(&high, true, &mut pending, &mut buf);
        assert!(buf.is_empty());
        assert_eq!(pending[1], Some(0xD83Du16));

        // Low surrogate completes the pair (U+1F600).
        let low = key_record(true, 0, 0, 0xDE00, 0, 1);
        serialize_key_event(&low, true, &mut pending, &mut buf);
        assert_eq!(buf, [0xF0, 0x9F, 0x98, 0x80]);
        assert_eq!(pending[1], None);
    }

    #[test]
    fn key_event_vt_input_drops_lone_low_surrogate() {
        let mut buf = Vec::new();
        let mut pending = [None, None];
        let lone = key_record(true, 0, 0, 0xDC00, 0, 1);
        serialize_key_event(&lone, true, &mut pending, &mut buf);
        assert!(buf.is_empty());
        assert!(pending[1].is_none());
    }

    #[test]
    fn key_event_vt_input_ignores_key_up() {
        let mut buf = Vec::new();
        let mut pending = [None, None];
        let kev = key_record(false, 0x41, 0x1E, b'A' as u16, 0, 1);
        serialize_key_event(&kev, true, &mut pending, &mut buf);
        assert!(buf.is_empty());
    }

    #[test]
    fn key_event_non_vt_emits_win32_input_sequence() {
        let mut buf = Vec::new();
        let mut pending = [None, None];
        // VK 0x25 = VK_LEFT, scan 0x4B, no unicode, key down, no mods,
        // single repeat.
        let kev = key_record(true, 0x25, 0x4B, 0, 0, 1);
        serialize_key_event(&kev, false, &mut pending, &mut buf);
        assert_eq!(buf, b"\x1b[37;75;0;1;0;1_");
    }

    #[test]
    fn key_event_non_vt_zero_vk_with_char_emits_utf8() {
        let mut buf = Vec::new();
        let mut pending = [None, None];
        let kev = key_record(true, 0, 0, b'x' as u16, 0, 1);
        serialize_key_event(&kev, false, &mut pending, &mut buf);
        assert_eq!(buf, b"x");
    }

    fn mouse_record(x: i16, y: i16, buttons: u32, cks: u32, flags: u32) -> MOUSE_EVENT_RECORD {
        MOUSE_EVENT_RECORD {
            dwMousePosition: COORD { X: x, Y: y },
            dwButtonState: buttons,
            dwControlKeyState: cks,
            dwEventFlags: flags,
        }
    }

    #[test]
    fn mouse_event_left_click_press() {
        let mut buf = Vec::new();
        let mut last = 0u32;
        let mev = mouse_record(4, 9, 0b0000_0001, 0, 0);
        serialize_mouse_event(&mev, &mut last, &mut buf);
        // SGR mouse: CSI < cb ; x+1 ; y+1 M
        assert_eq!(buf, b"\x1b[<0;5;10M");
        assert_eq!(last, 0b0000_0001);
    }

    #[test]
    fn mouse_event_left_click_release() {
        let mut buf = Vec::new();
        let mut last = 0b0000_0001u32;
        let mev = mouse_record(4, 9, 0, 0, 0);
        serialize_mouse_event(&mev, &mut last, &mut buf);
        assert_eq!(buf, b"\x1b[<0;5;10m");
    }

    #[test]
    fn mouse_event_move_sets_move_bit() {
        let mut buf = Vec::new();
        let mut last = 0u32;
        let mev = mouse_record(0, 0, 0, 0, MOUSE_MOVED);
        serialize_mouse_event(&mev, &mut last, &mut buf);
        // No button change so nothing emitted (cb is None).
        assert!(buf.is_empty());

        // Move while left button held: cb = 0 | 32 = 32.
        let mev = mouse_record(2, 3, 0b0000_0001, 0, MOUSE_MOVED);
        let mut last = 0b0000_0001u32;
        serialize_mouse_event(&mev, &mut last, &mut buf);
        assert_eq!(buf, b"\x1b[<32;3;4M");
    }

    #[test]
    fn mouse_event_wheel_up_and_down() {
        let mut buf = Vec::new();
        let mut last = 0u32;
        // Wheel up: +delta in the high word.
        let mev = mouse_record(0, 0, 0x0078_0000, 0, MOUSE_WHEELED);
        serialize_mouse_event(&mev, &mut last, &mut buf);
        assert_eq!(buf, b"\x1b[<64;1;1M");

        buf.clear();
        let mut last = 0u32;
        // Wheel down: -delta.
        let mev = mouse_record(0, 0, 0xFF88_0000, 0, MOUSE_WHEELED);
        serialize_mouse_event(&mev, &mut last, &mut buf);
        assert_eq!(buf, b"\x1b[<65;1;1M");
    }

    #[test]
    fn mouse_event_modifiers_set_bits() {
        let mut buf = Vec::new();
        let mut last = 0u32;
        // Left-click + Shift + Ctrl => cb = 0 | 4 | 16 = 20.
        let mev = mouse_record(0, 0, 0b0000_0001, SHIFT_PRESSED | LEFT_CTRL_PRESSED, 0);
        serialize_mouse_event(&mev, &mut last, &mut buf);
        assert_eq!(buf, b"\x1b[<20;1;1M");
    }

    #[test]
    fn resize_event_emits_csi_48_and_dedupes() {
        let mut buf = Vec::new();
        let mut last = None;
        let wev = WINDOW_BUFFER_SIZE_RECORD {
            dwSize: COORD { X: 120, Y: 40 },
        };
        serialize_resize_event(&wev, &mut last, &mut buf);
        assert_eq!(buf, b"\x1b[48;40;120;0;0t");

        // Same size again: no new bytes.
        buf.clear();
        serialize_resize_event(&wev, &mut last, &mut buf);
        assert!(buf.is_empty());
    }

    #[test]
    fn focus_event_in_and_out() {
        let mut buf = Vec::new();
        serialize_focus_event(&FOCUS_EVENT_RECORD { bSetFocus: 1 }, &mut buf);
        assert_eq!(buf, b"\x1b[I");

        buf.clear();
        serialize_focus_event(&FOCUS_EVENT_RECORD { bSetFocus: 0 }, &mut buf);
        assert_eq!(buf, b"\x1b[O");
    }

    /// End-to-end Windows paste pipeline: simulate the KEY_EVENT_RECORD
    /// stream a VT-input-mode console produces when the user pastes
    /// text containing characters from each UTF-8 length class —
    /// ASCII, two-byte (Latin-1 supplement), three-byte (BMP CJK), and
    /// four-byte (supplementary, delivered as a UTF-16 surrogate pair).
    ///
    /// The bytes from `serialize_key_event` are fed to the same
    /// `Decoder` used by the source, and the `PasteChunk` payloads
    /// must reassemble to the exact original Unicode bytes — no
    /// `U+FFFD` substitution, no surrogate leakage, no dropped
    /// codepoints.
    #[test]
    fn windows_paste_pipeline_preserves_unicode_round_trip() {
        use crate::event::{Decoder, Event};

        let body = "héllo 日本 😀 world";
        let mut pending = [None, None];
        let mut bytes = Vec::new();

        // Helper: append every UTF-16 code unit of `s` as one KEY_EVENT
        // record (key-down), matching what ReadConsoleInputW emits.
        let feed = |s: &str, out: &mut Vec<u8>, pending: &mut [Option<u16>; 2]| {
            for unit in s.encode_utf16() {
                let kev = key_record(true, 0, 0, unit, 0, 1);
                serialize_key_event(&kev, true, pending, out);
            }
        };

        feed("\x1b[200~", &mut bytes, &mut pending);
        feed(body, &mut bytes, &mut pending);
        feed("\x1b[201~", &mut bytes, &mut pending);

        // Sanity check: the serializer emitted exactly the UTF-8 of the
        // wire (bracket markers + body) without surrogate fragments.
        let mut expected = Vec::new();
        expected.extend_from_slice(b"\x1b[200~");
        expected.extend_from_slice(body.as_bytes());
        expected.extend_from_slice(b"\x1b[201~");
        assert_eq!(bytes, expected);

        // Drive the decoder and reassemble.
        let mut decoder = Decoder::new(DecoderFlags::empty());
        let events = decoder.parse(&bytes);
        assert!(matches!(events.first(), Some(Event::PasteStart)));
        assert!(matches!(events.last(), Some(Event::PasteEnd)));

        let mut assembled = Vec::new();
        for ev in &events {
            if let Event::PasteChunk(b) = ev {
                assembled.extend_from_slice(b);
            }
        }
        assert_eq!(assembled, body.as_bytes());
        let decoded = String::from_utf8(assembled).expect("valid utf-8");
        assert!(!decoded.contains('\u{FFFD}'));
        assert_eq!(decoded, body);
    }

    /// Same as the round-trip test above but with a UTF-16 surrogate
    /// pair (the 4-byte emoji) split across the decoder's read
    /// boundary: the serializer's two records for the high + low
    /// surrogate are produced first, then the rest of the body. Even
    /// when those bytes are fed to the decoder in two separate calls,
    /// the assembled paste must equal the original.
    #[test]
    fn windows_paste_pipeline_chunked_feed_preserves_emoji() {
        use crate::event::{Decoder, Event};

        let body = "ok 😀 done";
        let mut pending = [None, None];
        let mut prefix = Vec::new();
        let mut suffix = Vec::new();

        // Emit `\x1b[200~ok ` into prefix.
        for unit in "\x1b[200~ok ".encode_utf16() {
            let kev = key_record(true, 0, 0, unit, 0, 1);
            serialize_key_event(&kev, true, &mut pending, &mut prefix);
        }
        // Emit the emoji's surrogate pair into prefix (so the high
        // surrogate is buffered and the low surrogate flushes the full
        // UTF-8 sequence in one shot).
        for unit in "😀".encode_utf16() {
            let kev = key_record(true, 0, 0, unit, 0, 1);
            serialize_key_event(&kev, true, &mut pending, &mut prefix);
        }
        // Emit the rest into suffix.
        for unit in " done\x1b[201~".encode_utf16() {
            let kev = key_record(true, 0, 0, unit, 0, 1);
            serialize_key_event(&kev, true, &mut pending, &mut suffix);
        }

        let mut decoder = Decoder::new(DecoderFlags::empty());
        let mut events = decoder.parse(&prefix);
        events.extend(decoder.parse(&suffix));
        assert!(matches!(events.first(), Some(Event::PasteStart)));
        assert!(matches!(events.last(), Some(Event::PasteEnd)));

        let mut assembled = Vec::new();
        for ev in &events {
            if let Event::PasteChunk(b) = ev {
                assembled.extend_from_slice(b);
            }
        }
        assert_eq!(assembled, body.as_bytes());
        let decoded = String::from_utf8(assembled).expect("valid utf-8");
        assert_eq!(decoded, body);
    }
}
