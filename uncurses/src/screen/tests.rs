//! Integration-style render-pipeline tests for [`Screen`] — mode toggles,
//! string painting, reset/restore, wide glyphs, and `insert_above`.
//!
//! These build a [`Screen`] over an in-memory `Vec` writer (or a borrowed
//! `&mut Vec`) and a null input via [`Screen::for_test`], so the full render
//! path can be exercised and the emitted bytes inspected without a terminal.

use std::sync::{Arc, Mutex};

use super::*;
use crate::renderer::{RenderBuffer, Renderer};
use crate::text::{TextSurface, WidthMode, WrapMode};

#[test]
fn screen_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    // Concrete stdio handles are Send + Sync, so this fails to compile only if
    // an internal field regresses (e.g. the renderer color cache going back to
    // RefCell would drop Sync).
    assert_send_sync::<Screen<crate::terminal::Stdin, crate::terminal::Stdout>>();
}

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

/// Test-only construction and a few shims mirroring the historical renderer
/// API the render tests were written against, so they read the same way while
/// driving the merged [`Screen`].
impl<O: Write> Screen<std::io::PipeReader, O> {
    /// Build a screen over `writer` and a null input, sized to `size`. Output
    /// bytes accumulate in `writer` after [`flush`](Screen::flush); inspect
    /// them via [`writer`](Self::writer) (or the borrowed buffer directly when
    /// `O` is `&mut Vec<u8>`).
    fn for_test(writer: O, size: (u16, u16)) -> Self {
        let env = crate::terminal::Env::from_process();
        let terminal = crate::terminal::Terminal::from_parts(null_input(), writer, env);
        let color_profile = crate::color::Profile::detect_from(terminal.env(), true);
        let optimizations = Optimizations::from_env(terminal.env());
        let mut renderer = Renderer::new();
        renderer.set_color_profile(color_profile);
        renderer.set_optimizations(optimizations);
        renderer.set_fullscreen(false);
        renderer.set_relative_cursor(true);
        let source = Arc::new(Mutex::new(
            crate::event::EventSource::new(null_input()).unwrap(),
        ));
        let mut screen = Self {
            terminal,
            front_buf: RenderBuffer::new(0, 0),
            renderer,
            out_buf: Vec::with_capacity(4096),
            width: 0,
            height: 0,
            eaw_wide: false,
            source,
            state: super::state::State::default(),
            caps: Capabilities::default(),
            options: ScreenOptions::default(),
            defaults_applied: false,
            window_cells: None,
            window_pixels: None,
            terminal_name: None,
            queries_sent_at: None,
            origin: Position::ORIGIN,
            origin_query_pending: false,
            terminal_palette: TerminalPalette::default(),
        };
        let (w, h) = size;
        if w != 0 || h != 0 {
            screen.resize((w, h));
        }
        screen
    }

    /// Like [`for_test`](Self::for_test) but the event source reads from
    /// `input`, so a test can feed terminal replies (e.g. a Primary DA) and
    /// exercise the teardown drain.
    fn for_test_with_input(writer: O, size: (u16, u16), input: std::io::PipeReader) -> Self {
        let mut screen = Self::for_test(writer, size);
        screen.source = Arc::new(Mutex::new(crate::event::EventSource::new(input).unwrap()));
        screen
    }

    fn writer(&self) -> &O {
        self.terminal.output_ref()
    }

    fn width(&self) -> u16 {
        self.width
    }

    fn height(&self) -> u16 {
        self.height
    }

    fn with_eaw_wide(mut self, eaw_wide: bool) -> Self {
        self.eaw_wide = eaw_wide;
        self
    }

    fn with_color_profile(mut self, profile: crate::color::Profile) -> Self {
        self.renderer.set_color_profile(profile);
        self
    }

    fn with_optimizations(mut self, optimizations: Optimizations) -> Self {
        self.renderer.set_optimizations(optimizations);
        self
    }

    fn set_alt_screen(&mut self, alt_screen: bool) {
        self.stage_set_alt_screen(alt_screen);
    }

    fn set_grapheme_clusters(&mut self, enable: bool) {
        self.stage_set_grapheme_clusters(enable);
    }

    fn set_kitty_keyboard_flags(&mut self, flags: crate::ansi::kitty::KittyKeyboardFlags) {
        self.stage_set_kitty_keyboard_flags(flags);
    }

    fn cursor_position(&self) -> Position {
        self.tracked_cursor().unwrap_or_default()
    }
}

#[test]
fn test_new_screen() {
    let screen = Screen::for_test(Vec::new(), (80, 24));
    assert_eq!(screen.width(), 80);
    assert_eq!(screen.height(), 24);
}

#[test]
fn test_write_and_render() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (20, 5));
        {
            screen.set_str((0, 0), "Hello", crate::style::Style::default());
        };
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    assert!(String::from_utf8_lossy(&buf).contains("Hello"));
}

#[test]
fn test_alt_screen() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (80, 24));
        screen.set_alt_screen(true);
        assert!(screen.state.alt_screen);
        screen.set_alt_screen(false);
        assert!(!screen.state.alt_screen);
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    assert!(out.contains("\x1b[?1049h"));
    assert!(out.contains("\x1b[?1049l"));
}

#[test]
fn default_width_mode_is_wc() {
    let screen = Screen::for_test(Vec::new(), (20, 1));
    assert_eq!(screen.width_mode(), WidthMode::Wc);
    assert!(!screen.eaw_wide());
}

#[test]
fn with_eaw_wide_sets_eaw_wide() {
    let screen = Screen::for_test(Vec::new(), (20, 1)).with_eaw_wide(true);
    assert!(screen.eaw_wide());
}

#[test]
fn str_width_follows_mode_and_eaw_and_counts_escapes_literally() {
    use crate::text::Painter;
    let mut screen = Screen::for_test(Vec::new(), (20, 1));
    // Plain CJK: two columns regardless of mode.
    assert_eq!(screen.str_width("中"), 2);
    // The literal default does not interpret SGR: the escape bytes count as
    // their visible width.
    assert_eq!(screen.str_width("\x1b[31mhi\x1b[0m"), 9);
    // A Painter is escape-aware, so the same string measures just "hi".
    assert_eq!(Painter::new(&mut screen).str_width("\x1b[31mhi\x1b[0m"), 2);
    // Ambiguous code point flips with eaw_wide.
    assert_eq!(screen.str_width("…"), 1);
    let wide = Screen::for_test(Vec::new(), (20, 1)).with_eaw_wide(true);
    assert_eq!(wide.str_width("…"), 2);
    // VS16 only matters once grapheme-cluster mode is on.
    assert_eq!(screen.str_width("\u{270b}\u{fe0e}"), 2);
    screen.set_grapheme_clusters(true);
    assert_eq!(screen.str_width("\u{270b}\u{fe0e}"), 1);
}

#[test]
fn grapheme_width_and_cells_use_screen_policy() {
    let mut screen = Screen::for_test(Vec::new(), (20, 1));
    // Wc mode is cluster-blind: the VS15 tail is ignored, base '✋' is 2.
    assert_eq!(screen.grapheme_width("\u{270b}\u{fe0e}"), 2);
    screen.set_grapheme_clusters(true);
    // Grapheme mode honours VS15 → text presentation, one column.
    assert_eq!(screen.grapheme_width("\u{270b}\u{fe0e}"), 1);

    let cells: Vec<_> = screen.grapheme_cells("Ae\u{0301}中").collect();
    assert_eq!(cells, vec![("A", 1), ("e\u{0301}", 1), ("中", 2)]);
}

#[test]
fn set_grapheme_clusters_toggles_width_mode_and_emits_decset() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (20, 1));
        screen.set_grapheme_clusters(true);
        assert!(screen.grapheme_clusters());
        // Internal width-mode tracks DEC-2027 so that our segmentation
        // matches the terminal's.
        assert_eq!(screen.width_mode(), WidthMode::Grapheme);
        screen.set_grapheme_clusters(false);
        assert!(!screen.grapheme_clusters());
        assert_eq!(screen.width_mode(), WidthMode::Wc);
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    assert!(out.contains("\x1b[?2027h"));
    assert!(out.contains("\x1b[?2027l"));
}

#[test]
fn write_string_widths_follow_current_mode() {
    // 'e' + combining acute: Wc treats the mark as a width-0 follow-up
    // that attaches to the base cell; Grapheme collapses both into a
    // single grapheme cluster. In both cases the cell at (0, 0) holds
    // the composed string; only the cursor advance differs.
    let mut screen = Screen::for_test(Vec::new(), (10, 1));
    {
        screen.set_str((0, 0), "e\u{0301}", crate::style::Style::default());
    };
    assert_eq!(
        screen
            .front_buf
            .cell(crate::layout::Position::new(0, 0))
            .unwrap()
            .content(),
        "e\u{0301}"
    );
    assert!(
        screen
            .front_buf
            .cell(crate::layout::Position::new(1, 0))
            .unwrap()
            .is_blank()
    );

    screen.set_grapheme_clusters(true);
    screen.clear();
    {
        screen.set_str((0, 0), "e\u{0301}", crate::style::Style::default());
    };
    // Grapheme mode: combined into one cell.
    assert_eq!(
        screen
            .front_buf
            .cell(crate::layout::Position::new(0, 0))
            .unwrap()
            .content(),
        "e\u{0301}"
    );
}

#[test]
fn reset_and_restore_round_trip_grapheme_clusters() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (20, 1));
        screen.set_grapheme_clusters(true);
        assert!(screen.grapheme_clusters());

        // reset: state preserved, teardown writes RM
        screen.reset().unwrap();
        assert!(screen.grapheme_clusters());

        // restore: re-emits SM
        screen.restore().unwrap();
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    // Enable SM, reset emits RM, restore emits SM again.
    assert!(out.matches("\x1b[?2027h").count() >= 2);
    assert!(out.contains("\x1b[?2027l"));
}

#[test]
fn test_resize() {
    let mut screen = Screen::for_test(Vec::new(), (80, 24));
    screen.resize((100, 30));
    assert_eq!(screen.width(), 100);
    assert_eq!(screen.height(), 30);
}

#[test]
fn test_screen_clears_stale_chars_after_navigating() {
    // Simulate the file_explorer scenario: a row that had a long
    // line in frame N becomes blank in frame N+1. The bytes emitted
    // for frame N+1 must clear every column that frame N had
    // written. This is the "stale glyph" reproduction.
    let mut frame1: Vec<u8> = Vec::new();
    let mut frame2: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut frame1, (40, 5));
        screen.clear();
        {
            screen.set_str(
                (0, 2),
                "icu_segmenter = \"compiled_data\"",
                crate::style::Style::default(),
            );
        };
        screen.render().unwrap();
        screen.flush().unwrap();
        // Swap writers so frame 2 lands in its own buffer while
        // the renderer's diff state carries over.
        let _ = std::mem::replace(screen.terminal.output_mut(), &mut frame2);
        screen.clear();
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let s = String::from_utf8_lossy(&frame2);

    // The rendered frame must include some form of clearing for
    // row 2 — either ECH/EL/spaces. If none of these are present,
    // the row will keep showing the old characters.
    let has_clear = s.contains("\x1b[K")
        || s.contains("\x1b[0K")
        || s.bytes().filter(|b| *b == b'X').count() > 0
        || s.contains("                              ");
    assert!(
        has_clear,
        "frame 2 did not clear stale chars on row 2: {:02x?}",
        frame2
    );
}

// --- end-to-end renderer tests ---
//
// Drive the renderer through the Screen render surface. The writer
// is `&mut Vec<u8>` (or scoped so the test owns the `Vec<u8>` that
// outlives the screen), and per-test configuration is established
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

use crate::color::Color;
use crate::renderer::Optimizations;
use crate::style::{Style, UnderlineStyle};

fn s(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn fill(screen: &mut Screen<std::io::PipeReader, &mut Vec<u8>>, x: u16, y: u16, content: &str) {
    screen.set_cell((x, y), &Cell::narrow(content));
}

fn draw_wrapped(screen: &mut Screen<std::io::PipeReader, &mut Vec<u8>>, src: &str) {
    let bounds = screen.bounds();
    screen.set_str_wrap(
        (bounds.x, bounds.y),
        src,
        WrapMode::Wrap,
        crate::style::Style::default(),
    );
}

fn blank_screen(screen: &mut Screen<std::io::PipeReader, &mut Vec<u8>>) {
    for y in 0..screen.height() {
        for x in 0..screen.width() {
            screen.set_cell((x, y), &Cell::BLANK);
        }
    }
}

// --- byte-exact: alt-screen + diagonal Xs ---

#[test]
fn fullscreen_diagonal_emits_exact_byte_stream() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (5, 3));
        screen.set_alt_screen(true);
        fill(&mut screen, 0, 0, "X");
        fill(&mut screen, 1, 1, "X");
        fill(&mut screen, 2, 2, "X");
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    // DECSET 1049, hide cursor, CUP home, ED2, three Xs separated by
    // LF (relies on the natural cursor wrap at column 5 to advance
    // rows), show cursor.
    assert_eq!(s(&buf), "\x1b[?1049h\x1b[?25l\x1b[H\x1b[2JX\nX\nX\x1b[?25h");
}

// --- byte-exact: inline "Hello, World!" ---

#[test]
fn inline_hello_world_emits_exact_byte_stream() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (80, 24));
        screen.set_str(
            (0u16, 0u16),
            "Hello, World!",
            crate::style::Style::default(),
        );
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    // Inline mode in a 24-row surface: hide cursor, CR + ED-below to
    // wipe any prior content from the cursor downward, print, then
    // pad with trailing blank rows. Total `\n` after the initial CR
    // is 23.
    let mut expected = String::from("\x1b[?25l\r\x1b[JHello, World!\r");
    for _ in 0..23 {
        expected.push('\n');
    }
    expected.push_str("\x1b[?25h");
    assert_eq!(s(&buf), expected);
}

// --- color profile downsampling: each profile must emit its own SGR ---

#[test]
fn truecolor_profile_emits_38_2_rgb() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (1, 1)).with_color_profile(Profile::TrueColor);
        screen.set_cell(
            (0u16, 0u16),
            &Cell::narrow("X").style(Style::default().fg(Color::rgb(255, 0, 0))),
        );
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(
        out.contains("\x1b[38;2;255;0;0m"),
        "missing TC SGR: {out:?}"
    );
    assert!(!out.contains("\x1b[38;5;"), "must not downsample: {out:?}");
}

#[test]
fn ansi256_profile_emits_38_5_index() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (1, 1)).with_color_profile(Profile::Ansi256);
        screen.set_cell(
            (0u16, 0u16),
            &Cell::narrow("X").style(Style::default().fg(Color::rgb(255, 0, 0))),
        );
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(out.contains("\x1b[38;5;"), "missing 256-color SGR: {out:?}");
    assert!(!out.contains("\x1b[38;2;"), "must downsample TC: {out:?}");
}

#[test]
fn ansi_profile_emits_basic_sgr_3x_or_9x() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (1, 1)).with_color_profile(Profile::Ansi);
        screen.set_cell(
            (0u16, 0u16),
            &Cell::narrow("X").style(Style::default().fg(Color::rgb(255, 0, 0))),
        );
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    // BrightRed renders as `\x1b[91m`; plain Red would be `\x1b[31m`.
    let has_basic = (30..=37)
        .chain(90..=97)
        .any(|n| out.contains(&format!("\x1b[{n}m")));
    assert!(has_basic, "missing 16-color SGR: {out:?}");
    assert!(!out.contains("\x1b[38;2;"));
    assert!(!out.contains("\x1b[38;5;"));
}

#[test]
fn ascii_profile_emits_no_color_sgr() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (1, 1)).with_color_profile(Profile::Ascii);
        screen.set_cell(
            (0u16, 0u16),
            &Cell::narrow("X").style(Style::default().fg(Color::rgb(255, 0, 0))),
        );
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(out.contains('X'));
    assert!(!out.contains("\x1b[38;"));
    assert!(!out.contains("\x1b[48;"));
    for n in (30..=37).chain(40..=47).chain(90..=97).chain(100..=107) {
        assert!(!out.contains(&format!("\x1b[{n}m")), "leak {n}: {out:?}");
    }
}

// --- API mechanics ---

#[test]
fn cursor_position_round_trip() {
    let mut screen = Screen::for_test(Vec::new(), (80, 24));
    screen.move_cursor_to((5, 10)).unwrap();
    let p = screen.cursor_position();
    assert_eq!((p.x, p.y), (5, 10));
}

#[test]
fn move_to_emits_relative_cursor_sequence() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (80, 24));
        screen.move_cursor_to((5, 3)).unwrap();
        screen.flush().unwrap();
    }
    // Inline mode at first move: CR, then 3 newlines down, then CUF 5.
    assert_eq!(s(&buf), "\r\n\n\n\x1b[5C");
}

#[test]
fn tracked_cursor_is_none_until_known() {
    let mut screen = Screen::for_test(Vec::new(), (80, 24));
    assert_eq!(screen.tracked_cursor(), None);
    screen.move_cursor_to((3, 2)).unwrap();
    assert_eq!(screen.tracked_cursor(), Some(Position::new(3, 2)));
    screen.invalidate_tracked_cursor();
    assert_eq!(screen.tracked_cursor(), None);
}

#[test]
fn set_tracked_cursor_updates_belief_without_emitting() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (80, 24));
        screen.set_tracked_cursor((4, 1));
        assert_eq!(screen.tracked_cursor(), Some(Position::new(4, 1)));
        screen.flush().unwrap();
    }
    assert!(buf.is_empty(), "set_tracked_cursor must not emit: {buf:?}");
}

#[test]
fn move_cursor_by_offsets_the_tracked_cursor() {
    let mut screen = Screen::for_test(Vec::new(), (80, 24));
    screen.move_cursor_to((10, 5)).unwrap();
    screen.move_cursor_by(-3, 2).unwrap();
    assert_eq!(screen.tracked_cursor(), Some(Position::new(7, 7)));
    // Saturates at the origin rather than wrapping.
    screen.move_cursor_by(-100, -100).unwrap();
    assert_eq!(screen.tracked_cursor(), Some(Position::ORIGIN));
}

#[test]
fn move_cursor_by_treats_unknown_tracked_cursor_as_origin() {
    let mut screen = Screen::for_test(Vec::new(), (80, 24));
    assert_eq!(screen.tracked_cursor(), None);
    screen.move_cursor_by(3, 2).unwrap();
    assert_eq!(screen.tracked_cursor(), Some(Position::new(3, 2)));
}

#[test]
fn set_cursor_position_applied_at_end_of_render() {
    let mut screen = Screen::for_test(Vec::new(), (80, 24));
    screen.set_str((0, 0), "Hi", crate::style::Style::default());
    screen.set_cursor_position(Position::new(5, 3));
    screen.render().unwrap();
    // After the cell diff the renderer parks the cursor at the staged spot.
    assert_eq!(screen.tracked_cursor(), Some(Position::new(5, 3)));
}

#[test]
fn set_cursor_position_is_sticky_across_frames() {
    let mut screen = Screen::for_test(Vec::new(), (80, 24));
    screen.set_cursor_position(Position::new(2, 1));

    screen.set_str((0, 0), "a", crate::style::Style::default());
    screen.render().unwrap();
    assert_eq!(screen.tracked_cursor(), Some(Position::new(2, 1)));

    // A later frame whose diff moves the cursor elsewhere still ends parked
    // at the sticky position without re-staging it.
    screen.set_str((0, 0), "b", crate::style::Style::default());
    screen.render().unwrap();
    assert_eq!(screen.tracked_cursor(), Some(Position::new(2, 1)));
}

#[test]
fn cursor_only_frame_emits_move_without_cell_changes() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (80, 24));
        screen.set_cursor_position(Position::new(4, 2));
        screen.render().unwrap();
        assert_eq!(screen.tracked_cursor(), Some(Position::new(4, 2)));
    }
    assert!(
        !buf.is_empty(),
        "cursor-only frame must emit a move: {buf:?}"
    );
}

#[test]
fn set_cursor_position_clamps_out_of_bounds() {
    let mut screen = Screen::for_test(Vec::new(), (5, 3));
    screen.set_cursor_position(Position::new(99, 99));
    screen.render().unwrap();
    // Clamped to the bottom-right cell of the 5x3 surface.
    assert_eq!(screen.tracked_cursor(), Some(Position::new(4, 2)));
}

#[test]
fn clearing_desired_cursor_emits_nothing_on_cursor_only_frame() {
    let mut screen = Screen::for_test(Vec::new(), (80, 24));
    screen.set_cursor_position(Position::new(4, 2));
    screen.render().unwrap();
    let len_after_move = screen.writer().len();
    // With no staged position and no cell changes, render does nothing.
    screen.clear_cursor_position();
    screen.render().unwrap();
    assert_eq!(
        screen.writer().len(),
        len_after_move,
        "cleared cursor-only frame must be silent"
    );
}

#[test]
fn set_cursor_position_accepts_bare_tuple() {
    let mut screen = Screen::for_test(Vec::new(), (80, 24));
    screen.set_cursor_position((5, 3));
    screen.render().unwrap();
    assert_eq!(screen.tracked_cursor(), Some(Position::new(5, 3)));
}

#[test]
fn set_title_emits_osc_0() {
    let mut screen = Screen::for_test(Vec::new(), (80, 24));
    screen.set_title("hi").unwrap();
    assert_eq!(s(screen.writer()), "\x1b]0;hi\x1b\\");
}

#[test]
fn set_window_title_emits_osc_2() {
    let mut screen = Screen::for_test(Vec::new(), (80, 24));
    screen.set_window_title("hi").unwrap();
    assert_eq!(s(screen.writer()), "\x1b]2;hi\x1b\\");
}

#[test]
fn set_icon_title_emits_osc_1() {
    let mut screen = Screen::for_test(Vec::new(), (80, 24));
    screen.set_icon_title("hi").unwrap();
    assert_eq!(s(screen.writer()), "\x1b]1;hi\x1b\\");
}

#[test]
fn reset_coalesces_equal_titles_into_single_osc_0() {
    let mut setup: Vec<u8> = Vec::new();
    let mut reset_buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut setup, (80, 24));
        screen.set_title("App").unwrap();
        // Swap writers so only reset-side bytes land in reset_buf.
        let _ = std::mem::replace(screen.terminal.output_mut(), &mut reset_buf);
        screen.reset().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&reset_buf);
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
    let mut setup: Vec<u8> = Vec::new();
    let mut reset_buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut setup, (80, 24));
        screen.set_window_title("Win").unwrap();
        screen.set_icon_title("Icon").unwrap();
        let _ = std::mem::replace(screen.terminal.output_mut(), &mut reset_buf);
        screen.reset().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&reset_buf);
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
    let mut setup: Vec<u8> = Vec::new();
    let mut restore_buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut setup, (80, 24));
        screen.set_title("App").unwrap();
        let _ = std::mem::replace(screen.terminal.output_mut(), &mut restore_buf);
        screen.restore().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&restore_buf);
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
    let mut setup: Vec<u8> = Vec::new();
    let mut restore_buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut setup, (80, 24));
        screen.set_title("App").unwrap();
        // An empty string clears the override (state -> None) while still
        // emitting the clearing OSC 0.
        screen.set_title("").unwrap();
        let _ = std::mem::replace(screen.terminal.output_mut(), &mut restore_buf);
        screen.restore().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&restore_buf);
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
fn empty_window_title_clears_only_window_state() {
    let mut setup: Vec<u8> = Vec::new();
    let mut restore_buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut setup, (80, 24));
        screen.set_window_title("Win").unwrap();
        screen.set_icon_title("Icon").unwrap();
        // Clearing only the window title leaves the icon name intact.
        screen.set_window_title("").unwrap();
        let _ = std::mem::replace(screen.terminal.output_mut(), &mut restore_buf);
        screen.restore().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&restore_buf);
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
fn sync_frame_omits_cursor_hide_show() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (80, 24));
        // The caller is in control: enabling sync is trusted, not gated on
        // detected capabilities.
        screen.set_synchronized_output(true);
        screen.set_str((0, 0), "Hi", crate::style::Style::default());
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    // The frame is wrapped in synchronized-output begin/end (DEC 2026) ...
    assert!(out.contains("\x1b[?2026h"), "missing BSU: {out:?}");
    assert!(out.contains("\x1b[?2026l"), "missing ESU: {out:?}");
    // ... and the per-frame DECTCEM toggle is dropped, so the cursor's blink
    // phase is never reset.
    assert!(
        !out.contains("\x1b[?25l"),
        "unexpected cursor hide: {out:?}"
    );
    assert!(
        !out.contains("\x1b[?25h"),
        "unexpected cursor show: {out:?}"
    );
}

#[test]
fn non_sync_frame_brackets_visible_cursor() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (80, 24));
        // Sync off: the visible cursor is bracketed so it doesn't dance.
        screen.set_str((0, 0), "Hi", crate::style::Style::default());
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(!out.contains("\x1b[?2026h"), "unexpected BSU: {out:?}");
    assert!(out.contains("\x1b[?25l"), "missing cursor hide: {out:?}");
    assert!(out.contains("\x1b[?25h"), "missing cursor show: {out:?}");
}

#[test]
fn screen_write_passes_bytes_verbatim() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (10, 1));
        let n = screen.write(b"Hello, World!").unwrap();
        assert_eq!(n, 13);
        screen.flush().unwrap();
    }
    assert_eq!(s(&buf), "Hello, World!");
}

#[test]
fn invalidate_forces_redraw_of_existing_content() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (3, 1));
        fill(&mut screen, 0, 0, "X");
        screen.render().unwrap();
        screen.flush().unwrap();
        screen.invalidate();
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    assert!(s(&buf).matches('X').count() >= 2);
}

#[test]
fn resize_does_not_crash_and_renders_blank() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (40, 10));
        screen.resize((80, 24));
        screen.render().unwrap();
        screen.flush().unwrap();
    }
}

// --- insert_above ---

#[test]
fn insert_above_emits_il_and_content() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (10, 5));
        screen.insert_above("Prepended line").unwrap();
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(
        out.contains("\x1b[L") || out.contains("\x1b[1L") || out.contains("\x1b[2L"),
        "missing IL: {out:?}"
    );
    assert!(out.contains("Prepended line"));
}

#[test]
fn insert_above_renders_styled_line() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (10, 5));
        screen.insert_above("Hello").unwrap();
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(
        out.contains("\x1b[L") || out.contains("\x1b[1L") || out.contains("\x1b[2L"),
        "missing IL: {out:?}"
    );
    assert!(out.contains("Hello"));
}

#[test]
fn multiple_insert_above_emit_one_il_per_call() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (20, 10));
        screen.insert_above("First line").unwrap();
        screen.insert_above("Second line").unwrap();
        screen.insert_above("Third line\nFourth lin").unwrap();
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    let il_total = out.matches("\x1b[L").count()
        + out.matches("\x1b[1L").count()
        + out.matches("\x1b[2L").count();
    assert!(il_total >= 3, "IL count too low ({il_total}) in {out:?}");
    assert!(
        out.contains("\x1b[2L"),
        "expected one multi-line IL in {out:?}"
    );
    for needle in ["First line", "Second line", "Third line", "Fourth lin"] {
        assert!(out.contains(needle), "missing {needle:?}");
    }
}

// --- optimization on/off pairs — each asserts the **discriminating** byte ---

#[test]
fn tab_optimization_on_emits_tab_character() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let opts = Optimizations::default().union(Optimizations::TABS);
        let mut screen = Screen::for_test(&mut buf, (20, 1)).with_optimizations(opts);
        fill(&mut screen, 8, 0, "X");
        fill(&mut screen, 16, 0, "X");
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(
        out.contains('\t'),
        "expected literal tab for advance: {out:?}"
    );
    assert!(!out.contains("\x1b[8C"), "tab should preempt CUF: {out:?}");
}

#[test]
fn tab_optimization_off_emits_cuf_instead_of_tab() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let opts = Optimizations::default().difference(Optimizations::TABS);
        let mut screen = Screen::for_test(&mut buf, (20, 1)).with_optimizations(opts);
        fill(&mut screen, 8, 0, "X");
        fill(&mut screen, 16, 0, "X");
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(
        out.contains("\x1b[8C"),
        "expected CUF 8 instead of tab: {out:?}"
    );
    assert!(!out.contains('\t'), "tab leaked while disabled: {out:?}");
}

#[test]
fn reset_tab_stops_noop_when_tabs_disabled() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let opts = Optimizations::default().difference(Optimizations::TABS);
        let mut screen = Screen::for_test(&mut buf, (20, 1)).with_optimizations(opts);
        screen.reset_tab_stops().unwrap();
    }
    assert!(buf.is_empty(), "tab reset leaked while TABS off: {buf:?}");
}

#[test]
fn reset_tab_stops_emits_reset_when_tabs_enabled() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let opts = Optimizations::default().union(Optimizations::TABS);
        let mut screen = Screen::for_test(&mut buf, (20, 1)).with_optimizations(opts);
        screen.reset_tab_stops().unwrap();
    }
    let out = s(&buf);
    // Either the DECST8C one-shot or the portable TBC clear-all, depending
    // on the terminal detected from the environment.
    assert!(
        out.contains("\x1b[?5W") || out.contains("\x1b[3g"),
        "expected a tab-stop reset: {out:?}"
    );
}

#[test]
fn tab_advance_does_not_overshoot_past_last_stop() {
    // Width 24: real tab stops are at 0, 8, 16. A move to column 21 sits
    // past the last interior stop. The renderer must not emit a tab that
    // relies on landing past that stop (the surface right edge), because on
    // a wider display a tab from column 16 goes to 24, not 21. Replaying
    // the bytes with standard 8-column tab stops must still land "B" at
    // column 21.
    let mut buf: Vec<u8> = Vec::new();
    {
        let opts = Optimizations::default().union(Optimizations::TABS);
        let mut screen = Screen::for_test(&mut buf, (24, 1)).with_optimizations(opts);
        fill(&mut screen, 0, 0, "A");
        fill(&mut screen, 21, 0, "B");
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert_eq!(
        column_of(&out, 'B'),
        Some(21),
        "B should land at column 21 under real 8-col tab stops: {out:?}"
    );
}

/// Replay `out` against a terminal with standard 8-column tab stops and
/// return the column where `target` is printed. Handles CR, tab, and the
/// CUF/CUB/CHA cursor moves the renderer emits.
#[cfg(test)]
fn column_of(out: &str, target: char) -> Option<u16> {
    let bytes = out.as_bytes();
    let mut col: u16 = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            0x1b if i + 1 < bytes.len() && bytes[i + 1] == b'[' => {
                let mut j = i + 2;
                let mut num: u16 = 0;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    num = num * 10 + u16::from(bytes[j] - b'0');
                    j += 1;
                }
                if j < bytes.len() {
                    match bytes[j] {
                        b'C' => col += num.max(1),
                        b'D' => col = col.saturating_sub(num.max(1)),
                        b'G' => col = num.saturating_sub(1),
                        _ => {}
                    }
                }
                i = j + 1;
            }
            b'\r' => {
                col = 0;
                i += 1;
            }
            b'\t' => {
                col = (col / 8 + 1) * 8;
                i += 1;
            }
            b'\n' => i += 1,
            c if c < 0x80 => {
                if c as char == target {
                    return Some(col);
                }
                col += 1;
                i += 1;
            }
            _ => {
                // Skip a UTF-8 continuation run, count one display column.
                col += 1;
                i += 1;
                while i < bytes.len() && bytes[i] & 0xC0 == 0x80 {
                    i += 1;
                }
            }
        }
    }
    None
}

#[test]
fn backspace_optimization_on_emits_bs_for_leftward_move() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let opts = Optimizations::default().union(Optimizations::BS);
        let mut screen = Screen::for_test(&mut buf, (20, 5)).with_optimizations(opts);
        fill(&mut screen, 5, 0, "A");
        fill(&mut screen, 3, 1, "B");
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(out.contains('\x08'), "expected literal BS: {out:?}");
    assert!(
        !out.contains("\x1b[3D"),
        "CUB leaked while BS enabled: {out:?}"
    );
}

#[test]
fn backspace_optimization_off_emits_cub() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let opts = Optimizations::default().difference(Optimizations::BS);
        let mut screen = Screen::for_test(&mut buf, (20, 5)).with_optimizations(opts);
        fill(&mut screen, 5, 0, "A");
        fill(&mut screen, 3, 1, "B");
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(out.contains("\x1b[3D"), "expected CUB 3: {out:?}");
    assert!(!out.contains('\x08'), "BS leaked while disabled: {out:?}");
}

#[test]
fn onlcr_on_uses_bare_lf_between_rows() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let opts = Optimizations::default().union(Optimizations::ONLCR);
        let mut screen = Screen::for_test(&mut buf, (10, 3)).with_optimizations(opts);
        for y in 0..3u16 {
            fill(&mut screen, 0, y, "X");
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(
        out.contains("X\nX\nX"),
        "expected bare-LF row separators: {out:?}"
    );
}

#[test]
fn onlcr_off_emits_crlf_between_rows() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let opts = Optimizations::default().difference(Optimizations::ONLCR);
        let mut screen = Screen::for_test(&mut buf, (10, 3)).with_optimizations(opts);
        for y in 0..3u16 {
            fill(&mut screen, 0, y, "X");
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(
        out.contains("X\r\nX\r\nX"),
        "expected CRLF row separators: {out:?}"
    );
}

#[test]
fn rep_on_collapses_run_to_rep_sequence() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let opts = Optimizations::default().union(Optimizations::REP);
        let mut screen = Screen::for_test(&mut buf, (20, 1)).with_optimizations(opts);
        for x in 0..15u16 {
            fill(&mut screen, x, 0, "A");
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(out.contains("A\x1b[14b"), "expected REP 14: {out:?}");
}

#[test]
fn rep_off_repeats_glyph_literally() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let opts = Optimizations::default().difference(Optimizations::REP);
        let mut screen = Screen::for_test(&mut buf, (20, 1)).with_optimizations(opts);
        for x in 0..15u16 {
            fill(&mut screen, x, 0, "A");
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(out.contains("AAAAAAAAAAAAAAA"));
    assert!(
        !out.contains("\x1b[15b"),
        "REP leaked while disabled: {out:?}"
    );
    assert!(
        !out.contains("\x1b[14b"),
        "REP leaked while disabled: {out:?}"
    );
}

#[test]
fn scroll_optimization_falls_back_to_lf_without_su_sd() {
    // Full-region 1-line scroll up: even with all scroll caps off,
    // branch 1 of scroll_up emits a bare LF (VT100), so the output
    // is dominated by the new last line + a single LF, not a full
    // repaint of every row.
    let mut buf: Vec<u8> = Vec::new();
    {
        let opts = Optimizations::default().difference(
            Optimizations::SU_SD | Optimizations::IL_DL | Optimizations::CSR | Optimizations::REP,
        );
        let mut screen = Screen::for_test(&mut buf, (10, 5)).with_optimizations(opts);
        screen.set_alt_screen(true);
        for y in 0..5u16 {
            for x in 0..10u16 {
                screen.set_cell(
                    (x, y),
                    &Cell::narrow(char::from(b'A' + y as u8).to_string()),
                );
            }
        }
        screen.render().unwrap();
        screen.flush().unwrap();
        for y in 0..4u16 {
            for x in 0..10u16 {
                screen.set_cell(
                    (x, y),
                    &Cell::narrow(char::from(b'A' + 1 + y as u8).to_string()),
                );
            }
        }
        for x in 0..10u16 {
            screen.set_cell((x, 4u16), &Cell::narrow("F"));
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(out.contains("FFFFFFFFF"));
    // Bare LF is the VT100 fallback for full-region 1-line scroll up;
    // exactly one new-line plus the new last row should be emitted
    // for the scroll, not a per-row repaint.
    assert!(out.contains('\n'), "expected bare LF fallback: {out:?}");
    // Total bytes stay small: the scroll did NOT degenerate into a
    // per-row repaint of all 5 rows (~129 bytes is the same shape
    // as the LF-optimized path that runs when caps are on).
    assert!(buf.len() < 180, "too many bytes: {}", buf.len());
}

// --- content correctness ---

#[test]
fn wide_characters_round_trip_to_output() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (10, 1));
        let wide = ["🌟", "中", "文", "字"];
        for (i, ch) in wide.iter().enumerate() {
            screen.set_cell((i as u16 * 2, 0u16), &Cell::wide(*ch));
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    for ch in ["🌟", "中", "文", "字"] {
        assert!(out.contains(ch), "missing {ch}");
    }
}

#[test]
fn zero_width_combining_mark_reaches_output() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (5, 1));
        screen.set_cell((0u16, 0u16), &Cell::narrow("a\u{0301}"));
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    assert!(s(&buf).contains("a\u{0301}"));
}

#[test]
fn styled_text_emits_specific_sgr_payloads() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (4, 1));
        screen.set_cell(
            (0u16, 0u16),
            &Cell::narrow("X").style(Style::default().bold()),
        );
        screen.set_cell(
            (1u16, 0u16),
            &Cell::narrow("X").style(Style::default().fg(Color::rgb(255, 0, 0))),
        );
        screen.set_cell(
            (2u16, 0u16),
            &Cell::narrow("X").style(Style::default().bg(Color::rgb(0, 255, 0))),
        );
        screen.set_cell(
            (3u16, 0u16),
            &Cell::narrow("X").style(Style::default().bold().fg(Color::rgb(0, 0, 255))),
        );
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(out.contains("\x1b[1m"), "missing bold SGR: {out:?}");
    assert!(out.contains("38;2;255;0;0"), "missing TC red fg: {out:?}");
    assert!(out.contains("48;2;0;255;0"), "missing TC green bg: {out:?}");
    assert!(out.contains("38;2;0;0;255"), "missing TC blue fg: {out:?}");
}

#[test]
fn hyperlinks_emit_osc8_with_url() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (10, 1));
        let style = Style::default().link("https://example.com", "");
        for (i, ch) in "link".chars().enumerate() {
            screen.set_cell(
                (i as u16, 0u16),
                &Cell::narrow(ch.to_string()).style(style.clone()),
            );
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(
        out.contains("\x1b]8;;https://example.com"),
        "missing OSC 8 open: {out:?}"
    );
    assert!(out.contains("\x1b]8;;\x1b\\") || out.contains("\x1b]8;;\x07"));
    assert!(out.contains("link"));
}

#[test]
fn hyperlinks_suppressed_under_disabled_profile() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (10, 1)).with_color_profile(Profile::Disabled);
        let style = Style::default().link("https://example.com", "");
        for (i, ch) in "link".chars().enumerate() {
            screen.set_cell(
                (i as u16, 0u16),
                &Cell::narrow(ch.to_string()).style(style.clone()),
            );
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(
        !out.contains("\x1b]8;"),
        "OSC 8 leaked under Disabled profile: {out:?}"
    );
    assert!(out.contains("link"), "glyphs still rendered: {out:?}");
}

#[test]
fn switch_buffer_resizes_and_repaints() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (5, 3));
        fill(&mut screen, 0, 0, "X");
        screen.render().unwrap();
        screen.flush().unwrap();

        screen.resize((10, 6));
        fill(&mut screen, 0, 1, "X");
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    assert!(s(&buf).matches('X').count() >= 2);
    assert!(s(&buf).contains("\x1b[J"));
}

#[test]
fn scroll_optimization_default_keeps_bottom_row_glyph() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (10, 5));
        screen.set_alt_screen(true);
        for y in 0..5u16 {
            for x in 0..10u16 {
                screen.set_cell(
                    (x, y),
                    &Cell::narrow(char::from(b'A' + y as u8).to_string()),
                );
            }
        }
        screen.render().unwrap();
        screen.flush().unwrap();
        for y in 0..4u16 {
            for x in 0..10u16 {
                screen.set_cell(
                    (x, y),
                    &Cell::narrow(char::from(b'A' + 1 + y as u8).to_string()),
                );
            }
        }
        for x in 0..10u16 {
            screen.set_cell((x, 4u16), &Cell::narrow("F"));
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    assert!(s(&buf).contains('F'));
}

#[test]
fn empty_buffer_renders_without_panic() {
    let mut buf: Vec<u8> = Vec::new();
    let mut screen = Screen::for_test(&mut buf, (0, 0));
    screen.render().unwrap();
    screen.flush().unwrap();
}

#[test]
fn large_buffer_renders_bottom_right_glyph() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (1000, 1000));
        screen.set_cell((999u16, 999u16), &Cell::narrow("X"));
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    assert!(s(&buf).contains('X'));
}

// --- style variants — each underline / attribute SGR is emitted ---

#[test]
fn underline_styles_emit_extended_sgr_params() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (10, 1));
        let styles = [
            UnderlineStyle::Single,
            UnderlineStyle::Double,
            UnderlineStyle::Curly,
            UnderlineStyle::Dotted,
            UnderlineStyle::Dashed,
        ];
        for (i, u) in styles.iter().enumerate() {
            let st = Style::default().underline_style(*u);
            screen.set_cell((i as u16, 0u16), &Cell::narrow("U").style(st));
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(out.contains("\x1b[4m"));
    assert!(out.contains("\x1b[4:2m"));
    assert!(out.contains("\x1b[4:3m"));
    assert!(out.contains("\x1b[4:4m"));
    assert!(out.contains("\x1b[4:5m"));
}

#[test]
fn text_attribute_variants_emit_matching_sgr_params() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (5, 1));
        let styles = [
            Style::default().italic(),
            Style::default().faint(),
            Style::default().reverse(),
            Style::default().strikethrough(),
            Style::default().bold(),
        ];
        for (i, st) in styles.iter().enumerate() {
            screen.set_cell((i as u16, 0u16), &Cell::narrow("A").style(st.clone()));
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(
        out.contains("\x1b[3m") || out.contains(";3m"),
        "italic: {out:?}"
    );
    assert!(
        out.contains("\x1b[2m") || out.contains(";2m"),
        "faint: {out:?}"
    );
    assert!(
        out.contains("\x1b[7m") || out.contains(";7m"),
        "reverse: {out:?}"
    );
    assert!(
        out.contains("\x1b[9m") || out.contains(";9m"),
        "strikethrough: {out:?}"
    );
    assert!(
        out.contains("\x1b[1m") || out.contains(";1m"),
        "bold: {out:?}"
    );
}

#[test]
fn color_downsampling_emits_profile_specific_sgr() {
    let cases = [
        (Profile::TrueColor, "\x1b[38;2;"),
        (Profile::Ansi256, "\x1b[38;5;"),
    ];
    for (profile, needle) in cases {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut screen = Screen::for_test(&mut buf, (3, 1)).with_color_profile(profile);
            let cell = Cell::narrow("C").style(Style::default().fg(Color::rgb(123, 234, 45)));
            screen.set_cell((0u16, 0u16), &cell);
            screen.render().unwrap();
            screen.flush().unwrap();
        }
        let out = s(&buf);
        assert!(
            out.contains(needle),
            "profile {profile:?} missing {needle:?}: {out:?}"
        );
    }
}

// --- phantom cursor on the right margin ---
//
// `set_alt_screen(true)` flips relative_cursor=false internally so
// this covers the fullscreen + absolute-cursor configuration
// without runtime mutation.

#[test]
fn phantom_cursor_wraps_glyph_in_autowrap_disable() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (5, 3));
        screen.set_alt_screen(true);
        for y in 0..3u16 {
            screen.set_cell((4u16, y), &Cell::narrow("X"));
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(
        out.contains("\x1b[?7lX\x1b[?7h"),
        "missing autowrap wrap: {out:?}"
    );
    assert_eq!(out.matches('X').count(), 3);
}

// --- line clearing / repeated content ---

#[test]
fn line_clearing_uses_el_when_row_shrinks() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (10, 3));
        for x in 0..10u16 {
            fill(&mut screen, x, 0, "X");
        }
        screen.render().unwrap();
        screen.flush().unwrap();
        for x in 0..10u16 {
            let c = if x == 0 {
                Cell::narrow("X")
            } else {
                Cell::BLANK
            };
            screen.set_cell((x, 0u16), &c);
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(out.contains("\x1b[K"), "missing EL: {out:?}");
}

#[test]
fn repeated_character_run_emits_literals_without_rep() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let opts = Optimizations::default().difference(Optimizations::REP);
        let mut screen = Screen::for_test(&mut buf, (20, 1)).with_optimizations(opts);
        for x in 0..15u16 {
            fill(&mut screen, x, 0, "A");
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(
        out.contains("AAAAAAAAAAAAAAA"),
        "expected literal repeat: {out:?}"
    );
    assert!(!out.contains("\x1b[14b"));
    assert!(!out.contains("\x1b[15b"));
}

#[test]
fn ech_on_mid_row_blanks_emit_erase_character() {
    let mut buf: Vec<u8> = Vec::new();
    let prime_len;
    {
        let opts = Optimizations::default()
            .union(Optimizations::ECH)
            .difference(Optimizations::TABS | Optimizations::CHT);
        let mut screen = Screen::for_test(&mut buf, (20, 1)).with_optimizations(opts);
        for x in 0..20u16 {
            fill(&mut screen, x, 0, "X");
        }
        screen.render().unwrap();
        screen.flush().unwrap();
        prime_len = screen.writer().len();
        fill(&mut screen, 0, 0, "A");
        for x in 1..19u16 {
            fill(&mut screen, x, 0, " ");
        }
        fill(&mut screen, 19, 0, "B");
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf[prime_len..]);
    let has_ech = (1..=20).any(|n| out.contains(&format!("\x1b[{n}X")));
    assert!(has_ech, "missing ECH: {out:?}");
    assert!(out.contains('A') && out.contains('B'));
}

#[test]
fn ech_off_mid_row_blanks_emit_literal_spaces() {
    let mut buf: Vec<u8> = Vec::new();
    let prime_len;
    {
        let opts = Optimizations::default().difference(
            Optimizations::ECH | Optimizations::REP | Optimizations::TABS | Optimizations::CHT,
        );
        let mut screen = Screen::for_test(&mut buf, (20, 1)).with_optimizations(opts);
        for x in 0..20u16 {
            fill(&mut screen, x, 0, "X");
        }
        screen.render().unwrap();
        screen.flush().unwrap();
        prime_len = screen.writer().len();
        fill(&mut screen, 0, 0, "A");
        for x in 1..19u16 {
            fill(&mut screen, x, 0, " ");
        }
        fill(&mut screen, 19, 0, "B");
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf[prime_len..]);
    assert!(
        out.contains("                  "),
        "expected 18-space run between A and B: {out:?}"
    );
    for n in 1..=20 {
        assert!(
            !out.contains(&format!("\x1b[{n}X")),
            "ECH leaked with cap disabled: {out:?}"
        );
    }
}

// --- alt-screen ---

#[test]
fn alt_screen_enter_exit_emits_decset_decrst_and_clear() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (3, 3));
        screen.move_cursor_to((1, 1)).unwrap();
        screen.set_alt_screen(true);
        screen.render().unwrap();
        screen.set_alt_screen(false);
        screen.flush().unwrap();
    }
    let out = s(&buf);
    let h_idx = out.find("\x1b[?1049h").expect("missing DECSET 1049");
    let ed_idx = out.find("\x1b[H\x1b[2J").expect("missing HOME+ED");
    let l_idx = out.find("\x1b[?1049l").expect("missing DECRST 1049");
    assert!(h_idx < ed_idx && ed_idx < l_idx, "out-of-order: {out:?}");
}

#[test]
fn renderer_redraws_when_style_changes() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (5, 1)).with_color_profile(Profile::Ansi);
        fill(&mut screen, 0, 0, "A");
        screen.render().unwrap();
        screen.flush().unwrap();

        screen.set_cell(
            (0u16, 0u16),
            &Cell::narrow("A").style(Style::default().bold()),
        );
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(out.matches('A').count() >= 2);
    assert!(out.contains("\x1b[1m"), "missing bold SGR: {out:?}");
}

#[test]
fn basic_color_fg_emits_sgr_31() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (1, 1));
        let cell = Cell::narrow("X").style(Style::default().fg(Color::Red));
        screen.set_cell((0u16, 0u16), &cell);
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(
        out.contains("\x1b[31m"),
        "missing 16-color red SGR: {out:?}"
    );
}

// --- multi-frame output cases ---

#[test]
fn scroll_to_bottom_in_inline_mode_renders_both_frames() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (10, 5));

        screen.set_str((0u16, 0u16), "ABC", crate::style::Style::default());
        screen.render().unwrap();
        screen.flush().unwrap();

        screen.set_str((0u16, 0u16), "XXX", crate::style::Style::default());
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(out.contains("ABC"));
    assert!(out.contains("XXX"));
}

#[test]
fn alt_screen_scroll_one_line_renders_new_content() {
    const LOREM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Vivamus";
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (10, 5));
        screen.set_alt_screen(true);
        screen.invalidate();

        draw_wrapped(&mut screen, LOREM);
        screen.render().unwrap();
        screen.flush().unwrap();

        blank_screen(&mut screen);
        screen.invalidate();
        draw_wrapped(&mut screen, &LOREM[10..]);
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(out.contains("Lorem"));
    assert!(out.contains("\x1b[H\x1b[2J"));
    assert!(out.contains("dolor"));
}

#[test]
fn alt_screen_scroll_two_lines_renders_tail_content() {
    const LOREM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Vivamus";
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (10, 5));
        screen.set_alt_screen(true);
        screen.invalidate();

        draw_wrapped(&mut screen, LOREM);
        screen.render().unwrap();
        screen.flush().unwrap();

        blank_screen(&mut screen);
        screen.invalidate();
        draw_wrapped(&mut screen, &LOREM[20..]);
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(out.contains("Lorem"));
    assert!(out.contains("amet"));
}

#[test]
fn alt_screen_insert_line_in_middle_renders_both_frames() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (10, 5));
        screen.set_alt_screen(true);
        screen.invalidate();

        draw_wrapped(&mut screen, "ABC\nDEF\nGHI\n");
        screen.render().unwrap();
        screen.flush().unwrap();

        blank_screen(&mut screen);
        draw_wrapped(&mut screen, "ABC\n\nDEF\nGHI");
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    for needle in ["ABC", "DEF", "GHI"] {
        assert!(out.contains(needle), "missing {needle:?}");
    }
}

#[test]
fn inline_erase_until_end_of_line_clears_trailing_cells() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (10, 5));

        screen.set_str((0u16, 1u16), "ABCEFGHIJK", crate::style::Style::default());
        screen.render().unwrap();
        screen.flush().unwrap();

        for x in 0..10u16 {
            let cell = match x {
                0 => Cell::narrow("A"),
                1 => Cell::narrow("B"),
                2 => Cell::narrow("C"),
                3 => Cell::narrow("E"),
                _ => Cell::BLANK,
            };
            screen.set_cell((x, 1u16), &cell);
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(out.contains("ABCEFGHIJK"));
    let cleared = out.contains("\x1b[K")
        || out.contains("\x1b[0K")
        || out.contains("\x1b[6X")
        || out.contains("      ");
    assert!(
        cleared,
        "no clearing sequence in cumulative output: {out:?}"
    );
}

#[test]
fn redraw_identical_frame_after_clear_emits_zero_bytes() {
    let mut buf: Vec<u8> = Vec::new();
    let bytes_after_frame_one;
    {
        let mut screen = Screen::for_test(&mut buf, (20, 4));
        screen.set_str((0, 0), "Hello", crate::style::Style::default());
        screen.set_str((0, 1), "World", crate::style::Style::default());
        screen.set_str((0, 2), "!!!!", crate::style::Style::default());
        screen.render().unwrap();
        screen.flush().unwrap();
        bytes_after_frame_one = screen.writer().len();

        // Frame 2: clear + identical redraw. Must produce no bytes.
        screen.clear();
        screen.set_str((0, 0), "Hello", crate::style::Style::default());
        screen.set_str((0, 1), "World", crate::style::Style::default());
        screen.set_str((0, 2), "!!!!", crate::style::Style::default());
        screen.render().unwrap();
        screen.flush().unwrap();

        let after = screen.writer().len();
        assert_eq!(
            after,
            bytes_after_frame_one,
            "expected zero bytes for identical redraw, got {} new bytes: {:?}",
            after - bytes_after_frame_one,
            String::from_utf8_lossy(&screen.writer()[bytes_after_frame_one..])
        );
    }
}

#[test]
fn reset_moves_cursor_to_last_row_inline() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (20, 5));
        screen.set_str((0, 0), "hi", crate::style::Style::default());
        screen.render().unwrap();
        screen.reset().unwrap();
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
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
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (20, 5));
        screen.set_alt_screen(true);
        screen.set_str((0, 0), "hi", crate::style::Style::default());
        screen.render().unwrap();
        screen.reset().unwrap();
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    let leave = out.find("\x1b[?1049l").expect("alt-screen leave");
    let head = &out[..leave];
    // Alt-screen: emit the move before leaving so terminals that
    // don't honor DECRST 1049's saved-cursor restore still land at
    // a sensible row. Terminals that do honor it will undo our move.
    assert!(
        head.contains("\x1b[5;1H")
            || head.contains("\x1b[5H")
            || head.contains("\x1b[4B")
            || head.matches('\n').count() >= 4,
        "expected cursor move to last row before alt-screen leave, head={head:?}"
    );
}

#[test]
fn reset_uses_front_buf_height_not_live_height_after_resize() {
    // Simulate the user's bug: small render, terminal grows, quit.
    // The cursor must land on the bottom of the *rendered* surface
    // (5 rows), not the new live height (50 rows), so a terminal
    // that loses the alt-screen saved cursor across a resize doesn't
    // pull the post-quit cursor far below where the user started.
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (20, 5));
        screen.set_alt_screen(true);
        screen.set_str((0, 0), "hi", crate::style::Style::default());
        screen.render().unwrap();
        // Grow the screen. front_buf still reflects the 5-row render.
        screen.resize((20, 50));
        screen.reset().unwrap();
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    let leave = out.find("\x1b[?1049l").expect("alt-screen leave");
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
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (20, 1));
        screen.set_kitty_keyboard_flags(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
        // Always emits, even when the tracked flags are unchanged.
        screen.set_kitty_keyboard_flags(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
        screen.set_kitty_keyboard_flags(KittyKeyboardFlags::empty());
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    let bits = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES.bits();
    assert_eq!(out.matches(&format!("\x1b[={bits};1u")).count(), 2);
    assert!(out.contains("\x1b[=0;1u"));
}

#[test]
fn kitty_keyboard_reapplies_on_alt_screen_toggle() {
    use crate::ansi::kitty::KittyKeyboardFlags;
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (20, 1));
        screen.set_kitty_keyboard_flags(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
        screen.set_alt_screen(true);
        screen.set_alt_screen(false);
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    let bits = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES.bits();
    // Initial set + alt-enter re-apply + alt-leave re-apply = 3 emissions.
    assert_eq!(out.matches(&format!("\x1b[={bits};1u")).count(), 3);
}

#[test]
fn reset_clears_kitty_keyboard_on_both_buffers_when_alt_active() {
    use crate::ansi::kitty::KittyKeyboardFlags;
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (20, 1));
        screen.set_kitty_keyboard_flags(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
        screen.set_alt_screen(true);
        screen.reset().unwrap();
        // State preserved across reset for restore to use.
        assert_eq!(
            screen.state.kitty_keyboard,
            KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        );
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    let leave = out.find("\x1b[?1049l").expect("alt-screen leave");
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
    use crate::ansi::kitty::KittyKeyboardFlags;
    let mut setup: Vec<u8> = Vec::new();
    let mut restore_buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut setup, (20, 1));
        screen.set_kitty_keyboard_flags(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
        screen.set_alt_screen(true);
        screen.reset().unwrap();
        // Swap writers so only restore-side bytes land in restore_buf.
        let _ = std::mem::replace(screen.terminal.output_mut(), &mut restore_buf);
        screen.restore().unwrap();
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&restore_buf);
    let bits = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES.bits();
    let enter = out.find("\x1b[?1049h").expect("alt-screen enter");
    let head = &out[..enter];
    let tail = &out[enter..];
    // Set on main first (before the alt-screen enter), then on alt
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
#[cfg(unix)]
#[test]
fn drain_consumes_pending_da_reply_and_marks_done() {
    let (reader, mut writer) = std::io::pipe().unwrap();
    // A Primary DA reply terminates the capability-reply stream.
    writer.write_all(b"\x1b[?65;1c").unwrap();
    drop(writer); // EOF after the reply so poll sees it and does not block

    let mut buf: Vec<u8> = Vec::new();
    let mut screen = Screen::for_test_with_input(&mut buf, (20, 1), reader);
    // Simulate post-init state: queries were just sent, not yet consumed.
    screen.queries_sent_at = Some(std::time::Instant::now());
    screen.defaults_applied = false;

    screen.drain_pending_queries().unwrap();

    assert!(
        screen.defaults_applied,
        "drain should consume the Primary DA reply and mark defaults applied"
    );
}

// Enabling in-band resize (DEC 2048) as a discovery-driven default makes the
// terminal echo one `CSI 48 ; ... t` size report *after* the Primary DA
// terminator. The drain must consume that echo too, or it leaks to the shell
// once cooked mode is restored. Unix-only for the same pipe-polling reason.
#[cfg(unix)]
#[test]
fn drain_consumes_in_band_resize_echo_from_enabling_default() {
    let (reader, mut writer) = std::io::pipe().unwrap();
    // DA1 terminates the query stream; enabling 2048 then echoes a resize.
    writer.write_all(b"\x1b[?65;1c").unwrap();
    writer.write_all(b"\x1b[48;24;80;768;1040t").unwrap();
    drop(writer); // EOF so poll does not block once both replies are read

    let mut buf: Vec<u8> = Vec::new();
    let mut screen = Screen::for_test_with_input(&mut buf, (20, 1), reader);
    // Terminal reported 2048 support, so apply_defaults will enable it.
    screen.caps.in_band_resize = true;
    screen.queries_sent_at = Some(std::time::Instant::now());
    screen.defaults_applied = false;

    screen.drain_pending_queries().unwrap();

    assert!(
        screen.defaults_applied,
        "DA reply should mark defaults applied"
    );
    assert!(
        screen.try_read_event().is_none(),
        "the in-band resize echo must be consumed, not left to leak"
    );
}

#[test]
fn drain_is_noop_when_defaults_already_applied() {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(writer);
    let mut buf: Vec<u8> = Vec::new();
    let mut screen = Screen::for_test_with_input(&mut buf, (20, 1), reader);
    // Already consumed: drain must not block or read.
    screen.defaults_applied = true;
    screen.queries_sent_at = Some(std::time::Instant::now());
    screen.options.query_drain_timeout = std::time::Duration::from_secs(10);

    let start = std::time::Instant::now();
    screen.drain_pending_queries().unwrap();
    assert!(
        start.elapsed() < std::time::Duration::from_millis(500),
        "drain must return immediately when defaults are already applied"
    );
}

#[test]
fn drain_is_noop_when_no_queries_were_sent() {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(writer);
    let mut buf: Vec<u8> = Vec::new();
    let mut screen = Screen::for_test_with_input(&mut buf, (20, 1), reader);
    // No queries sent (e.g. query_capabilities was off): nothing to drain.
    screen.defaults_applied = false;
    screen.queries_sent_at = None;
    screen.options.query_drain_timeout = std::time::Duration::from_secs(10);

    let start = std::time::Instant::now();
    screen.drain_pending_queries().unwrap();
    assert!(
        start.elapsed() < std::time::Duration::from_millis(500),
        "drain must return immediately when no queries were sent"
    );
}

// Uses a pipe the event source must poll; Unix-only for the same reason as
// drain_consumes_pending_da_reply_and_marks_done.
#[cfg(unix)]
#[test]
fn drain_gives_up_after_timeout_when_no_reply_arrives() {
    // Keep the write end open so the input never reaches EOF and no reply
    // ever arrives; the drain must still return once the budget elapses.
    let (reader, _writer) = std::io::pipe().unwrap();
    let mut buf: Vec<u8> = Vec::new();
    let mut screen = Screen::for_test_with_input(&mut buf, (20, 1), reader);
    screen.defaults_applied = false;
    screen.queries_sent_at = Some(std::time::Instant::now());
    screen.options.query_drain_timeout = std::time::Duration::from_millis(80);

    let start = std::time::Instant::now();
    screen.drain_pending_queries().unwrap();
    let elapsed = start.elapsed();
    assert!(
        !screen.defaults_applied,
        "no reply arrived, so defaults must stay unapplied"
    );
    assert!(
        elapsed >= std::time::Duration::from_millis(60),
        "drain should wait roughly the budget: {elapsed:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(800),
        "drain must not wait far beyond the budget: {elapsed:?}"
    );
}

// --- opt-in terminal palette query (OSC 10/11/4) -----------------------------

// With the palette option on, init stages the OSC 10/11/4 requests in the same
// batch as the capability queries, before the terminating Primary DA. This is
// the whole safety argument: the requests carry no terminator of their own, so
// they must ride *inside* the DA-terminated init batch to be covered by it.
#[test]
fn palette_query_rides_the_init_batch_before_the_da() {
    let mut buf: Vec<u8> = Vec::new();
    let mut screen = Screen::for_test(&mut buf, (20, 1));
    screen.options.query_terminal_palette = true;
    screen.send_init_queries().unwrap();

    let out = &buf;
    let da = find_sub(out, b"\x1b[c").or_else(|| find_sub(out, b"\x1b[0c"));
    let da = da.expect("init batch must end with a Primary DA request");
    let fg = find_sub(out, b"\x1b]10;?").expect("foreground query present");
    let bg = find_sub(out, b"\x1b]11;?").expect("background query present");
    let idx = find_sub(out, b"\x1b]4;").expect("indexed palette query present");
    assert!(fg < da && bg < da && idx < da, "palette queries must precede the DA terminator");
}

// The default is off: an application that does not opt in gets no palette
// escapes on the wire, so nothing new can leak.
#[test]
fn palette_query_absent_by_default() {
    let mut buf: Vec<u8> = Vec::new();
    let mut screen = Screen::for_test(&mut buf, (20, 1));
    assert!(!screen.options.query_terminal_palette);
    screen.send_init_queries().unwrap();
    assert!(find_sub(&buf, b"\x1b]10;?").is_none(), "no fg query without opt-in");
    assert!(find_sub(&buf, b"\x1b]11;?").is_none(), "no bg query without opt-in");
    assert!(find_sub(&buf, b"\x1b]4;").is_none(), "no indexed query without opt-in");
}

// The leak test the reviewer asked for, at the layer the terminator lives:
// palette replies still outstanding when teardown runs must be CONSUMED by the
// drain (covered by the shared Primary DA), never left in the input for the
// shell to read once cooked mode is restored. A delayed reply is queued so it
// is pending at drain time, exactly the "delayed OSC reply after handoff"
// case. Unix-only for the same pipe-polling reason as the other drain tests.
#[cfg(unix)]
#[test]
fn drain_consumes_pending_palette_replies_then_da() {
    let (reader, mut writer) = std::io::pipe().unwrap();
    // The terminal answers the palette query and THEN sends the DA terminator,
    // exactly the order init requested them in. All of it is still pending
    // when teardown runs.
    writer.write_all(b"\x1b]10;rgb:1a1a/1b1b/1c1c\x07").unwrap();
    writer.write_all(b"\x1b]11;rgb:2a2a/2b2b/2c2c\x07").unwrap();
    for i in 0..16u8 {
        write!(writer, "\x1b]4;{i};rgb:0f0f/0f0f/0f0f\x07").unwrap();
    }
    writer.write_all(b"\x1b[?65;1c").unwrap();
    drop(writer); // EOF after the replies so poll sees them and does not block

    let mut buf: Vec<u8> = Vec::new();
    let mut screen = Screen::for_test_with_input(&mut buf, (20, 1), reader);
    screen.options.query_terminal_palette = true;
    // Simulate post-init state: queries were sent, nothing consumed yet.
    screen.queries_sent_at = Some(std::time::Instant::now());
    screen.defaults_applied = false;

    screen.drain_pending_queries().unwrap();

    assert!(
        screen.defaults_applied,
        "the DA reply terminating the palette batch must be drained"
    );
    assert!(
        screen.try_read_event().is_none(),
        "every palette reply must be consumed, not left to leak to the shell"
    );
    // And the drain folded them into the accumulator, so the app can read them.
    let pal = screen.terminal_palette();
    assert_eq!(pal.foreground, Some(crate::color::Color::Rgb(0x1a, 0x1b, 0x1c)));
    assert_eq!(pal.background, Some(crate::color::Color::Rgb(0x2a, 0x2b, 0x2c)));
    assert_eq!(pal.indexed[0], Some(crate::color::Color::Rgb(0x0f, 0x0f, 0x0f)));
    assert_eq!(pal.indexed[15], Some(crate::color::Color::Rgb(0x0f, 0x0f, 0x0f)));
}

// A helper local to these tests: first index of `needle` in `hay`.
fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

#[test]
fn origin_defaults_to_zero_and_maps_mouse() {
    use crate::event::{KeyModifiers, Mouse, MouseButton};

    let mut buf = Vec::new();
    let screen = Screen::for_test(&mut buf, (10, 3));
    // No query has run, so the origin is the terminal top-left and mapping is
    // a pass-through.
    assert_eq!(screen.origin(), Position::ORIGIN);
    let m = Mouse::new(5, 7, MouseButton::Left, KeyModifiers::empty());
    let mapped = screen.mouse_to_origin(m);
    assert_eq!((mapped.x, mapped.y), (5, 7));
}

#[test]
fn origin_maps_mouse_relative_to_stored_origin() {
    use crate::event::{KeyModifiers, Mouse, MouseButton};

    let mut buf = Vec::new();
    let mut screen = Screen::for_test(&mut buf, (10, 3));
    // Simulate a tracked inline origin three rows down.
    screen.origin = Position::new(2, 3);
    let m = Mouse::new(6, 5, MouseButton::Left, KeyModifiers::empty());
    let mapped = screen.mouse_to_origin(m);
    assert_eq!((mapped.x, mapped.y), (4, 2));
    // Clicks above/left of the origin saturate at zero.
    let above = Mouse::new(1, 1, MouseButton::Left, KeyModifiers::empty());
    let mapped = screen.mouse_to_origin(above);
    assert_eq!((mapped.x, mapped.y), (0, 0));
}

#[test]
fn alt_screen_origin_is_zero() {
    let mut buf = Vec::new();
    let mut screen = Screen::for_test(&mut buf, (10, 3));
    screen.origin = Position::new(4, 9);
    screen.state.alt_screen = true;
    // In the alternate screen the managed area starts at the top-left,
    // regardless of any stored inline origin.
    assert_eq!(screen.origin(), Position::ORIGIN);
}

#[test]
fn origin_captured_from_cursor_position_reply() {
    let mut buf = Vec::new();
    let mut screen = Screen::for_test(&mut buf, (10, 3));
    screen.window_cells = Some(Size::new(10, 8));
    // Simulate an outstanding `CSI 6n` origin request.
    screen.origin_query_pending = true;
    // The reply is observed (never consumed) and captured as the origin,
    // clipped so the 3-row area stays on screen in an 8-row terminal.
    screen
        .observe_event(&Event::CursorPosition(Position::new(2, 7)))
        .unwrap();
    assert!(!screen.origin_query_pending);
    assert_eq!(screen.origin(), Position::new(2, 5));

    // A later stray CursorPosition (no request pending) is ignored.
    screen
        .observe_event(&Event::CursorPosition(Position::new(0, 0)))
        .unwrap();
    assert_eq!(screen.origin(), Position::new(2, 5));
}

#[test]
fn clip_origin_keeps_area_on_screen() {
    let mut buf = Vec::new();
    let mut screen = Screen::for_test(&mut buf, (10, 3));
    screen.window_cells = Some(Size::new(10, 8));
    // A 3-row area in an 8-row terminal can start no lower than row 5.
    assert_eq!(screen.clip_origin(Position::new(2, 7)), Position::new(2, 5));
    // Within bounds is left untouched.
    assert_eq!(screen.clip_origin(Position::new(2, 4)), Position::new(2, 4));
    // An area at least as tall as the terminal pins to the top.
    screen.window_cells = Some(Size::new(10, 3));
    assert_eq!(screen.clip_origin(Position::new(0, 6)), Position::new(0, 0));
}
