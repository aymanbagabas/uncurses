//! Integration-style tests for [`Screen`] — full render-pipeline
//! checks for mode toggles, string painting, reset/restore, wide
//! glyphs, and `insert_above`.

use super::*;
use crate::text::{WidthMode, WrapMode};

#[test]
fn test_new_screen() {
    let screen = Screen::new(Vec::new(), (80, 24));
    assert_eq!(screen.width(), 80);
    assert_eq!(screen.height(), 24);
}

#[test]
fn test_write_and_render() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (20, 5));
        {
            screen.set_str((0, 0), "Hello", WrapMode::Truncate);
        };
        screen.render();
        screen.flush().unwrap();
    }
    assert!(String::from_utf8_lossy(&buf).contains("Hello"));
}

#[test]
fn test_alt_screen() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (80, 24));
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
    let screen = Screen::new(Vec::new(), (20, 1));
    assert_eq!(screen.width_mode(), WidthMode::Wc);
    assert!(!screen.eaw_wide());
}

#[test]
fn with_eaw_wide_sets_eaw_wide() {
    let screen = Screen::new(Vec::new(), (20, 1)).with_eaw_wide(true);
    assert!(screen.eaw_wide());
}

#[test]
fn str_width_follows_mode_and_eaw_and_ignores_escapes() {
    let mut screen = Screen::new(Vec::new(), (20, 1));
    // Plain CJK: two columns regardless of mode.
    assert_eq!(screen.str_width("中"), 2);
    // Inline SGR contributes no width.
    assert_eq!(screen.str_width("\x1b[31mhi\x1b[0m"), 2);
    // Ambiguous code point flips with eaw_wide.
    assert_eq!(screen.str_width("…"), 1);
    let wide = Screen::new(Vec::new(), (20, 1)).with_eaw_wide(true);
    assert_eq!(wide.str_width("…"), 2);
    // VS16 only matters once grapheme-cluster mode is on.
    assert_eq!(screen.str_width("\u{270b}\u{fe0e}"), 2);
    screen.set_grapheme_clusters(true);
    assert_eq!(screen.str_width("\u{270b}\u{fe0e}"), 1);
}

#[test]
fn grapheme_width_and_cells_use_screen_policy() {
    let mut screen = Screen::new(Vec::new(), (20, 1));
    // Wc mode is cluster-blind: the VS15 tail is ignored, base '✋' is 2.
    assert_eq!(screen.grapheme_width("\u{270b}\u{fe0e}"), 2);
    screen.set_grapheme_clusters(true);
    // Grapheme mode honours VS15 → text presentation, one column.
    assert_eq!(screen.grapheme_width("\u{270b}\u{fe0e}"), 1);

    let cells: Vec<_> = screen.grapheme_cells("Ae\u{0301}中").collect();
    assert_eq!(cells, vec![("A", 1), ("e\u{0301}", 1), ("中", 2)]);
}

#[test]
fn set_color_scheme_updates_emits_decset_2031_and_tracks_state() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (20, 1));
        assert!(!screen.color_scheme_updates());
        screen.set_color_scheme_updates(true);
        assert!(screen.color_scheme_updates());
        // Idempotent: second enable doesn't write again.
        screen.set_color_scheme_updates(true);
        screen.set_color_scheme_updates(false);
        assert!(!screen.color_scheme_updates());
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    assert!(out.contains("\x1b[?2031h"));
    assert!(out.contains("\x1b[?2031l"));
    assert_eq!(out.matches("\x1b[?2031h").count(), 1);
}

#[test]
fn set_grapheme_clusters_toggles_width_mode_and_emits_decset() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (20, 1));
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
    let mut screen = Screen::new(Vec::new(), (10, 1));
    {
        screen.set_str((0, 0), "e\u{0301}", WrapMode::Truncate);
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
        screen.set_str((0, 0), "e\u{0301}", WrapMode::Truncate);
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
        let mut screen = Screen::new(&mut buf, (20, 1));
        screen.set_grapheme_clusters(true);
        assert!(screen.grapheme_clusters());

        // reset: state preserved, teardown writes RM
        screen.reset();
        assert!(screen.grapheme_clusters());

        // restore: re-emits SM
        screen.restore();
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    // Enable SM, reset emits RM, restore emits SM again.
    assert!(out.matches("\x1b[?2027h").count() >= 2);
    assert!(out.contains("\x1b[?2027l"));
}

#[test]
fn set_in_band_resize_emits_decset_2048_and_tracks_state() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (20, 1));
        assert!(!screen.in_band_resize());
        screen.set_in_band_resize(true);
        assert!(screen.in_band_resize());
        // Idempotent: second enable doesn't write again.
        screen.set_in_band_resize(true);
        screen.set_in_band_resize(false);
        assert!(!screen.in_band_resize());
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    assert!(out.contains("\x1b[?2048h"));
    assert!(out.contains("\x1b[?2048l"));
    assert_eq!(out.matches("\x1b[?2048h").count(), 1);
}

#[test]
fn reset_and_restore_round_trip_in_band_resize() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (20, 1));
        screen.set_in_band_resize(true);
        assert!(screen.in_band_resize());

        // reset: state preserved, teardown writes RM
        screen.reset();
        assert!(screen.in_band_resize());

        // restore: re-emits SM
        screen.restore();
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    // Enable SM, reset emits RM, restore emits SM again.
    assert!(out.matches("\x1b[?2048h").count() >= 2);
    assert!(out.contains("\x1b[?2048l"));
}

#[test]
fn test_resize() {
    let mut screen = Screen::new(Vec::new(), (80, 24));
    screen.resize(100, 30);
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
        let mut screen = Screen::new(&mut frame1, (40, 5));
        screen.clear();
        {
            screen.set_str(
                (0, 2),
                "icu_segmenter = \"compiled_data\"",
                WrapMode::Truncate,
            );
        };
        screen.render();
        screen.flush().unwrap();
        // Swap writers so frame 2 lands in its own buffer while
        // the renderer's diff state carries over.
        let _ = std::mem::replace(&mut screen.writer, &mut frame2);
        screen.clear();
        screen.render();
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
        &frame2
    );
}

// --- end-to-end renderer tests ---
//
// Drive the renderer through the public Screen surface. The writer
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
// (with `set_str(..., WrapMode::Wrap)`), render both frames in a
// single Screen lifetime, and assert that the relevant content and
// clearing sequences appear in the cumulative byte stream.

use crate::color::{BasicColor, Color};
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
    screen.set_str((bounds.x, bounds.y), src, WrapMode::Wrap);
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
        let mut screen = Screen::new(&mut buf, (5, 3));
        screen.set_alt_screen(true);
        fill(&mut screen, 0, 0, "X");
        fill(&mut screen, 1, 1, "X");
        fill(&mut screen, 2, 2, "X");
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (80, 24));
        screen.set_str((0u16, 0u16), "Hello, World!", WrapMode::Truncate);
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (1, 1)).with_color_profile(Profile::TrueColor);
        screen.set_cell(
            (0u16, 0u16),
            &Cell::narrow("X").style(Style::default().fg(Color::rgb(255, 0, 0))),
        );
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (1, 1)).with_color_profile(Profile::Ansi256);
        screen.set_cell(
            (0u16, 0u16),
            &Cell::narrow("X").style(Style::default().fg(Color::rgb(255, 0, 0))),
        );
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (1, 1)).with_color_profile(Profile::Ansi);
        screen.set_cell(
            (0u16, 0u16),
            &Cell::narrow("X").style(Style::default().fg(Color::rgb(255, 0, 0))),
        );
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (1, 1)).with_color_profile(Profile::Ascii);
        screen.set_cell(
            (0u16, 0u16),
            &Cell::narrow("X").style(Style::default().fg(Color::rgb(255, 0, 0))),
        );
        screen.render();
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
    let mut screen = Screen::new(Vec::new(), (80, 24));
    screen.set_cursor_position(5, 10);
    let p = screen.cursor_position();
    assert_eq!((p.x, p.y), (5, 10));
}

#[test]
fn move_to_emits_relative_cursor_sequence() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (80, 24));
        screen.set_cursor_position(5, 3);
        screen.flush().unwrap();
    }
    // Inline mode at first move: CR, then 3 newlines down, then CUF 5.
    assert_eq!(s(&buf), "\r\n\n\n\x1b[5C");
}

#[test]
fn screen_write_passes_bytes_verbatim() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (10, 1));
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
        let mut screen = Screen::new(&mut buf, (3, 1));
        fill(&mut screen, 0, 0, "X");
        screen.render();
        screen.flush().unwrap();
        screen.invalidate();
        screen.render();
        screen.flush().unwrap();
    }
    assert!(s(&buf).matches('X').count() >= 2);
}

#[test]
fn resize_does_not_crash_and_renders_blank() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (40, 10));
        screen.resize(80, 24);
        screen.render();
        screen.flush().unwrap();
    }
}

// --- insert_above ---

#[test]
fn insert_above_emits_il_and_content() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (10, 5));
        screen.insert_above("Prepended line");
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (10, 5));
        screen.insert_above("Hello");
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (20, 10));
        screen.insert_above("First line");
        screen.insert_above("Second line");
        screen.insert_above("Third line\nFourth lin");
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (20, 1)).with_optimizations(opts);
        fill(&mut screen, 8, 0, "X");
        fill(&mut screen, 16, 0, "X");
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (20, 1)).with_optimizations(opts);
        fill(&mut screen, 8, 0, "X");
        fill(&mut screen, 16, 0, "X");
        screen.render();
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
fn backspace_optimization_on_emits_bs_for_leftward_move() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let opts = Optimizations::default().union(Optimizations::BS);
        let mut screen = Screen::new(&mut buf, (20, 5)).with_optimizations(opts);
        fill(&mut screen, 5, 0, "A");
        fill(&mut screen, 3, 1, "B");
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (20, 5)).with_optimizations(opts);
        fill(&mut screen, 5, 0, "A");
        fill(&mut screen, 3, 1, "B");
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (10, 3)).with_optimizations(opts);
        for y in 0..3u16 {
            fill(&mut screen, 0, y, "X");
        }
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (10, 3)).with_optimizations(opts);
        for y in 0..3u16 {
            fill(&mut screen, 0, y, "X");
        }
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (20, 1)).with_optimizations(opts);
        for x in 0..15u16 {
            fill(&mut screen, x, 0, "A");
        }
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (20, 1)).with_optimizations(opts);
        for x in 0..15u16 {
            fill(&mut screen, x, 0, "A");
        }
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (10, 5)).with_optimizations(opts);
        screen.set_alt_screen(true);
        for y in 0..5u16 {
            for x in 0..10u16 {
                screen.set_cell(
                    (x, y),
                    &Cell::narrow(char::from(b'A' + y as u8).to_string()),
                );
            }
        }
        screen.render();
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
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (10, 1));
        let wide = ["🌟", "中", "文", "字"];
        for (i, ch) in wide.iter().enumerate() {
            screen.set_cell((i as u16 * 2, 0u16), &Cell::wide(*ch));
        }
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (5, 1));
        screen.set_cell((0u16, 0u16), &Cell::narrow("a\u{0301}"));
        screen.render();
        screen.flush().unwrap();
    }
    assert!(s(&buf).contains("a\u{0301}"));
}

#[test]
fn styled_text_emits_specific_sgr_payloads() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (4, 1));
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
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (10, 1));
        let style = Style::default().link("https://example.com", "");
        for (i, ch) in "link".chars().enumerate() {
            screen.set_cell(
                (i as u16, 0u16),
                &Cell::narrow(ch.to_string()).style(style.clone()),
            );
        }
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (10, 1)).with_color_profile(Profile::Disabled);
        let style = Style::default().link("https://example.com", "");
        for (i, ch) in "link".chars().enumerate() {
            screen.set_cell(
                (i as u16, 0u16),
                &Cell::narrow(ch.to_string()).style(style.clone()),
            );
        }
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (5, 3));
        fill(&mut screen, 0, 0, "X");
        screen.render();
        screen.flush().unwrap();

        screen.resize(10, 6);
        fill(&mut screen, 0, 1, "X");
        screen.render();
        screen.flush().unwrap();
    }
    assert!(s(&buf).matches('X').count() >= 2);
    assert!(s(&buf).contains("\x1b[J"));
}

#[test]
fn scroll_optimization_default_keeps_bottom_row_glyph() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (10, 5));
        screen.set_alt_screen(true);
        for y in 0..5u16 {
            for x in 0..10u16 {
                screen.set_cell(
                    (x, y),
                    &Cell::narrow(char::from(b'A' + y as u8).to_string()),
                );
            }
        }
        screen.render();
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
        screen.render();
        screen.flush().unwrap();
    }
    assert!(s(&buf).contains('F'));
}

#[test]
fn empty_buffer_renders_without_panic() {
    let mut buf: Vec<u8> = Vec::new();
    let mut screen = Screen::new(&mut buf, (0, 0));
    screen.render();
    screen.flush().unwrap();
}

#[test]
fn large_buffer_renders_bottom_right_glyph() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (1000, 1000));
        screen.set_cell((999u16, 999u16), &Cell::narrow("X"));
        screen.render();
        screen.flush().unwrap();
    }
    assert!(s(&buf).contains('X'));
}

// --- style variants — each underline / attribute SGR is emitted ---

#[test]
fn underline_styles_emit_extended_sgr_params() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (10, 1));
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
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (5, 1));
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
        screen.render();
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
            let mut screen = Screen::new(&mut buf, (3, 1)).with_color_profile(profile);
            let cell = Cell::narrow("C").style(Style::default().fg(Color::rgb(123, 234, 45)));
            screen.set_cell((0u16, 0u16), &cell);
            screen.render();
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
        let mut screen = Screen::new(&mut buf, (5, 3));
        screen.set_alt_screen(true);
        for y in 0..3u16 {
            screen.set_cell((4u16, y), &Cell::narrow("X"));
        }
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (10, 3));
        for x in 0..10u16 {
            fill(&mut screen, x, 0, "X");
        }
        screen.render();
        screen.flush().unwrap();
        for x in 0..10u16 {
            let c = if x == 0 {
                Cell::narrow("X")
            } else {
                Cell::BLANK
            };
            screen.set_cell((x, 0u16), &c);
        }
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (20, 1)).with_optimizations(opts);
        for x in 0..15u16 {
            fill(&mut screen, x, 0, "A");
        }
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (20, 1)).with_optimizations(opts);
        for x in 0..20u16 {
            fill(&mut screen, x, 0, "X");
        }
        screen.render();
        screen.flush().unwrap();
        prime_len = screen.writer().len();
        fill(&mut screen, 0, 0, "A");
        for x in 1..19u16 {
            fill(&mut screen, x, 0, " ");
        }
        fill(&mut screen, 19, 0, "B");
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (20, 1)).with_optimizations(opts);
        for x in 0..20u16 {
            fill(&mut screen, x, 0, "X");
        }
        screen.render();
        screen.flush().unwrap();
        prime_len = screen.writer().len();
        fill(&mut screen, 0, 0, "A");
        for x in 1..19u16 {
            fill(&mut screen, x, 0, " ");
        }
        fill(&mut screen, 19, 0, "B");
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (3, 3));
        screen.set_cursor_position(1, 1);
        screen.set_alt_screen(true);
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (5, 1)).with_color_profile(Profile::Ansi);
        fill(&mut screen, 0, 0, "A");
        screen.render();
        screen.flush().unwrap();

        screen.set_cell(
            (0u16, 0u16),
            &Cell::narrow("A").style(Style::default().bold()),
        );
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (1, 1));
        let cell = Cell::narrow("X").style(Style::default().fg(Color::Basic(BasicColor::Red)));
        screen.set_cell((0u16, 0u16), &cell);
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (10, 5));

        screen.set_str((0u16, 0u16), "ABC", WrapMode::Truncate);
        screen.render();
        screen.flush().unwrap();

        screen.set_str((0u16, 0u16), "XXX", WrapMode::Truncate);
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (10, 5));
        screen.set_alt_screen(true);
        screen.invalidate();

        draw_wrapped(&mut screen, LOREM);
        screen.render();
        screen.flush().unwrap();

        blank_screen(&mut screen);
        screen.invalidate();
        draw_wrapped(&mut screen, &LOREM[10..]);
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (10, 5));
        screen.set_alt_screen(true);
        screen.invalidate();

        draw_wrapped(&mut screen, LOREM);
        screen.render();
        screen.flush().unwrap();

        blank_screen(&mut screen);
        screen.invalidate();
        draw_wrapped(&mut screen, &LOREM[20..]);
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (10, 5));
        screen.set_alt_screen(true);
        screen.invalidate();

        draw_wrapped(&mut screen, "ABC\nDEF\nGHI\n");
        screen.render();
        screen.flush().unwrap();

        blank_screen(&mut screen);
        draw_wrapped(&mut screen, "ABC\n\nDEF\nGHI");
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (10, 5));

        screen.set_str((0u16, 1u16), "ABCEFGHIJK", WrapMode::Truncate);
        screen.render();
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
        screen.render();
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
        let mut screen = Screen::new(&mut buf, (20, 4));
        screen.set_str((0, 0), "Hello", WrapMode::Truncate);
        screen.set_str((0, 1), "World", WrapMode::Truncate);
        screen.set_str((0, 2), "!!!!", WrapMode::Truncate);
        screen.render();
        screen.flush().unwrap();
        bytes_after_frame_one = screen.writer.len();

        // Frame 2: clear + identical redraw. Must produce no bytes.
        screen.clear();
        screen.set_str((0, 0), "Hello", WrapMode::Truncate);
        screen.set_str((0, 1), "World", WrapMode::Truncate);
        screen.set_str((0, 2), "!!!!", WrapMode::Truncate);
        screen.render();
        screen.flush().unwrap();

        let after = screen.writer.len();
        assert_eq!(
            after,
            bytes_after_frame_one,
            "expected zero bytes for identical redraw, got {} new bytes: {:?}",
            after - bytes_after_frame_one,
            String::from_utf8_lossy(&screen.writer[bytes_after_frame_one..])
        );
    }
}

#[test]
fn reset_moves_cursor_to_last_row_inline() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (20, 5));
        screen.set_str((0, 0), "hi", WrapMode::Truncate);
        screen.render();
        screen.reset();
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
        let mut screen = Screen::new(&mut buf, (20, 5));
        screen.set_alt_screen(true);
        screen.set_str((0, 0), "hi", WrapMode::Truncate);
        screen.render();
        screen.reset();
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
        let mut screen = Screen::new(&mut buf, (20, 5));
        screen.set_alt_screen(true);
        screen.set_str((0, 0), "hi", WrapMode::Truncate);
        screen.render();
        // Grow the screen. front_buf still reflects the 5-row render.
        screen.resize(20, 50);
        screen.reset();
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

// --- foreground/background/cursor color setters ---

#[test]
fn set_foreground_color_emits_osc_10_and_is_idempotent() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (20, 1));
        screen.set_foreground_color(Some(crate::color::Color::Rgb(255, 128, 0)));
        // Idempotent: same color does not re-emit.
        screen.set_foreground_color(Some(crate::color::Color::Rgb(255, 128, 0)));
        screen.set_foreground_color(None);
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    assert_eq!(out.matches("\x1b]10;").count(), 1);
    assert!(out.contains("\x1b]110\x07"));
}

#[test]
fn set_background_color_emits_osc_11_and_reset() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (20, 1));
        screen.set_background_color(Some(crate::color::Color::Rgb(0, 0, 255)));
        screen.set_background_color(None);
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    assert!(out.contains("\x1b]11;"));
    assert!(out.contains("\x1b]111\x07"));
}

#[test]
fn set_cursor_color_emits_osc_12_and_reset() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (20, 1));
        screen.set_cursor_color(Some(crate::color::Color::Rgb(0, 255, 0)));
        screen.set_cursor_color(None);
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    assert!(out.contains("\x1b]12;"));
    assert!(out.contains("\x1b]112\x07"));
}

#[test]
fn reset_clears_color_state_and_restore_reapplies() {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (20, 1));
        screen.set_foreground_color(Some(crate::color::Color::Rgb(10, 20, 30)));
        screen.set_background_color(Some(crate::color::Color::Rgb(40, 50, 60)));
        screen.set_cursor_color(Some(crate::color::Color::Rgb(70, 80, 90)));
        screen.reset();
        // State preserved across reset.
        assert!(screen.state.foreground_color.is_some());
        assert!(screen.state.background_color.is_some());
        assert!(screen.state.cursor_color.is_some());
        screen.restore();
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    // Reset emits the three OSC reset sequences.
    assert!(out.contains("\x1b]110\x07"));
    assert!(out.contains("\x1b]111\x07"));
    assert!(out.contains("\x1b]112\x07"));
    // Restore re-emits each set sequence (so the count is 2: initial + restore).
    assert_eq!(out.matches("\x1b]10;").count(), 2);
    assert_eq!(out.matches("\x1b]11;").count(), 2);
    assert_eq!(out.matches("\x1b]12;").count(), 2);
}

// --- kitty keyboard setter ---

#[test]
fn set_kitty_keyboard_flags_emits_set_and_is_idempotent() {
    use crate::ansi::KittyKeyboardFlags;
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (20, 1));
        screen.set_kitty_keyboard_flags(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
        // Idempotent.
        screen.set_kitty_keyboard_flags(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
        screen.set_kitty_keyboard_flags(KittyKeyboardFlags::NONE);
        screen.flush().unwrap();
    }
    let out = String::from_utf8_lossy(&buf);
    let bits = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES.bits();
    assert_eq!(out.matches(&format!("\x1b[={bits};1u")).count(), 1);
    assert!(out.contains("\x1b[=0;1u"));
}

#[test]
fn kitty_keyboard_reapplies_on_alt_screen_toggle() {
    use crate::ansi::KittyKeyboardFlags;
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (20, 1));
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
    use crate::ansi::KittyKeyboardFlags;
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut buf, (20, 1));
        screen.set_kitty_keyboard_flags(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
        screen.set_alt_screen(true);
        screen.reset();
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
    use crate::ansi::KittyKeyboardFlags;
    let mut setup: Vec<u8> = Vec::new();
    let mut restore_buf: Vec<u8> = Vec::new();
    {
        let mut screen = Screen::new(&mut setup, (20, 1));
        screen.set_kitty_keyboard_flags(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
        screen.set_alt_screen(true);
        screen.reset();
        // Swap writers so only restore-side bytes land in restore_buf.
        let _ = std::mem::replace(&mut screen.writer, &mut restore_buf);
        screen.restore();
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
