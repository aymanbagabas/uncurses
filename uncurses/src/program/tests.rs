//! Integration-style tests for [`Program`] — terminal/input mode toggles,
//! save/restore round-trips, capability observation, origin requests, and
//! lifecycle over a real pty.
//!
//! Most build a [`Program`] over an in-memory buffer via
//! [`Program::for_test`]. The writer is `&RefCell<Vec<u8>>` rather than
//! `&mut Vec<u8>` because a [`Program`] hands its [`Screen`] a copy of the
//! output handle, so the handle has to be [`Copy`]; read the emitted bytes
//! back with [`out`].

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use super::*;
use crate::text::TextSurface;

#[cfg(unix)]
fn null_input() -> std::io::PipeReader {
    // A pipe read end is epoll/kqueue-registerable (unlike /dev/null), which
    // EventSource::new requires. The write end is dropped; tests never read.
    let (reader, _writer) = std::io::pipe().unwrap();
    reader
}

#[cfg(windows)]
fn null_input() -> std::io::PipeReader {
    let (reader, _writer) = std::io::pipe().unwrap();
    reader
}

/// A [`Copy`] writer that appends into a borrowed buffer, so the same handle
/// can sit in both the [`Terminal`](crate::terminal::Terminal) and the
/// [`Screen`] the way a real `Stdout` does.
#[derive(Clone, Copy)]
struct TestOut<'a>(&'a RefCell<Vec<u8>>);

impl Write for TestOut<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Everything written to `buf` so far, as a string.
fn written(buf: &RefCell<Vec<u8>>) -> String {
    String::from_utf8_lossy(&buf.borrow()).into_owned()
}

impl<'a> Program<std::io::PipeReader, TestOut<'a>> {
    /// Build a program over `buf` and a null input, with a managed area of
    /// `size`. Bypasses [`new`](Program::new) so no fds are needed: nothing
    /// here enters raw mode or measures a window.
    fn for_test(buf: &'a RefCell<Vec<u8>>, size: (u16, u16)) -> Self {
        Self::for_test_with_input(buf, size, null_input())
    }

    /// Like [`for_test`](Self::for_test) but the event source reads from
    /// `input`, so a test can feed terminal replies and watch the program
    /// record them.
    fn for_test_with_input(
        buf: &'a RefCell<Vec<u8>>,
        size: (u16, u16),
        input: std::io::PipeReader,
    ) -> Self {
        let env = crate::terminal::Env::from_process();
        let writer = TestOut(buf);
        let terminal = crate::terminal::Terminal::from_parts(null_input(), writer, env);
        let color_profile = crate::color::Profile::detect_from(terminal.env(), true);
        let optimizations = Optimizations::from_env(terminal.env());
        let mut screen = Screen::new(writer, size);
        screen.set_color_profile(color_profile);
        screen.set_optimizations(optimizations);
        Self {
            terminal,
            screen,
            source: Arc::new(Mutex::new(crate::event::EventSource::new(input).unwrap())),
            unread: VecDeque::new(),
            state: state::State::default(),
            caps: Capabilities::default(),
            options: ProgramOptions::default(),
            window_cells: None,
            window_pixels: None,
            cell_pixels: None,
            origin: Position::ORIGIN,
            origin_queries_pending: 0,
        }
    }
}

#[test]
fn reset_and_restore_round_trip_grapheme_clusters() {
    let buf = RefCell::new(Vec::new());
    {
        let mut program = Program::for_test(&buf, (20, 1));
        program.enable_grapheme_clusters().unwrap();
        assert!(program.screen().grapheme_clusters());

        // reset: state preserved, teardown writes RM
        program.reset().unwrap();
        assert!(program.screen().grapheme_clusters());

        // restore: re-emits SM
        program.restore().unwrap();
        program.screen_mut().flush().unwrap();
    }
    let out = written(&buf);
    // Enable SM, reset emits RM, restore emits SM again.
    assert!(out.matches("\x1b[?2027h").count() >= 2);
    assert!(out.contains("\x1b[?2027l"));
}

/// Resizing drops the tracked terminal contents, so it costs a full repaint.
/// `autoresize` runs on every resize report, including ones that leave the
/// cell grid alone, so the same size must not pay that cost.

// --- end-to-end renderer tests ---
//
// Drive the renderer through the Screen render surface. The writer
// is `&mut Vec<u8>` (or scoped so the test owns the `Vec<u8>` that
// outlives the program), and per-test configuration is established
// up front with the `with_color_profile` / `with_optimizations`
// builders — there is no runtime mutation of these knobs.
//
// Each optimization-toggle pair asserts on the **discriminating
// byte sequence** the optimization controls (ECH, REP, tab,
// backspace, map-newline, scroll), so the ON and OFF variants
// would diverge if the optimization regressed.
//
// Some renderer behaviours are intentionally not covered here:
// * Touched-span tracking: depends on directly poking `RenderBuffer`
//   and reading per-line touched spans, neither of which is part of
//   the public API. Touch tracking is exercised indirectly by every
//   render test that asserts on emitted bytes.
// * Renderer logger hooks: not provided.
// * Style-change runtime mutation: exercised by
//   `renderer_redraws_when_style_changes` below.
// * Runtime mutation of the relative-cursor flag: inline /
//   fullscreen flavors of the relative-cursor model are covered by
//   `inline_hello_world_emits_exact_byte_stream` and
//   `fullscreen_diagonal_emits_exact_byte_stream`.
//
// Output-frame cases drive the same source text through Screen
// (with `set_str_wrap(..., WrapMode::Wrap)`), render both frames in a
// single Screen lifetime, and assert that the relevant content and
// clearing sequences appear in the cumulative byte stream.

#[test]
fn set_title_emits_osc_0() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (80, 24));
    program.set_title("hi").unwrap();
    assert_eq!(written(&buf), "\x1b]0;hi\x1b\\");
}

#[test]
fn set_window_title_emits_osc_2() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (80, 24));
    program.set_window_title("hi").unwrap();
    assert_eq!(written(&buf), "\x1b]2;hi\x1b\\");
}

#[test]
fn set_icon_title_emits_osc_1() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (80, 24));
    program.set_icon_title("hi").unwrap();
    assert_eq!(written(&buf), "\x1b]1;hi\x1b\\");
}

#[test]
fn reset_coalesces_equal_titles_into_single_osc_0() {
    let buf = RefCell::new(Vec::new());
    {
        let mut program = Program::for_test(&buf, (80, 24));
        program.set_title("App").unwrap();
        // Swap writers so only reset-side bytes land in reset_buf.
        buf.borrow_mut().clear();
        program.reset().unwrap();
        program.screen_mut().flush().unwrap();
    }
    let out = written(&buf);
    assert!(
        out.contains("\x1b]0;\x1b\\"),
        "missing OSC 0 reset: {out:?}"
    );
    assert!(
        !out.contains("\x1b]2;\x1b\\"),
        "unexpected OSC 2 reset: {out:?}"
    );
    assert!(
        !out.contains("\x1b]1;\x1b\\"),
        "unexpected OSC 1 reset: {out:?}"
    );
}

#[test]
fn reset_uses_separate_osc_for_differing_titles() {
    let buf = RefCell::new(Vec::new());
    {
        let mut program = Program::for_test(&buf, (80, 24));
        program.set_window_title("Win").unwrap();
        program.set_icon_title("Icon").unwrap();
        buf.borrow_mut().clear();
        program.reset().unwrap();
        program.screen_mut().flush().unwrap();
    }
    let out = written(&buf);
    assert!(
        out.contains("\x1b]2;\x1b\\"),
        "missing OSC 2 reset: {out:?}"
    );
    assert!(
        out.contains("\x1b]1;\x1b\\"),
        "missing OSC 1 reset: {out:?}"
    );
    assert!(
        !out.contains("\x1b]0;\x1b\\"),
        "unexpected OSC 0 reset: {out:?}"
    );
}

#[test]
fn restore_coalesces_equal_titles_into_single_osc_0() {
    let buf = RefCell::new(Vec::new());
    {
        let mut program = Program::for_test(&buf, (80, 24));
        program.set_title("App").unwrap();
        buf.borrow_mut().clear();
        program.restore().unwrap();
        program.screen_mut().flush().unwrap();
    }
    let out = written(&buf);
    assert!(
        out.contains("\x1b]0;App\x1b\\"),
        "missing OSC 0 restore: {out:?}"
    );
    assert!(
        !out.contains("\x1b]2;App"),
        "unexpected OSC 2 restore: {out:?}"
    );
    assert!(
        !out.contains("\x1b]1;App"),
        "unexpected OSC 1 restore: {out:?}"
    );
}

#[test]
fn empty_title_clears_state_and_is_not_restored() {
    let buf = RefCell::new(Vec::new());
    {
        let mut program = Program::for_test(&buf, (80, 24));
        program.set_title("App").unwrap();
        // An empty string clears the override (state -> None) while still
        // emitting the clearing OSC 0.
        program.set_title("").unwrap();
        buf.borrow_mut().clear();
        program.restore().unwrap();
        program.screen_mut().flush().unwrap();
    }
    let out = written(&buf);
    assert!(
        !out.contains("\x1b]0;"),
        "title should not be restored: {out:?}"
    );
    assert!(
        !out.contains("\x1b]1;"),
        "icon should not be restored: {out:?}"
    );
    assert!(
        !out.contains("\x1b]2;"),
        "window should not be restored: {out:?}"
    );
}

#[test]
fn set_progress_state_emits_osc_9_4() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (80, 24));
    program
        .set_progress_state(ProgressState::Normal(42))
        .unwrap();
    assert_eq!(written(&buf), "\x1b]9;4;1;42\x07");
}

#[test]
fn set_progress_state_clamps_and_maps_each_state() {
    for (state, want) in [
        (ProgressState::Normal(0), "\x1b]9;4;1;0\x07"),
        (ProgressState::Normal(200), "\x1b]9;4;1;100\x07"),
        (ProgressState::Error(7), "\x1b]9;4;2;7\x07"),
        (ProgressState::Warning(99), "\x1b]9;4;4;99\x07"),
        (ProgressState::Indeterminate, "\x1b]9;4;3\x07"),
    ] {
        let buf = RefCell::new(Vec::new());
        let mut program = Program::for_test(&buf, (80, 24));
        program.set_progress_state(state).unwrap();
        assert_eq!(written(&buf), want, "wrong bytes for {state:?}");
    }
}

#[test]
fn reset_progress_state_emits_removal() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (80, 24));
    program
        .set_progress_state(ProgressState::Indeterminate)
        .unwrap();
    program.reset_progress_state().unwrap();
    assert!(written(&buf).ends_with("\x1b]9;4;0\x07"));
}

#[test]
fn reset_removes_progress_and_restore_reports_it_again() {
    let buf = RefCell::new(Vec::new());
    {
        let mut program = Program::for_test(&buf, (80, 24));
        program
            .set_progress_state(ProgressState::Error(64))
            .unwrap();
        // Shell handoff: the progress report must come down...
        buf.borrow_mut().clear();
        program.reset().unwrap();
        program.screen_mut().flush().unwrap();
        let reset_out = written(&buf);
        assert!(
            reset_out.contains("\x1b]9;4;0\x07"),
            "progress not removed on reset: {reset_out:?}"
        );

        // ...and go back up on resume.
        buf.borrow_mut().clear();
        program.restore().unwrap();
        program.screen_mut().flush().unwrap();
    }
    let restore_out = written(&buf);
    assert!(
        restore_out.contains("\x1b]9;4;2;64\x07"),
        "progress not restored: {restore_out:?}"
    );
}

#[test]
fn untouched_progress_is_not_reset_or_restored() {
    let buf = RefCell::new(Vec::new());
    {
        let mut program = Program::for_test(&buf, (80, 24));
        program.reset().unwrap();
        program.restore().unwrap();
        program.screen_mut().flush().unwrap();
    }
    assert!(
        !written(&buf).contains("\x1b]9;4"),
        "unexpected progress sequence: {:?}",
        written(&buf)
    );
}

#[test]
fn empty_window_title_clears_only_window_state() {
    let buf = RefCell::new(Vec::new());
    {
        let mut program = Program::for_test(&buf, (80, 24));
        program.set_window_title("Win").unwrap();
        program.set_icon_title("Icon").unwrap();
        // Clearing only the window title leaves the icon name intact.
        program.set_window_title("").unwrap();
        buf.borrow_mut().clear();
        program.restore().unwrap();
        program.screen_mut().flush().unwrap();
    }
    let out = written(&buf);
    assert!(
        out.contains("\x1b]1;Icon\x1b\\"),
        "icon should be restored: {out:?}"
    );
    assert!(
        !out.contains("\x1b]2;"),
        "window should not be restored: {out:?}"
    );
    assert!(!out.contains("\x1b]0;"), "no OSC 0 expected: {out:?}");
}

#[test]
fn reset_lnm_emits_ansi_mode_20_reset() {
    let buf = RefCell::new(Vec::new());
    {
        let mut program = Program::for_test(&buf, (20, 1));
        program.reset_lnm().unwrap();
    }
    // ANSI mode, so no `?` private marker: a DEC-private `\x1b[?20l` is a
    // different mode entirely.
    assert_eq!(written(&buf), "\x1b[20l");
}

#[test]
fn reset_tab_stops_emits_even_when_tabs_disabled() {
    let buf = RefCell::new(Vec::new());
    {
        let opts = Optimizations::default().difference(Optimizations::TABS);
        let mut program = Program::for_test(&buf, (20, 1));
        program.screen_mut().set_optimizations(opts);
        program.reset_tab_stops().unwrap();
    }
    assert!(
        !buf.borrow().is_empty(),
        "tab stops belong to the terminal, not to our willingness to use them: \
         turning TABS on later must not find them unknown"
    );
}

#[test]
fn reset_tab_stops_emits_reset_when_tabs_enabled() {
    let buf = RefCell::new(Vec::new());
    {
        let opts = Optimizations::default().union(Optimizations::TABS);
        let mut program = Program::for_test(&buf, (20, 1));
        program.screen_mut().set_optimizations(opts);
        program.reset_tab_stops().unwrap();
    }
    let out = written(&buf);
    // Either the DECST8C one-shot or the portable TBC clear-all, depending
    // on the terminal detected from the environment.
    assert!(
        out.contains("\x1b[?5W") || out.contains("\x1b[3g"),
        "expected a tab-stop reset: {out:?}"
    );
}

#[test]
fn reset_moves_cursor_to_last_row_inline() {
    let buf = RefCell::new(Vec::new());
    {
        let mut program = Program::for_test(&buf, (20, 5));
        program
            .screen_mut()
            .set_str((0, 0), "hi", crate::style::Style::default());
        program.screen_mut().render().unwrap();
        program.reset().unwrap();
        program.screen_mut().flush().unwrap();
    }
    let out = written(&buf);
    // After "hi" lands at row 0, cur.pos.y == 0. reset must walk down
    // to row 4 (last of 5-row inline surface). In inline / relative
    // mode the planner emits CUD 4 (or 4× LF when cheaper).
    assert!(
        out.contains("\x1b[4B") || out.matches('\n').count() >= 4,
        "expected reset to walk cursor down 4 rows, got {out:?}"
    );
}

#[test]
fn reset_moves_cursor_to_last_row_alt_screen() {
    let buf = RefCell::new(Vec::new());
    {
        let mut program = Program::for_test(&buf, (20, 5));
        program.set_alt_screen(true).unwrap();
        program
            .screen_mut()
            .set_str((0, 0), "hi", crate::style::Style::default());
        program.screen_mut().render().unwrap();
        program.reset().unwrap();
        program.screen_mut().flush().unwrap();
    }
    let out = written(&buf);
    let leave = out.find("\x1b[?1049l").expect("alt-program leave");
    let head = &out[..leave];
    // Alt-program: emit the move before leaving so terminals that
    // don't honor DECRST 1049's saved-cursor restore still land at
    // a sensible row. Terminals that do honor it will undo our move.
    assert!(
        head.contains("\x1b[5;1H")
            || head.contains("\x1b[5H")
            || head.contains("\x1b[4B")
            || head.matches('\n').count() >= 4,
        "expected cursor move to last row before alt-program leave, head={head:?}"
    );
}

#[test]
fn reset_uses_front_buf_height_not_live_height_after_resize() {
    // Simulate the user's bug: small render, terminal grows, quit.
    // The cursor must land on the bottom of the *rendered* surface
    // (5 rows), not the new live height (50 rows), so a terminal
    // that loses the alt-program saved cursor across a resize doesn't
    // pull the post-quit cursor far below where the user started.
    let buf = RefCell::new(Vec::new());
    {
        let mut program = Program::for_test(&buf, (20, 5));
        program.set_alt_screen(true).unwrap();
        program
            .screen_mut()
            .set_str((0, 0), "hi", crate::style::Style::default());
        program.screen_mut().render().unwrap();
        // Grow the program. front_buf still reflects the 5-row render.
        program.screen_mut().resize((20, 50));
        program.reset().unwrap();
        program.screen_mut().flush().unwrap();
    }
    let out = written(&buf);
    let leave = out.find("\x1b[?1049l").expect("alt-program leave");
    let head = &out[..leave];
    // Must not target row 50.
    assert!(
        !head.contains("\x1b[50;1H") && !head.contains("\x1b[50H"),
        "reset targeted live height instead of front-buf height: head={head:?}"
    );
}

// --- kitty keyboard setter ---

#[test]
fn set_kitty_keyboard_flags_always_emits_set() {
    use crate::ansi::kitty::KittyKeyboardFlags;
    let buf = RefCell::new(Vec::new());
    {
        let mut program = Program::for_test(&buf, (20, 1));
        program
            .set_kitty_keyboard(Some(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES))
            .unwrap();
        // Always emits, even when the tracked flags are unchanged.
        program
            .set_kitty_keyboard(Some(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES))
            .unwrap();
        program
            .set_kitty_keyboard(Some(KittyKeyboardFlags::empty()))
            .unwrap();
        program.screen_mut().flush().unwrap();
    }
    let out = written(&buf);
    let bits = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES.bits();
    assert_eq!(out.matches(&format!("\x1b[={bits};1u")).count(), 2);
    assert!(out.contains("\x1b[=0;1u"));
}

#[test]
fn kitty_keyboard_reapplies_on_alt_screen_toggle() {
    use crate::ansi::kitty::KittyKeyboardFlags;
    let buf = RefCell::new(Vec::new());
    {
        let mut program = Program::for_test(&buf, (20, 1));
        program
            .set_kitty_keyboard(Some(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES))
            .unwrap();
        program.set_alt_screen(true).unwrap();
        program.set_alt_screen(false).unwrap();
        program.screen_mut().flush().unwrap();
    }
    let out = written(&buf);
    let bits = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES.bits();
    // Initial set + alt-enter re-apply + alt-leave re-apply = 3 emissions.
    assert_eq!(out.matches(&format!("\x1b[={bits};1u")).count(), 3);
}

#[test]
fn reset_clears_kitty_keyboard_on_both_buffers_when_alt_active() {
    use crate::ansi::kitty::KittyKeyboardFlags;
    let buf = RefCell::new(Vec::new());
    {
        let mut program = Program::for_test(&buf, (20, 1));
        program
            .set_kitty_keyboard(Some(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES))
            .unwrap();
        program.set_alt_screen(true).unwrap();
        program.reset().unwrap();
        // State preserved across reset for restore to use.
        assert_eq!(
            program.state.kitty_keyboard,
            KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        );
        program.screen_mut().flush().unwrap();
    }
    let out = written(&buf);
    let leave = out.find("\x1b[?1049l").expect("alt-program leave");
    let head = &out[..leave];
    let tail = &out[leave..];
    // A clear targets the alt buffer before leaving, and another
    // clear targets the main buffer afterwards.
    assert!(
        head.contains("\x1b[=0;1u"),
        "alt clear missing: head={head:?}"
    );
    assert!(
        tail.contains("\x1b[=0;1u"),
        "main clear missing: tail={tail:?}"
    );
}

#[test]
fn restore_reapplies_kitty_keyboard_on_both_buffers_when_alt_active() {
    let buf = RefCell::new(Vec::new());
    use crate::ansi::kitty::KittyKeyboardFlags;
    {
        let mut program = Program::for_test(&buf, (20, 1));
        program
            .set_kitty_keyboard(Some(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES))
            .unwrap();
        program.set_alt_screen(true).unwrap();
        program.reset().unwrap();
        // Swap writers so only restore-side bytes land in restore_buf.
        buf.borrow_mut().clear();
        program.restore().unwrap();
        program.screen_mut().flush().unwrap();
    }
    let out = written(&buf);
    let bits = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES.bits();
    let enter = out.find("\x1b[?1049h").expect("alt-program enter");
    let head = &out[..enter];
    let tail = &out[enter..];
    // Set on main first (before the alt-program enter), then on alt
    // (after entering).
    assert!(
        head.contains(&format!("\x1b[={bits};1u")),
        "main set missing: head={head:?}"
    );
    assert!(
        tail.contains(&format!("\x1b[={bits};1u")),
        "alt set missing: tail={tail:?}"
    );
}

// --- teardown drain of pending capability-query replies ---

// Uses a pipe the event source must poll; on Windows the source polls a
// console handle, not a pipe, so this drain path is exercised on Unix only.

#[test]
fn origin_defaults_to_zero_and_maps_mouse() {
    let buf = RefCell::new(Vec::new());
    use crate::event::{KeyModifiers, Mouse, MouseButton};

    let program = Program::for_test(&buf, (10, 3));
    // No query has run, so the origin is the terminal top-left and mapping is
    // a pass-through.
    assert_eq!(program.origin(), Position::ORIGIN);
    let m = Mouse::new(5, 7, MouseButton::Left, KeyModifiers::empty());
    let mapped = program.mouse_to_origin(m);
    assert_eq!((mapped.x, mapped.y), (5, 7));
}

#[test]
fn origin_maps_mouse_relative_to_stored_origin() {
    let buf = RefCell::new(Vec::new());
    use crate::event::{KeyModifiers, Mouse, MouseButton};

    let mut program = Program::for_test(&buf, (10, 3));
    // Simulate a tracked inline origin three rows down.
    program.origin = Position::new(2, 3);
    let m = Mouse::new(6, 5, MouseButton::Left, KeyModifiers::empty());
    let mapped = program.mouse_to_origin(m);
    assert_eq!((mapped.x, mapped.y), (4, 2));
    // Clicks above/left of the origin saturate at zero.
    let above = Mouse::new(1, 1, MouseButton::Left, KeyModifiers::empty());
    let mapped = program.mouse_to_origin(above);
    assert_eq!((mapped.x, mapped.y), (0, 0));
}

#[test]
fn alt_screen_origin_is_zero() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));
    program.origin = Position::new(4, 9);
    program.screen_mut().set_fullscreen(true);
    // In the alternate program the managed area starts at the top-left,
    // regardless of any stored inline origin.
    assert_eq!(program.origin(), Position::ORIGIN);
}

#[test]
fn origin_captured_from_cursor_position_reply() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));
    program.window_cells = Some(Size::new(10, 8));
    // Simulate an outstanding `CSI 6n` origin request.
    program.origin_queries_pending = 1;
    // The reply is observed (never consumed) and captured as the origin,
    // clipped so the 3-row area stays on program in an 8-row terminal.
    program
        .observe_event(&Event::CursorPosition(Position::new(2, 7)))
        .unwrap();
    assert_eq!(program.origin_queries_pending, 0);
    assert_eq!(program.origin(), Position::new(2, 5));

    // A later stray CursorPosition (no request pending) is ignored.
    program
        .observe_event(&Event::CursorPosition(Position::new(0, 0)))
        .unwrap();
    assert_eq!(program.origin(), Position::new(2, 5));
}

#[test]
fn a_burst_of_origin_requests_keeps_the_last_reply() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));
    program.window_cells = Some(Size::new(10, 20));
    // Two requests in flight, e.g. two resize reports during a window drag.
    program.request_origin().unwrap();
    program.request_origin().unwrap();
    assert_eq!(program.origin_queries_pending, 2);
    // A single flag would keep the first reply and drop the second, leaving
    // the origin at the position the drag started from.
    program
        .observe_event(&Event::CursorPosition(Position::new(0, 4)))
        .unwrap();
    program
        .observe_event(&Event::CursorPosition(Position::new(0, 9)))
        .unwrap();
    assert_eq!(program.origin_queries_pending, 0);
    assert_eq!(program.origin(), Position::new(0, 9));
}

/// An unread event was already observed on its way out, so the read path must
/// hand it back untouched. Routing it through the shared source instead would
/// spend a second in-flight request on the one reply.
#[test]
fn an_unread_event_is_not_observed_a_second_time() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));
    program.window_cells = Some(Size::new(10, 20));
    program.request_origin().unwrap();
    program
        .observe_event(&Event::CursorPosition(Position::new(0, 4)))
        .unwrap();
    assert_eq!(program.origin_queries_pending, 0);

    // A second request is now in flight, and the app hands the spent reply
    // back. Reading it again must not consume that request.
    program.request_origin().unwrap();
    program.unread_event(Event::CursorPosition(Position::new(0, 4)));
    assert!(program.poll_event(Some(Duration::ZERO)).unwrap());
    assert!(matches!(
        program.read_event().unwrap(),
        Event::CursorPosition(_)
    ));
    assert_eq!(program.origin_queries_pending, 1);
}

/// Pixel-to-cell mapping has to scale across the whole grid. Dividing by a
/// truncated cell width reports columns past the last one whenever the pixel
/// width is not an exact multiple of the column count.
#[test]
fn mouse_pixels_map_to_cells_without_running_past_the_grid() {
    use crate::event::KeyModifiers;
    use crate::event::{Mouse, MouseButton};

    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (80, 25));
    // 1000 / 80 truncates to 12 pixels per cell, so a naive divide puts the
    // last pixel column at 83 in an 80-column grid.
    program.window_pixels = Some(Size::new(1000, 500));
    program.window_cells = Some(Size::new(80, 25));

    let at = |x, y| {
        program
            .mouse_pixels_to_cells(Mouse::new(x, y, MouseButton::Left, KeyModifiers::empty()))
            .unwrap()
    };
    assert_eq!((at(999, 499).x, at(999, 499).y), (79, 24));
    assert_eq!((at(0, 0).x, at(0, 0).y), (0, 0));
    assert_eq!(at(500, 250).x, 40);
}

/// The synchronous reads observe as the event passes through, which is why
/// applications call [`Program::observe_event`] only on the async stream.
/// Observing twice would spend two of the requests in flight on one reply.
///
/// Unix-only because it feeds real bytes through the source. The Windows
/// source reads console records, so a pipe handle is not something it can
/// ever read: `GetNumberOfConsoleInputEvents` rejects it as an invalid
/// handle. The behaviour under test is platform-independent.
#[cfg(unix)]
#[test]
fn a_synchronous_read_observes_without_a_second_call() {
    let (reader, mut writer) = std::io::pipe().unwrap();
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test_with_input(&buf, (10, 3), reader);
    program.window_cells = Some(Size::new(10, 20));
    program.request_origin().unwrap();

    writer.write_all(b"\x1b[5;3R").unwrap();
    drop(writer);

    let ev = program.read_event().unwrap();
    assert!(matches!(ev, Event::CursorPosition(_)), "{ev:?}");
    // No observe_event call here on purpose.
    assert_eq!(
        program.origin_queries_pending, 0,
        "the read did not observe"
    );
    assert_eq!(program.origin(), Position::new(2, 4));
}

#[test]
fn mouse_to_origin_is_an_identity_in_fullscreen() {
    use crate::event::{KeyModifiers, Mouse, MouseButton};

    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));
    program.origin = Position::new(0, 5);
    // Entering the alternate screen re-anchors the area at the top-left, so
    // the inline origin recorded earlier must not be subtracted any more.
    program.screen_mut().set_fullscreen(true);
    let m = Mouse::new(3, 7, MouseButton::Left, KeyModifiers::empty());
    let mapped = program.mouse_to_origin(m);
    assert_eq!((mapped.x, mapped.y), (3, 7));
}

#[test]
fn clip_origin_keeps_area_on_screen() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));
    program.window_cells = Some(Size::new(10, 8));
    // A 3-row area in an 8-row terminal can start no lower than row 5.
    assert_eq!(
        program.clip_origin(Position::new(2, 7)),
        Position::new(2, 5)
    );
    // Within bounds is left untouched.
    assert_eq!(
        program.clip_origin(Position::new(2, 4)),
        Position::new(2, 4)
    );
    // An area at least as tall as the terminal pins to the top.
    program.window_cells = Some(Size::new(10, 3));
    assert_eq!(
        program.clip_origin(Position::new(0, 6)),
        Position::new(0, 0)
    );
}

/// The purity guarantee: observing an event records, it never queries. A
/// resize used to fire `CSI 14 t` and `CSI 6n` from inside the read loop.
#[test]
fn observing_a_resize_records_the_size_without_writing() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));
    program.enable_mouse(MouseTracking::empty()).unwrap();
    buf.borrow_mut().clear();

    let ws = crate::terminal::Winsize {
        col: 40,
        row: 12,
        xpixel: 0,
        ypixel: 0,
    };
    program.observe_event(&Event::Resize(ws)).unwrap();

    assert_eq!(program.window_cells, Some(Size::new(40, 12)));
    assert_eq!(program.origin_queries_pending, 0);
    assert!(written(&buf).is_empty(), "{:?}", written(&buf));
}

/// Enabling the mouse sets modes and nothing else; the origin is the caller's
/// to request.
#[test]
fn enabling_the_mouse_does_not_query_the_origin() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));
    program.enable_mouse(MouseTracking::empty()).unwrap();
    assert_eq!(program.origin_queries_pending, 0);
    assert!(!written(&buf).contains("\x1b[6n"), "{:?}", written(&buf));
}

#[test]
fn request_origin_parks_the_cursor_and_asks_where_it_landed() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));
    program.window_cells = Some(Size::new(10, 20));
    program.request_origin().unwrap();

    let out = written(&buf);
    assert!(out.ends_with("\x1b[6n"), "{out:?}");
    assert_eq!(program.origin_queries_pending, 1);

    // The reply is captured as the origin, and observing stays pure.
    buf.borrow_mut().clear();
    program
        .observe_event(&Event::CursorPosition(Position::new(2, 5)))
        .unwrap();
    assert_eq!(program.origin(), Position::new(2, 5));
    assert_eq!(program.origin_queries_pending, 0);
    assert!(written(&buf).is_empty());
}

#[test]
fn request_origin_is_a_no_op_in_fullscreen() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));
    program.screen_mut().set_fullscreen(true);
    program.request_origin().unwrap();
    assert_eq!(program.origin_queries_pending, 0);
    assert!(written(&buf).is_empty(), "{:?}", written(&buf));
}

/// `init` must grant the line-discipline optimizations from the state the
/// terminal is in *after* `make_raw`, not the one it was in before. Raw mode
/// clears `OPOST`, so tabs and backspace reach the terminal untouched and `\n`
/// no longer carries a carriage return. This drives a real pty to prove raw
/// mode actually delivers that, rather than taking `cfmakeraw` on faith.
#[cfg(all(unix, not(target_os = "l4re")))]
#[test]
fn init_grants_tabs_and_bs_after_entering_raw_mode() {
    use std::os::fd::AsRawFd;

    // The master must outlive the slave, so bind it for the whole test.
    let Some((_master, slave)) = crate::testutil::open_pty_pair() else {
        return;
    };
    // Put the slave in cooked mode with tab expansion and ONLCR: the exact
    // opposite of what raw mode leaves behind. Everything past the pty probe
    // is a hard failure -- a silent return here would let the whole test pass
    // vacuously on a machine where ptys work fine.
    let mut cooked: libc::termios = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut cooked) },
        0,
        "tcgetattr on a fresh pty slave: {}",
        std::io::Error::last_os_error()
    );
    cooked.c_oflag |= libc::OPOST | libc::ONLCR;
    assert_eq!(
        unsafe { libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &cooked) },
        0,
        "tcsetattr on a fresh pty slave: {}",
        std::io::Error::last_os_error()
    );

    let term = crate::terminal::Terminal::new(&slave, &slave, crate::terminal::Env::new());
    let mut program = Program::new(term).expect("Program::new over a pty slave");
    // Start from the opposite of what raw mode implies: TABS and BS off, so
    // only the init-time grant can explain their final values. ONLCR on, so
    // the same assertion proves init leaves an opt-in flag alone.
    let primed = Optimizations::modern()
        .with_tabs(false)
        .with_bs(false)
        .with_onlcr(true);
    program.screen_mut().set_optimizations(primed);
    program
        .init_with(ProgramOptions::default())
        .expect("init over a pty slave");

    assert_tabs_and_bs_granted(program.screen().optimizations(), primed, "init");

    // Resume re-enters raw mode, so it must re-grant too. Undo the flags
    // first -- otherwise the assertion would still hold if `resume` did
    // nothing at all.
    program.pause().expect("pause");
    assert_eq!(
        unsafe { libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &cooked) },
        0,
        "restoring cooked mode while paused: {}",
        std::io::Error::last_os_error()
    );
    program.screen_mut().set_optimizations(primed);
    program.resume().expect("resume");
    assert_tabs_and_bs_granted(program.screen().optimizations(), primed, "resume");
}

/// Exactly what a raw-mode entry must leave behind: `TABS` and `BS` on, and
/// every other capability -- including the opt-in `ONLCR` -- exactly as the
/// caller left it. One equality pins the grant, the opt-in, and the baseline.
#[cfg(all(unix, not(target_os = "l4re")))]
fn assert_tabs_and_bs_granted(got: Optimizations, primed: Optimizations, phase: &str) {
    assert_eq!(
        got,
        primed.with_tabs(true).with_bs(true),
        "{phase} must grant TABS and BS and change nothing else"
    );
}

#[cfg(all(unix, not(target_os = "l4re")))]
mod lnm {
    use super::*;
    use crate::terminal::{Env, Terminal};
    use crate::testutil::{drain, open_pty_pair};

    fn init_bytes_over_pty(f: impl FnOnce(&mut Program<&std::fs::File, &std::fs::File>)) -> String {
        let (Some((_ma, input)), Some((mb, out))) = (open_pty_pair(), open_pty_pair()) else {
            return String::new();
        };
        let terminal = Terminal::new(&input, &out, Env::from_pairs([("TERM", "xterm")]));
        let mut program = Program::new(terminal).expect("program over two ptys");
        f(&mut program);
        String::from_utf8_lossy(&drain(&mb)).into_owned()
    }

    #[test]
    fn init_resets_lnm() {
        let out = init_bytes_over_pty(|program| {
            program.init_with(ProgramOptions::default()).expect("init");
        });
        if out.is_empty() {
            return; // no usable pty here
        }
        assert!(out.contains("\x1b[20l"), "init must reset LNM: {out:?}");
    }

    #[test]
    fn resume_resets_lnm() {
        // A program run while we were paused can set LNM behind our back, so
        // re-entering raw mode has to impose it again rather than assume the
        // reset from init survived.
        let out = init_bytes_over_pty(|program| {
            program.init_with(ProgramOptions::default()).expect("init");
            program.pause().expect("pause");
            program.resume().expect("resume");
        });
        if out.is_empty() {
            return; // no usable pty here
        }
        assert_eq!(
            out.matches("\x1b[20l").count(),
            2,
            "init and resume must each reset LNM: {out:?}"
        );
    }
}

/// `finish` consumes the program and there is no `Drop` impl, so if a teardown
/// write failure skipped the restore the terminal would stay raw for good. A
/// program piped into `head` hits exactly that: the reader goes away, the
/// final flush fails with `EPIPE`, and the user is left with a broken shell.
#[cfg(all(unix, not(target_os = "l4re")))]
mod teardown_failure {
    use super::*;
    use crate::terminal::{Env, Terminal};
    use crate::testutil::{open_pty_pair, opost, prime};
    use std::cell::Cell;
    use std::fs::File;
    use std::os::fd::{AsFd, BorrowedFd};

    /// A pty output half whose writes can be made to fail on demand, which is
    /// what a broken pipe or a hung-up terminal looks like to the renderer.
    /// `Screen` needs its output half to be `Copy`, hence the shared `Cell`
    /// rather than a plain `bool`.
    #[derive(Clone, Copy)]
    struct Breakable<'a> {
        tty: &'a File,
        broken: &'a Cell<bool>,
    }

    impl io::Write for Breakable<'_> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.broken.get() {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            (&*self.tty).write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.broken.get() {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            (&*self.tty).flush()
        }
    }

    impl AsFd for Breakable<'_> {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.tty.as_fd()
        }
    }

    #[test]
    fn finish_restores_the_terminal_even_when_teardown_fails() {
        let (Some((_ma, input)), Some((_mb, out))) = (open_pty_pair(), open_pty_pair()) else {
            return;
        };
        // A fresh pty's attributes are implementation-defined, so put `OPOST`
        // where the assertions below need it rather than assuming it.
        prime(&input, true);
        prime(&out, true);

        let broken = Cell::new(false);
        let output = Breakable {
            tty: &out,
            broken: &broken,
        };
        let terminal = Terminal::new(&input, output, Env::from_pairs([("TERM", "xterm")]));
        let mut program = Program::new(terminal).expect("program over two ptys");
        program.init().expect("init");
        assert!(!opost(&input) && !opost(&out), "both halves must be raw");

        broken.set(true);
        let err = program.finish().expect_err("the teardown flush must fail");
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
        assert!(
            opost(&input) && opost(&out),
            "finish must hand the terminal back even when teardown fails"
        );
    }
}

/// Switching screen buffers is the program's job: it emits 1049 and brings the
/// screen's addressing with it.
#[test]
fn enter_and_exit_alt_screen_emit_1049_and_retarget_the_screen() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (80, 24));

    program.enter_alt_screen().unwrap();
    assert!(program.screen().fullscreen());
    assert!(written(&buf).contains("\x1b[?1049h"));

    buf.borrow_mut().clear();
    program.exit_alt_screen().unwrap();
    assert!(!program.screen().fullscreen());
    assert!(written(&buf).contains("\x1b[?1049l"));
}

/// Some terminals track DECTCEM per screen buffer, so a hidden cursor must be
/// re-hidden on whichever buffer becomes active.
#[test]
fn alt_screen_switch_reasserts_cursor_visibility() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (80, 24));
    program.hide_cursor().unwrap();
    assert!(!program.screen().cursor_visible());

    buf.borrow_mut().clear();
    program.enter_alt_screen().unwrap();
    let out = written(&buf);
    let enter = out.find("\x1b[?1049h").expect("missing DECSET 1049");
    assert!(
        out[enter..].contains("\x1b[?25l"),
        "cursor not re-hidden on the alt buffer: {out:?}"
    );

    buf.borrow_mut().clear();
    program.exit_alt_screen().unwrap();
    let out = written(&buf);
    let exit = out.find("\x1b[?1049l").expect("missing DECRST 1049");
    assert!(
        out[exit..].contains("\x1b[?25l"),
        "cursor not re-hidden on the normal buffer: {out:?}"
    );
}

/// Re-entering the buffer the program is already on emits the mode (harmless,
/// and it re-asserts intent) but does not repeat the per-buffer fixups.
#[test]
fn redundant_alt_screen_switch_skips_the_per_buffer_fixups() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (80, 24));
    program.enter_alt_screen().unwrap();

    buf.borrow_mut().clear();
    program.enter_alt_screen().unwrap();
    assert_eq!(written(&buf), "\x1b[?1049h");
}

/// `screen_mut` hands out the render properties, so an app can retarget the
/// renderer without the program ever emitting the matching mode. Teardown must
/// follow what the program actually sent: emitting DECRST 1049 for a buffer
/// that was never entered would drop the shell onto the wrong screen.
#[test]
fn reset_ignores_render_properties_the_program_never_emitted() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (80, 24));

    // Move every render-coupled property behind the program's back.
    program.screen_mut().set_fullscreen(true);
    program.screen_mut().set_cursor_visible(false);
    program.screen_mut().set_grapheme_clusters(true);

    buf.borrow_mut().clear();
    program.reset().unwrap();
    program.screen_mut().flush().unwrap();

    let out = written(&buf);
    assert!(
        !out.contains("\x1b[?1049"),
        "left a buffer it never entered: {out:?}"
    );
    assert!(
        !out.contains("\x1b[?25h"),
        "showed a cursor it never hid: {out:?}"
    );
    assert!(
        !out.contains("\x1b[?2027"),
        "reset a mode it never set: {out:?}"
    );
}

/// The mirror image: the program emitted the modes, then the app moved the
/// render properties back. Teardown still has to undo what was sent, or the
/// terminal is left on the alt screen with no cursor.
#[test]
fn reset_undoes_emitted_modes_even_when_render_properties_disagree() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (80, 24));
    program.enter_alt_screen().unwrap();
    program.hide_cursor().unwrap();
    program.enable_grapheme_clusters().unwrap();

    program.screen_mut().set_fullscreen(false);
    program.screen_mut().set_cursor_visible(true);
    program.screen_mut().set_grapheme_clusters(false);

    buf.borrow_mut().clear();
    program.reset().unwrap();
    program.screen_mut().flush().unwrap();

    let out = written(&buf);
    assert!(
        out.contains("\x1b[?1049l"),
        "stuck on the alt screen: {out:?}"
    );
    assert!(out.contains("\x1b[?25h"), "left the cursor hidden: {out:?}");
    assert!(
        out.contains("\x1b[?2027l"),
        "left grapheme mode on: {out:?}"
    );
}

/// `restore` re-applies what the program had emitted, so a pause/resume round
/// trip puts the terminal back exactly as it was.
#[test]
fn restore_reapplies_the_emitted_render_coupled_modes() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (80, 24));
    program.enter_alt_screen().unwrap();
    program.hide_cursor().unwrap();
    program.enable_grapheme_clusters().unwrap();

    buf.borrow_mut().clear();
    program.restore().unwrap();
    program.screen_mut().flush().unwrap();

    let out = written(&buf);
    assert!(
        out.contains("\x1b[?1049h"),
        "did not re-enter the alt screen: {out:?}"
    );
    assert!(
        out.contains("\x1b[?25l"),
        "did not re-hide the cursor: {out:?}"
    );
    assert!(
        out.contains("\x1b[?2027h"),
        "did not re-enable grapheme mode: {out:?}"
    );
}

#[test]
fn prefer_grapheme_clusters_enables_the_mode_when_the_terminal_reports_it() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (20, 1));
    assert!(program.options.prefer_grapheme_clusters);
    assert!(!program.screen().grapheme_clusters());

    program
        .observe_event(&Event::ModeReport {
            mode: crate::ansi::mode::Mode::UNICODE_CORE,
            setting: crate::ansi::mode::ModeSetting::Reset,
        })
        .unwrap();
    program.screen_mut().flush().unwrap();

    assert!(
        program
            .capabilities()
            .supports(crate::ansi::mode::Mode::UNICODE_CORE)
    );
    // The render property follows, so the screen measures the way the
    // terminal now does.
    assert!(program.screen().grapheme_clusters());
    assert!(written(&buf).contains("\x1b[?2027h"));
}

#[test]
fn prefer_in_band_resize_enables_the_mode_when_the_terminal_reports_it() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (20, 1));
    assert!(program.options.prefer_in_band_resize);

    program
        .observe_event(&Event::ModeReport {
            mode: crate::ansi::mode::Mode::IN_BAND_RESIZE,
            setting: crate::ansi::mode::ModeSetting::Reset,
        })
        .unwrap();
    program.screen_mut().flush().unwrap();

    assert!(
        program
            .capabilities()
            .supports(crate::ansi::mode::Mode::IN_BAND_RESIZE)
    );
    assert!(program.state.in_band_resize);
    assert!(written(&buf).contains("\x1b[?2048h"));
}

#[test]
fn prefer_flags_off_records_the_capability_without_emitting() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (20, 1));
    program.options.prefer_grapheme_clusters = false;
    program.options.prefer_in_band_resize = false;

    for mode in [
        crate::ansi::mode::Mode::UNICODE_CORE,
        crate::ansi::mode::Mode::IN_BAND_RESIZE,
    ] {
        program
            .observe_event(&Event::ModeReport {
                mode,
                setting: crate::ansi::mode::ModeSetting::Reset,
            })
            .unwrap();
    }
    program.screen_mut().flush().unwrap();

    // Detection still records the capability; only the adoption is opt-out.
    assert!(
        program
            .capabilities()
            .supports(crate::ansi::mode::Mode::UNICODE_CORE)
    );
    assert!(
        program
            .capabilities()
            .supports(crate::ansi::mode::Mode::IN_BAND_RESIZE)
    );
    assert!(!program.screen().grapheme_clusters());
    assert!(!program.state.in_band_resize);
    let out = written(&buf);
    assert!(
        !out.contains("2027h"),
        "emitted 2027 with the flag off: {out:?}"
    );
    assert!(
        !out.contains("2048h"),
        "emitted 2048 with the flag off: {out:?}"
    );
}

#[test]
fn a_repeated_mode_report_does_not_re_emit_the_mode() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (20, 1));
    for _ in 0..3 {
        for mode in [
            crate::ansi::mode::Mode::UNICODE_CORE,
            crate::ansi::mode::Mode::IN_BAND_RESIZE,
        ] {
            program
                .observe_event(&Event::ModeReport {
                    mode,
                    setting: crate::ansi::mode::ModeSetting::Reset,
                })
                .unwrap();
        }
    }
    program.screen_mut().flush().unwrap();

    let out = written(&buf);
    assert_eq!(out.matches("\x1b[?2027h").count(), 1, "{out:?}");
    assert_eq!(out.matches("\x1b[?2048h").count(), 1, "{out:?}");
}

#[test]
fn an_unavailable_mode_report_enables_nothing() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (20, 1));
    for mode in [
        crate::ansi::mode::Mode::UNICODE_CORE,
        crate::ansi::mode::Mode::IN_BAND_RESIZE,
    ] {
        program
            .observe_event(&Event::ModeReport {
                mode,
                setting: crate::ansi::mode::ModeSetting::NotRecognized,
            })
            .unwrap();
    }
    program.screen_mut().flush().unwrap();

    assert!(
        !program
            .capabilities()
            .supports(crate::ansi::mode::Mode::UNICODE_CORE)
    );
    assert!(
        !program
            .capabilities()
            .supports(crate::ansi::mode::Mode::IN_BAND_RESIZE)
    );
    assert!(!program.screen().grapheme_clusters());
    let out = written(&buf);
    assert!(!out.contains("2027h") && !out.contains("2048h"), "{out:?}");
}

#[test]
fn a_preferred_mode_enabled_by_discovery_is_undone_by_reset() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (20, 1));
    for mode in [
        crate::ansi::mode::Mode::UNICODE_CORE,
        crate::ansi::mode::Mode::IN_BAND_RESIZE,
    ] {
        program
            .observe_event(&Event::ModeReport {
                mode,
                setting: crate::ansi::mode::ModeSetting::Reset,
            })
            .unwrap();
    }
    buf.borrow_mut().clear();

    // Discovery goes through the same emit path as an explicit call, so the
    // emitted-mode record covers it and teardown undoes it.
    program.reset().unwrap();
    program.screen_mut().flush().unwrap();
    let out = written(&buf);
    assert!(out.contains("\x1b[?2027l"), "{out:?}");
    assert!(out.contains("\x1b[?2048l"), "{out:?}");
}

#[test]
fn capabilities_keep_the_reported_mode_setting_not_just_availability() {
    use crate::ansi::mode::{Mode, ModeSetting};

    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (20, 1));
    for (mode, setting) in [
        (Mode::SYNCHRONIZED_OUTPUT, ModeSetting::PermanentlySet),
        (Mode::MOUSE_SGR_PIXEL, ModeSetting::Reset),
        (Mode::COLUMN_132, ModeSetting::NotRecognized),
    ] {
        program
            .observe_event(&Event::ModeReport { mode, setting })
            .unwrap();
    }

    let caps = program.capabilities();
    // The exact reply survives, so "permanently set" stays distinguishable
    // from "currently set".
    assert_eq!(
        caps.mode(Mode::SYNCHRONIZED_OUTPUT),
        Some(ModeSetting::PermanentlySet)
    );
    assert_eq!(caps.mode(Mode::MOUSE_SGR_PIXEL), Some(ModeSetting::Reset));
    // A definite "no" is recorded, and is not the same as silence.
    assert_eq!(
        caps.mode(Mode::COLUMN_132),
        Some(ModeSetting::NotRecognized)
    );
    assert_eq!(caps.mode(Mode::AUTO_WRAP), None);

    assert!(caps.supports(Mode::SYNCHRONIZED_OUTPUT));
    assert!(caps.supports(Mode::MOUSE_SGR_PIXEL));
    assert!(!caps.supports(Mode::COLUMN_132));
    assert!(!caps.supports(Mode::AUTO_WRAP));
}

#[test]
fn capabilities_record_a_mode_the_program_does_not_act_on() {
    use crate::ansi::mode::{Mode, ModeSetting};

    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (20, 1));
    // Nothing in the program special-cases DECCKM; it is still recorded, so
    // an app can query any mode it asked about.
    program
        .observe_event(&Event::ModeReport {
            mode: Mode::CURSOR_KEYS,
            setting: ModeSetting::Set,
        })
        .unwrap();

    assert_eq!(
        program.capabilities().mode(Mode::CURSOR_KEYS),
        Some(ModeSetting::Set)
    );
}

#[test]
fn capabilities_keep_the_raw_primary_da_attributes() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (20, 1));
    program
        .observe_event(&Event::PrimaryDeviceAttributes(vec![
            Some(62),
            Some(4),
            None,
            Some(52),
        ]))
        .unwrap();

    let caps = program.capabilities();
    assert_eq!(
        caps.primary_device_attributes(),
        Some([Some(62), Some(4), None, Some(52)].as_slice())
    );
    assert!(caps.sixel());
    assert!(caps.clipboard());
    assert!(caps.da_attribute(62));
    assert!(!caps.da_attribute(21));
}

#[test]
fn primary_device_attributes_are_none_until_the_terminal_answers() {
    let buf = RefCell::new(Vec::new());
    let program = Program::for_test(&buf, (20, 1));
    let caps = program.capabilities();
    assert_eq!(caps.primary_device_attributes(), None);
    assert!(!caps.sixel());
    assert!(!caps.clipboard());
    assert_eq!(caps.kitty_keyboard(), None);
    assert_eq!(caps.modify_other_keys(), None);
    assert_eq!(caps.terminal_name(), None);
    assert!(!caps.true_color());
}

#[test]
fn capabilities_keep_the_reported_kitty_flags_and_modify_other_keys() {
    use crate::ansi::kitty::KittyKeyboardFlags;

    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (20, 1));
    let flags =
        KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES | KittyKeyboardFlags::REPORT_EVENT_TYPES;
    program
        .observe_event(&Event::KittyKeyboardEnhancements(flags))
        .unwrap();
    program
        .observe_event(&Event::ModifyOtherKeys(
            crate::event::ModifyOtherKeysMode::Mode2,
        ))
        .unwrap();

    let caps = program.capabilities();
    // The reported value, not merely "it answered".
    assert_eq!(caps.kitty_keyboard(), Some(flags));
    assert_eq!(
        caps.modify_other_keys(),
        Some(crate::event::ModifyOtherKeysMode::Mode2)
    );
}

#[test]
fn an_empty_kitty_reply_still_proves_support() {
    use crate::ansi::kitty::KittyKeyboardFlags;

    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (20, 1));
    program
        .observe_event(&Event::KittyKeyboardEnhancements(
            KittyKeyboardFlags::empty(),
        ))
        .unwrap();

    // Some(empty) means "supported, nothing enabled"; None would mean silence.
    assert_eq!(
        program.capabilities().kitty_keyboard(),
        Some(KittyKeyboardFlags::empty())
    );
}

#[test]
fn a_disabled_modify_other_keys_reply_still_proves_support() {
    use crate::event::ModifyOtherKeysMode;

    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (20, 1));
    program
        .observe_event(&Event::ModifyOtherKeys(ModifyOtherKeysMode::Disabled))
        .unwrap();

    // The old bool could not say this: answering "disabled" proves the
    // terminal knows the feature, which is not the same as never answering.
    assert_eq!(
        program.capabilities().modify_other_keys(),
        Some(ModifyOtherKeysMode::Disabled)
    );
}

#[test]
fn the_terminal_name_lands_in_capabilities() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (20, 1));
    program
        .observe_event(&Event::TerminalName("XTerm(380)".to_string()))
        .unwrap();

    assert_eq!(program.capabilities().terminal_name(), Some("XTerm(380)"));
    // The Program shorthand reads the same storage.
    assert_eq!(program.terminal_name(), Some("XTerm(380)"));
}

#[test]
fn a_later_mode_report_replaces_an_earlier_one() {
    use crate::ansi::mode::{Mode, ModeSetting};

    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (20, 1));
    for setting in [ModeSetting::Reset, ModeSetting::Set] {
        program
            .observe_event(&Event::ModeReport {
                mode: Mode::MOUSE_ANY,
                setting,
            })
            .unwrap();
    }

    assert_eq!(
        program.capabilities().mode(Mode::MOUSE_ANY),
        Some(ModeSetting::Set)
    );
    assert_eq!(program.capabilities().modes().len(), 1);
}

#[test]
fn capabilities_keep_the_raw_secondary_and_tertiary_device_attributes() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));

    program
        .observe_event(&Event::SecondaryDeviceAttributes(vec![
            Some(0),
            Some(95),
            None,
        ]))
        .unwrap();
    program
        .observe_event(&Event::TertiaryDeviceAttributes("00000000".to_string()))
        .unwrap();

    let caps = program.capabilities();
    assert_eq!(
        caps.secondary_device_attributes(),
        Some(&[Some(0), Some(95), None][..]),
        "DA2 is reported unparsed, empty parameters included"
    );
    assert_eq!(caps.tertiary_device_attributes(), Some("00000000"));
}

#[test]
fn capabilities_record_the_terminals_own_colors_not_the_overrides() {
    use crate::color::Color;

    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));

    let reported = Color::Rgb(0x1e, 0x1e, 0x2e);
    program
        .observe_event(&Event::BackgroundColor(reported))
        .unwrap();
    program
        .observe_event(&Event::ForegroundColor(Color::Rgb(0xcd, 0xd6, 0xf4)))
        .unwrap();
    program
        .observe_event(&Event::CursorColor(Color::Rgb(0xf5, 0xe0, 0xdc)))
        .unwrap();
    program
        .observe_event(&Event::PaletteColor {
            index: 4,
            color: Color::Rgb(0, 0, 0xff),
        })
        .unwrap();

    // Overriding the background must not rewrite what the terminal reported:
    // one is what we told it, the other is what it told us.
    program
        .set_background_color(Color::Rgb(0xff, 0, 0))
        .unwrap();

    let caps = program.capabilities();
    assert_eq!(caps.background_color(), Some(reported));
    assert_eq!(caps.foreground_color(), Some(Color::Rgb(0xcd, 0xd6, 0xf4)));
    assert_eq!(caps.cursor_color(), Some(Color::Rgb(0xf5, 0xe0, 0xdc)));
    assert_eq!(caps.palette_color(4), Some(Color::Rgb(0, 0, 0xff)));
    assert_eq!(caps.palette_color(5), None);
}

#[test]
fn capabilities_track_the_latest_color_scheme() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));

    assert_eq!(program.capabilities().color_scheme(), None);
    program
        .observe_event(&Event::ColorScheme(crate::event::ColorScheme::Dark))
        .unwrap();
    assert_eq!(
        program.capabilities().color_scheme(),
        Some(crate::event::ColorScheme::Dark)
    );
    // Mode 2031 keeps reporting as the user toggles the scheme.
    program
        .observe_event(&Event::ColorScheme(crate::event::ColorScheme::Light))
        .unwrap();
    assert_eq!(
        program.capabilities().color_scheme(),
        Some(crate::event::ColorScheme::Light)
    );
}

#[test]
fn capabilities_keep_termcap_values_and_distinguish_a_no_from_silence() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));

    program
        .observe_event(&Event::Termcap {
            recognized: true,
            entries: vec![
                ("TN".to_string(), Some("xterm".to_string())),
                ("Co".to_string(), Some("256".to_string())),
                ("kb".to_string(), None),
                // A value carrying the wire delimiters, which is ordinary:
                // xterm-256color's kf13 is "\E[1;2P".
                ("kf13".to_string(), Some("\x1b[1;2P".to_string())),
            ],
        })
        .unwrap();
    program
        .observe_event(&Event::Termcap {
            recognized: false,
            entries: vec![("RGB".to_string(), None)],
        })
        .unwrap();

    let caps = program.capabilities();
    assert_eq!(caps.termcap("TN"), Some("xterm"));
    assert_eq!(caps.termcap("Co"), Some("256"));
    // A boolean capability arrives with no value but is still supported.
    assert_eq!(caps.termcap("kb"), Some(""));
    assert!(caps.supports_termcap("kb"));
    // Reported unsupported: recorded as a definite no, not as silence.
    assert!(!caps.supports_termcap("RGB"));
    assert_eq!(caps.termcap_reports().get("RGB"), Some(&None));
    assert_eq!(caps.termcap_reports().get("Tc"), None);
    assert!(!caps.true_color());
    // A value containing `;` and `=` stays one capability.
    assert_eq!(caps.termcap("kf13"), Some("\x1b[1;2P"));
    assert_eq!(caps.termcap_reports().len(), 5);
}

#[test]
fn a_setting_report_is_not_recorded_as_a_termcap_capability() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));

    // DECRPSS reports a setting, not a capability: the reply names nothing
    // that could key a record. It shared `Event::Termcap` once, which split
    // `0;1;4m` into capabilities the terminal never mentioned.
    program
        .observe_event(&Event::SettingReport {
            recognized: true,
            payload: "1$r0;1;4m".to_string(),
        })
        .unwrap();

    assert!(program.capabilities().termcap_reports().is_empty());
    assert!(!program.capabilities().supports_termcap("1"));
}

#[test]
fn a_truecolor_termcap_reply_upgrades_the_color_profile() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));
    program.screen_mut().set_color_profile(Profile::Ansi256);

    program
        .observe_event(&Event::Termcap {
            recognized: true,
            entries: vec![("Tc".to_string(), None)],
        })
        .unwrap();

    assert!(program.capabilities().true_color());
    assert_eq!(program.screen().color_profile(), Profile::TrueColor);
}

#[test]
fn a_kitty_graphics_response_records_graphics_support() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));

    assert!(!program.capabilities().kitty_graphics());
    program
        .observe_event(&Event::KittyGraphics {
            options: vec![("i".to_string(), "1".to_string())],
            payload: b"OK".to_vec(),
        })
        .unwrap();
    assert!(program.capabilities().kitty_graphics());
}

#[test]
fn a_reported_cell_size_beats_dividing_the_window_sizes() {
    let buf = RefCell::new(Vec::new());
    let mut program = Program::for_test(&buf, (10, 3));

    assert_eq!(program.cell_pixels(), None);

    // Padding around the grid makes the quotient (8x16) understate the cell.
    program
        .observe_event(&Event::WindowCellSize {
            width: 10,
            height: 4,
        })
        .unwrap();
    program
        .observe_event(&Event::WindowPixelSize {
            width: 84,
            height: 68,
        })
        .unwrap();
    assert_eq!(program.cell_pixels(), Some(Size::new(8, 17)));

    program
        .observe_event(&Event::CellPixelSize {
            width: 8,
            height: 16,
        })
        .unwrap();
    assert_eq!(
        program.cell_pixels(),
        Some(Size::new(8, 16)),
        "the terminal's own reply wins over the derived approximation"
    );

    let mouse = crate::event::Mouse::new(
        24,
        32,
        crate::event::MouseButton::Left,
        crate::event::KeyModifiers::empty(),
    );
    let cells = program.mouse_pixels_to_cells(mouse).unwrap();
    assert_eq!((cells.x, cells.y), (3, 2));
}
