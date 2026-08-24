//! Integration-style render-pipeline tests for [`Screen`] — mode toggles,
//! string painting, reset/restore, wide glyphs, and `insert_above`.
//!
//! These build a [`Screen`] over an in-memory `Vec` writer (or a borrowed
//! `&mut Vec`) via [`Screen::for_test`], so the full render path can be
//! exercised and the emitted bytes inspected without a terminal.
use super::*;

use crate::color::{Color, Profile};
use crate::text::{TextSurface, WidthMode, WrapMode};

#[test]
fn screen_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    // Concrete stdio handles are Send + Sync, so this fails to compile only if
    // an internal field regresses (e.g. the renderer color cache going back to
    // RefCell would drop Sync).
    assert_send_sync::<Screen<crate::terminal::Stdout>>();
}

/// Test-only construction and a few shims mirroring the historical renderer
/// API the render tests were written against, so they read the same way while
/// driving the split [`Screen`].
impl<O: Write> Screen<O> {
    /// Build a screen over `writer`, sized to `size`, with the color profile
    /// and optimizations the environment implies — matching what
    /// [`Program`](crate::program::Program) would set up, so the emitted bytes
    /// are what a real session would see. Output accumulates in `writer` after
    /// [`flush`](Screen::flush); inspect it via [`writer`](Screen::writer) (or
    /// the borrowed buffer directly when `O` is `&mut Vec<u8>`).
    fn for_test(writer: O, size: (u16, u16)) -> Self {
        let env = crate::terminal::EnvList::from_process();
        let mut screen = Screen::new(writer, size);
        screen.set_color_profile(crate::color::Profile::detect_from(&env, true));
        screen.set_optimizations(Optimizations::from_env(&env));
        screen
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
        self.set_color_profile(profile);
        self
    }

    fn with_optimizations(mut self, optimizations: Optimizations) -> Self {
        self.set_optimizations(optimizations);
        self
    }

    fn set_alt_screen(&mut self, alt_screen: bool) {
        self.set_fullscreen(alt_screen);
    }

    fn diverge(&self) -> Option<Position> {
        self.renderer.first_divergence(&self.front_buf)
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
fn set_grapheme_clusters_toggles_width_mode() {
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
    // DEC 2027 is a terminal mode; the screen only records how to measure.
    assert!(buf.is_empty(), "screen must not emit modes: {buf:02x?}");
}

#[test]
fn changing_the_width_mode_repaints_but_setting_it_again_does_not() {
    // Whatever is on screen was measured the other way, so the tracked
    // contents are no longer a valid diff base.
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (3, 1));
        fill(&mut screen, 0, 0, "X");
        screen.render().unwrap();
        screen.flush().unwrap();
        screen.set_grapheme_clusters(true);
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    assert!(s(&buf).matches('X').count() >= 2, "{:?}", s(&buf));

    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (3, 1));
        fill(&mut screen, 0, 0, "X");
        screen.render().unwrap();
        screen.flush().unwrap();
        screen.set_grapheme_clusters(false);
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    assert_eq!(s(&buf).matches('X').count(), 1, "{:?}", s(&buf));
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
fn resize_to_the_same_size_does_not_repaint() {
    let mut screen = Screen::for_test(Vec::new(), (80, 24));
    screen.set_str((0, 0), "hello", crate::style::Style::default());
    screen.render().unwrap();

    let after_first = screen.writer().len();
    screen.resize((80, 24));
    screen.render().unwrap();
    assert_eq!(
        screen.writer().len(),
        after_first,
        "same size repainted: {:?}",
        String::from_utf8_lossy(&screen.writer()[after_first..])
    );

    // A real change still repaints.
    screen.resize((80, 25));
    screen.render().unwrap();
    assert!(
        screen.writer().len() > after_first,
        "a changed size must repaint"
    );
}

use crate::renderer::Optimizations;

use crate::style::{Style, UnderlineStyle};

fn s(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn fill(screen: &mut Screen<&mut Vec<u8>>, x: u16, y: u16, content: &str) {
    screen.set_cell((x, y), &Cell::narrow(content));
}

fn draw_wrapped(screen: &mut Screen<&mut Vec<u8>>, src: &str) {
    let bounds = screen.bounds();
    screen.set_str_wrap(
        (bounds.x, bounds.y),
        src,
        WrapMode::Wrap,
        crate::style::Style::default(),
    );
}

fn blank_screen(screen: &mut Screen<&mut Vec<u8>>) {
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
    // Hide cursor, CUP home, ED2, three Xs separated by LF (relies on the
    // natural cursor wrap at column 5 to advance rows), show cursor. No 1049:
    // switching buffers is the terminal's business, not the renderer's.
    assert_eq!(s(&buf), "\x1b[?25l\x1b[H\x1b[2JX\nX\nX\x1b[?25h");
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
fn fullscreen_toggle_repaints_and_leaves_modes_to_the_caller() {
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
    // Going fullscreen discards the tracked contents, so the frame is a full
    // repaint from the home position...
    assert!(out.contains("\x1b[H\x1b[2J"), "missing HOME+ED: {out:?}");
    // ...but the buffer switch itself is a terminal mode the caller owns.
    assert!(
        !out.contains("\x1b[?1049"),
        "screen must not emit 1049: {out:?}"
    );
}

#[test]
fn leaving_fullscreen_repaints_too() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (3, 1));
        screen.set_alt_screen(true);
        fill(&mut screen, 0, 0, "A");
        screen.render().unwrap();
        // Back to the normal buffer, drawing the same content. The tracked
        // contents describe the buffer just left, not the one now showing, so
        // a diff against them would emit nothing and leave the band blank.
        screen.set_alt_screen(false);
        fill(&mut screen, 0, 0, "A");
        screen.render().unwrap();
    }
    let out = s(&buf);
    assert_eq!(out.matches('A').count(), 2, "second frame skipped: {out:?}");
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

/// The fullscreen flag is a render property, not a mode: it retargets the
/// renderer without touching the terminal. Emitting DECSET 1049 belongs to
/// `Program`.
#[test]
fn test_alt_screen() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (80, 24));
        screen.set_alt_screen(true);
        assert!(screen.fullscreen());
        screen.set_alt_screen(false);
        assert!(!screen.fullscreen());
        screen.flush().unwrap();
    }
    // Switching buffers is the terminal's business: the screen only flips how
    // it addresses cells, so no 1049 comes out of it.
    assert!(buf.is_empty(), "screen must not emit modes: {buf:02x?}");
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
    let mut frames: Vec<u8> = Vec::new();
    let frame2_start;
    {
        let mut screen = Screen::for_test(&mut frames, (40, 5));
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
        // Mark where frame 2 starts so it can be inspected alone while the
        // renderer's diff state carries over.
        frame2_start = screen.writer().len();
        screen.clear();
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let frame2 = &frames[frame2_start..];
    let s = String::from_utf8_lossy(frame2);

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

#[test]
fn truecolor_termcap_upgrade_repaints_unchanged_cells() {
    let mut screen = Screen::for_test(Vec::new(), (1, 1)).with_color_profile(Profile::Ansi256);
    screen.set_cell(
        (0u16, 0u16),
        &Cell::narrow("X").style(Style::default().fg(Color::rgb(255, 0, 0))),
    );
    screen.render().unwrap();

    let first_len = screen.writer().len();
    let first = s(screen.writer());
    assert!(
        first.contains("\x1b[38;5;"),
        "first frame should be downsampled: {first:?}"
    );

    // A truecolor discovery reaches the renderer as a profile change; the
    // point of the test is that raising it repaints cells that did not change.
    screen.set_color_profile(Profile::TrueColor);
    screen.render().unwrap();

    let second = s(&screen.writer()[first_len..]);
    assert!(
        second.contains("\x1b[38;2;255;0;0m"),
        "unchanged cell must be repainted after truecolor upgrade: {second:?}"
    );
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

// --- BCE: pen reset before a scrolling newline ---

/// Inline downward moves are emitted as literal `\n`, which scrolls the
/// host when the destination row does not exist yet. Under back-color
/// erase that scroll paints the exposed row with the active pen's
/// background, and no later diff repairs it — so every newline the
/// renderer emits has to leave the pen at the default first.
#[test]
fn inline_render_resets_pen_before_every_newline() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (10, 3));
        // `for_test` derives capabilities and colour from `$TERM`, and BCE
        // is exactly what decides whether the reset is needed. Pin both so
        // the assertion means the same thing on every platform.
        screen.set_optimizations(Optimizations::modern().with_bce(true));
        screen.set_color_profile(crate::color::Profile::Ansi);
        let bg = Style::default().bg(Color::Blue);
        for y in 0..3u16 {
            screen.set_str((0, y), "abcdefghij", bg.clone());
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(
        out.contains("\u{1b}[44m"),
        "expected a painted background: {out:?}"
    );
    for (i, _) in out.match_indices('\n') {
        assert!(
            out[..i].ends_with("\u{1b}[m"),
            "newline at byte {i} is not preceded by a pen reset: {out:?}"
        );
    }
}

/// The reset is only worth its bytes when the pen carries a background:
/// back-color erase paints nothing else, so an unstyled frame must not
/// pay for it. Pinned to the same capabilities as the test above, so
/// this proves the background is what gates the reset — not a missing
/// BCE flag.
#[test]
fn inline_render_without_background_emits_no_pen_resets() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::for_test(&mut buf, (10, 3));
        screen.set_optimizations(Optimizations::modern().with_bce(true));
        screen.set_color_profile(crate::color::Profile::Ansi);
        for y in 0..3u16 {
            screen.set_str((0, y), "abcdefghij", Style::default());
        }
        screen.render().unwrap();
        screen.flush().unwrap();
    }
    let out = s(&buf);
    assert!(
        !out.contains("\u{1b}[m"),
        "an unstyled frame should need no pen reset: {out:?}"
    );
}

/// Render two frames of a two-pane layout and return the bytes the second
/// one emits.
///
/// The layout is a static "file tree" in the left `SIDEBAR` columns of the
/// top `TREE_ROWS` rows, with a scrolling content pane beside it. Frame two
/// scrolls the pane by `SHIFT`; the tree is painted identically both times.
///
/// Two details make this reproduce the case `set_scroll_optimize` exists
/// for, and both are load-bearing:
///
/// * the tree is shorter than the screen, so the rows below it match
///   exactly after the shift and seed a hunk — `grow_hunks` cannot start
///   from nothing, and a full-height tree detects no scroll at all;
/// * the content differs a lot from row to row, the way real source or log
///   lines do, so repainting a row in place costs far more cells than
///   repainting the narrow tree after a scroll. With near-identical rows
///   the cost analysis correctly declines to grow.
fn two_pane_second_frame(scroll_optimize: bool) -> String {
    const W: u16 = 40;
    const H: u16 = 12;
    const SIDEBAR: u16 = 10;
    const TREE_ROWS: u16 = 4;
    const SHIFT: usize = 3;

    fn paint(screen: &mut Screen<Vec<u8>>, offset: usize) {
        for y in 0..H {
            if y < TREE_ROWS {
                for (i, ch) in format!("tree-{y:02}").chars().enumerate() {
                    screen.set_cell((i as u16, y), &Cell::narrow(ch.to_string()));
                }
            }
            let n = y as usize + offset;
            let body: String = "abcdefghijklmnopqrstuvwxyz0123456789"
                .chars()
                .cycle()
                .skip(n * 7 % 36)
                .take((W - SIDEBAR) as usize)
                .collect();
            for (i, ch) in body.chars().enumerate() {
                screen.set_cell((SIDEBAR + i as u16, y), &Cell::narrow(ch.to_string()));
            }
        }
    }

    // Pin the scroll capabilities rather than inheriting them from the
    // environment: which sequence implements the scroll is irrelevant here,
    // but whether one is emitted at all is the whole assertion.
    let opts = Optimizations::default()
        .union(Optimizations::SU_SD | Optimizations::CSR | Optimizations::IL_DL);
    let mut screen = Screen::for_test(Vec::new(), (W, H)).with_optimizations(opts);
    screen.set_alt_screen(true);
    screen.set_scroll_optimize(scroll_optimize);

    paint(&mut screen, 0);
    screen.render().unwrap();
    screen.flush().unwrap();
    screen.writer_mut().clear();

    paint(&mut screen, SHIFT);
    screen.render().unwrap();
    screen.flush().unwrap();
    s(screen.writer())
}

#[test]
fn scroll_detection_moves_a_fixed_column_and_paints_it_back() {
    // Pins the behavior `set_scroll_optimize` exists to switch off, so that
    // turning the knob off is visibly a change rather than a no-op.
    //
    // The scrolls the renderer emits are always full width, so the detected
    // scroll moves the tree too and the renderer repaints it inside the same
    // frame. The settled screen is correct either way, which is exactly why
    // this needs a byte-level assertion: comparing the finished screen sees
    // nothing.
    let out = two_pane_second_frame(true);

    assert!(
        out.contains("\x1b[3S") || out.contains("\x1b[3M") || out.contains("\x1b[3L"),
        "expected a detected 3-row scroll: {out:?}"
    );
    assert!(
        out.contains("tree-"),
        "the scroll moved the tree, so the frame must paint it back: {out:?}"
    );
}

#[test]
fn scroll_optimize_off_leaves_a_fixed_column_untouched() {
    // The same frame with detection off: no scroll goes out, so the tree
    // never moves and never needs repainting. It is unchanged between the
    // two frames, so its cells must not appear in the output at all.
    let out = two_pane_second_frame(false);

    assert!(
        !out.contains("\x1b[3S") && !out.contains("\x1b[3M") && !out.contains("\x1b[3L"),
        "scroll detection is off, so no scroll should be emitted: {out:?}"
    );
    assert!(
        !out.contains("tree-"),
        "an untouched fixed column must not be repainted: {out:?}"
    );
}

/// An imperative cursor move happens between frames, where the desired grid
/// is not what the terminal shows. The move planner may pay for a short
/// forward hop by re-emitting the cells it passes over, so planning it over
/// that grid paints cells the terminal does not have — and it never records
/// them in the tracked buffer, so the next diff sees no work and the
/// divergence is permanent.
///
/// Three ways the desired grid diverges, each reached by a forward hop short
/// enough for the overwrite candidate to beat CUF.
#[test]
fn move_cursor_to_never_emits_cell_content() {
    // (name, how the grid is made to diverge, where to move)
    #[allow(clippy::type_complexity)]
    let cases: [(&str, fn(&mut Screen<Vec<u8>>), (u16, u16)); 3] = [
        // An edit staged since the last render is not on the terminal yet.
        (
            "edit staged but not rendered",
            |s| {
                s.set_str((0, 0), "XY", crate::style::Style::default());
            },
            (3, 0),
        ),
        // A column at or past the width wraps into a later row, so the row
        // the planner lands on is not the one the caller named. A per-row
        // guard in front of the planner would not see this one.
        (
            "column past the width wraps into a dirty row",
            |s| {
                s.set_str((0, 1), "XY", crate::style::Style::default());
            },
            (22, 0),
        ),
        // Entering the alternate screen blanks the terminal while the desired
        // grid still holds the inline frame. The mode is emitted by Program,
        // so the renderer never sees the terminal change.
        (
            "fullscreen switch pending",
            |s| s.set_fullscreen(true),
            (3, 0),
        ),
    ];

    for (name, diverge, to) in cases {
        let mut screen = Screen::for_test(Vec::new(), (20, 5));
        screen.set_str((0, 0), "ab", crate::style::Style::default());
        screen.set_str((0, 1), "cd", crate::style::Style::default());
        screen.render().unwrap();
        screen.move_cursor_to((1, 0)).unwrap();

        diverge(&mut screen);
        let n = screen.writer().len();
        screen.move_cursor_to(to).unwrap();
        let payload = printable_payload(&screen.writer()[n..]);
        assert!(
            payload.is_empty(),
            "{name}: move emitted cell content: {payload:?} (raw {:?})",
            String::from_utf8_lossy(&screen.writer()[n..])
        );
    }
}

/// Everything a byte run would put on screen, with escape sequences and the
/// cursor-moving control characters removed. A cursor move is allowed to emit
/// those; anything left over is cell content it had no business writing.
///
/// Checking for the content bytes directly does not work: `A`, `B`, `C` and
/// `D` are CUU/CUD/CUF/CUB final bytes, and `b` and `d` are REP and VPA.
fn printable_payload(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            0x1b => {
                i += 1;
                if i < bytes.len() && bytes[i] == b'[' {
                    i += 1;
                    // Parameter and intermediate bytes, then one final byte.
                    while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                        i += 1;
                    }
                }
                i += 1;
            }
            b'\r' | b'\n' | 0x08 | b'\t' => i += 1,
            b => {
                out.push(b as char);
                i += 1;
            }
        }
    }
    out
}

/// The end-of-frame cursor move is the one move that may still plan over the
/// front buffer, because it runs after the cell diff has reconciled the
/// terminal to it. That is a precondition, not a guarantee: if a frame ever
/// stopped leaving the tracked contents equal to the front buffer, the move
/// would start re-emitting cells the terminal does not show, exactly as an
/// imperative move between frames would.
#[test]
fn a_rendered_frame_leaves_the_front_buffer_matching_the_terminal() {
    use crate::style::Style;
    let st = Style::default();

    // Fullscreen with every row full to the right margin, which reaches the
    // bottom-right cell the renderer protects from scrolling.
    let mut screen = Screen::for_test(Vec::new(), (10, 3));
    screen.set_fullscreen(true);
    for y in 0..3 {
        screen.set_str((0, y), "0123456789", st.clone());
    }
    screen.set_cursor_position((0, 0));
    screen.render().unwrap();
    assert_eq!(
        screen.diverge(),
        None,
        "fullscreen, rows full to the margin"
    );

    // Wide cells, whose continuation columns the diff has to keep in step.
    let mut screen = Screen::for_test(Vec::new(), (12, 2));
    screen.set_str((0, 0), "漢字テスト", st.clone());
    screen.set_cursor_position((2, 0));
    screen.render().unwrap();
    assert_eq!(screen.diverge(), None, "wide cells");

    // The scroll-optimize path, which rewrites tracked rows in place rather
    // than emitting them cell by cell.
    let mut screen = Screen::for_test(Vec::new(), (12, 6));
    screen.set_fullscreen(true);
    for y in 0..6 {
        screen.set_str((0, y), &format!("line{y}"), st.clone());
    }
    screen.render().unwrap();
    for y in 0..6 {
        screen.set_str((0, y), &format!("line{}", y + 1), st.clone());
    }
    screen.set_cursor_position((3, 3));
    screen.render().unwrap();
    assert_eq!(screen.diverge(), None, "after a scroll");

    // A resize, whose repaint rebuilds both buffers.
    screen.resize((15, 4));
    screen.set_str((0, 3), "grown", st.clone());
    screen.render().unwrap();
    assert_eq!(screen.diverge(), None, "after a resize");
}
