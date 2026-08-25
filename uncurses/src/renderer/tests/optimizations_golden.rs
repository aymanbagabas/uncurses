//! Golden tests pinning byte-exact renderer output across a matrix of
//! [`Optimizations`] presets and per-cap toggles.
//!
//! The planner picks the **shortest** candidate among the sequences
//! permitted by the active [`Optimizations`]. As a result:
//!
//! - Some on/off pairs produce identical bytes because the gated
//!   sequence ties (or loses) against a non-gated alternative.
//! - The presets ([`Optimizations::modern`], [`Optimizations::xterm`],
//!   …) usually emit the same bytes for medium-sized edits because
//!   the shortest path is the same.
//!
//! Tests here are still useful: they lock the planner's actual
//! choices in place, so any change to the planner's cost model or
//! tiebreaker is caught.

use super::*;
use crate::color::Color;
use crate::renderer::RenderBuffer;
use crate::cell::Cell;
use crate::style::Style;

fn renderer_with(opts: Optimizations) -> Renderer {
    let mut r = Renderer::new();
    r.set_optimizations(opts);
    r
}

fn render_to_vec(renderer: &mut Renderer, buf: &mut RenderBuffer) -> Vec<u8> {
    let mut out = Vec::new();
    renderer.render(&mut out, buf).unwrap();
    out
}

#[track_caller]
fn assert_golden(actual: Vec<u8>, expected: &[u8]) {
    assert_eq!(
        actual,
        expected,
        "\nactual bytes: {:?}\nactual text:  {}\n",
        String::from_utf8_lossy(&actual),
        String::from_utf8_lossy(&actual),
    );
}

/// Run `scenario` against `opts` with the host's line discipline granted
/// and withheld, pinning both. `TABS` and `BS` are never part of a
/// `$TERM` baseline — `Screen::init` grants them from the live terminal
/// state — so every preset has to render correctly either way.
#[track_caller]
fn assert_both(
    opts: Optimizations,
    scenario: fn(&mut Renderer) -> Vec<u8>,
    with_line_discipline: &[u8],
    without_line_discipline: &[u8],
) {
    let mut r = renderer_with(opts.with_tabs(true).with_bs(true));
    assert_golden(scenario(&mut r), with_line_discipline);
    let mut r = renderer_with(opts);
    assert_golden(scenario(&mut r), without_line_discipline);
}

/// `scenario` whose bytes are identical either way: no candidate move in
/// it is a viable tab or backspace. Still run both ways, so a change that
/// makes one of them viable has to be looked at.
#[track_caller]
fn assert_both_same(opts: Optimizations, scenario: fn(&mut Renderer) -> Vec<u8>, expected: &[u8]) {
    assert_both(opts, scenario, expected, expected);
}

fn set_text(buf: &mut RenderBuffer, y: u16, text: &str) {
    for (x, ch) in text.chars().enumerate() {
        buf.set_ref((x as u16, y), &Cell::narrow(ch));
    }
}

fn fill_row(buf: &mut RenderBuffer, y: u16, ch: &str) {
    for x in 0..buf.width() {
        buf.set_ref((x, y), &Cell::narrow(ch.chars().next().unwrap()));
    }
}

fn fill_alpha_rows(buf: &mut RenderBuffer) {
    for y in 0..buf.height() {
        for x in 0..buf.width() {
            buf.set_ref((x, y), &Cell::narrow(char::from(b'A' + y as u8)));
        }
    }
}

// ===========================================================================
// Capability toggles that genuinely flip emitted bytes.
// ===========================================================================

// --- REP --------------------------------------------------------------------

#[test]
fn rep_on_collapses_run_of_same_glyph() {
    let opts = Optimizations::none().union(Optimizations::REP);
    let mut r = renderer_with(opts);
    let mut buf = RenderBuffer::new(20, 1);
    for x in 0..15u16 {
        buf.set_ref((x, 0), &Cell::narrow('A'));
    }
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\rA\x1b[14b\r");
}

#[test]
fn rep_off_emits_literal_repeats() {
    let opts = Optimizations::none().difference(Optimizations::REP);
    let mut r = renderer_with(opts);
    let mut buf = RenderBuffer::new(20, 1);
    for x in 0..15u16 {
        buf.set_ref((x, 0), &Cell::narrow('A'));
    }
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\rAAAAAAAAAAAAAAA\r");
}

// --- TABS -------------------------------------------------------------------

#[test]
fn tabs_on_advances_with_tab_character() {
    let opts = Optimizations::none().union(Optimizations::TABS);
    let mut r = renderer_with(opts);
    let mut buf = RenderBuffer::new(40, 1);
    buf.set_ref((8, 0), &Cell::narrow('X'));
    buf.set_ref((16, 0), &Cell::narrow('Y'));
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\r\tX\tY\r");
}

#[test]
fn tabs_off_advances_with_cuf_and_overwrite() {
    let opts = Optimizations::none().difference(Optimizations::TABS);
    let mut r = renderer_with(opts);
    let mut buf = RenderBuffer::new(40, 1);
    buf.set_ref((8, 0), &Cell::narrow('X'));
    buf.set_ref((16, 0), &Cell::narrow('Y'));
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\r\x1b[8CX\x1b[7CY\r");
}

// --- ONLCR ------------------------------------------------------------------

#[test]
fn onlcr_on_joins_rows_with_bare_lf() {
    let opts = Optimizations::none().union(Optimizations::ONLCR);
    let mut r = renderer_with(opts);
    let mut buf = RenderBuffer::new(5, 3);
    set_text(&mut buf, 0, "AA");
    set_text(&mut buf, 1, "BB");
    set_text(&mut buf, 2, "CC");
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\rAA\nBB\nCC\r");
}

#[test]
fn onlcr_off_joins_rows_with_crlf() {
    let opts = Optimizations::none().difference(Optimizations::ONLCR);
    let mut r = renderer_with(opts);
    let mut buf = RenderBuffer::new(5, 3);
    set_text(&mut buf, 0, "AA");
    set_text(&mut buf, 1, "BB");
    set_text(&mut buf, 2, "CC");
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\rAA\r\nBB\r\nCC\r");
}

// ===========================================================================
// Single-cap snapshots — bytes are independent of the cap in these scenarios
// because the planner finds an equal or shorter alternative, but the goldens
// still pin behaviour for regression detection.
// ===========================================================================

#[test]
fn cha_enabled_jump_to_col_50_still_uses_cr_plus_cuf() {
    // CHA `\x1b[51G` ties with `\r\x1b[50C` at 5 bytes after the CR;
    // the planner picks the relative pair.
    let opts = Optimizations::none()
        .union(Optimizations::CHA)
        .difference(Optimizations::TABS | Optimizations::HPA);
    let mut r = renderer_with(opts);
    let mut buf = RenderBuffer::new(80, 1);
    buf.set_ref((50, 0), &Cell::narrow('X'));
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\r\x1b[50CX\r");
}

#[test]
fn hpa_enabled_jump_to_col_50_still_uses_cr_plus_cuf() {
    let opts = Optimizations::none()
        .union(Optimizations::HPA)
        .difference(Optimizations::CHA | Optimizations::TABS);
    let mut r = renderer_with(opts);
    let mut buf = RenderBuffer::new(80, 1);
    buf.set_ref((50, 0), &Cell::narrow('X'));
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\r\x1b[50CX\r");
}

#[test]
fn no_cha_no_hpa_jump_to_col_50_uses_cr_plus_cuf() {
    let opts = Optimizations::none()
        .difference(Optimizations::CHA | Optimizations::HPA | Optimizations::TABS);
    let mut r = renderer_with(opts);
    let mut buf = RenderBuffer::new(80, 1);
    buf.set_ref((50, 0), &Cell::narrow('X'));
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\r\x1b[50CX\r");
}

#[test]
fn vpa_enabled_jump_to_row_10_uses_cud_chain_anyway() {
    // After an empty first frame the planner has an (unknown, unknown)
    // cursor and walks down with `\r` plus 10× LF rather than a CUP
    // `\x1b[11;1H` (7 bytes).
    let opts = Optimizations::none().union(Optimizations::VPA);
    let mut r = renderer_with(opts);
    let mut buf = RenderBuffer::new(10, 20);
    let _ = render_to_vec(&mut r, &mut buf);
    buf.set_ref((0, 10), &Cell::narrow('X'));
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\r\n\n\n\n\n\n\n\n\n\nX\r\n\n\n\n\n\n\n\n\n");
}

#[test]
fn vpa_disabled_jump_to_row_10_uses_cud_chain() {
    let opts = Optimizations::none().difference(Optimizations::VPA);
    let mut r = renderer_with(opts);
    let mut buf = RenderBuffer::new(10, 20);
    let _ = render_to_vec(&mut r, &mut buf);
    buf.set_ref((0, 10), &Cell::narrow('X'));
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\r\n\n\n\n\n\n\n\n\n\nX\r\n\n\n\n\n\n\n\n\n");
}

#[test]
fn bs_step_overwrite_uses_cuf_because_cursor_parked_at_col_0() {
    // The renderer parks the cursor at column 0 after each render,
    // so the "step left" scenario is actually a step *right* (CUF 4)
    // regardless of whether BS is enabled.
    let opts_on = Optimizations::none().union(Optimizations::BS);
    let mut r = renderer_with(opts_on);
    let mut buf = RenderBuffer::new(10, 1);
    set_text(&mut buf, 0, "ABCDE");
    let _ = render_to_vec(&mut r, &mut buf);
    buf.set_ref((4, 0), &Cell::narrow('Z'));
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\x1b[4CZ");

    let opts_off = Optimizations::none().difference(Optimizations::BS);
    let mut r = renderer_with(opts_off);
    let mut buf = RenderBuffer::new(10, 1);
    set_text(&mut buf, 0, "ABCDE");
    let _ = render_to_vec(&mut r, &mut buf);
    buf.set_ref((4, 0), &Cell::narrow('Z'));
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\x1b[4CZ");
}

// --- ECH --------------------------------------------------------------------

#[test]
fn ech_clears_trailing_blanks_with_el_when_row_shrinks() {
    // When the rest of the row is already blank in the prior frame
    // the planner picks `\x1b[K` (EL) which is independent of ECH.
    let opts_on = Optimizations::none().union(Optimizations::ECH);
    let mut r = renderer_with(opts_on);
    let mut buf = RenderBuffer::new(20, 2);
    set_text(&mut buf, 0, "HELLO WORLD!!!!");
    let _ = render_to_vec(&mut r, &mut buf);
    set_text(&mut buf, 0, "HELLO");
    for x in 5..15u16 {
        buf.set_ref((x, 0), &Cell::narrow(' '));
    }
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\x1b[A\x1b[6C\x1b[K");
}

// --- SU/SD ------------------------------------------------------------------

#[test]
fn scroll_up_by_one_in_fullscreen_uses_lf_with_rep_terminator() {
    // With REP available the new bottom row collapses to `F\x1b[8b`
    // for the run, followed by a manual autowrap-off cell so the
    // cursor does not pre-wrap.
    let opts = Optimizations::none()
        .union(Optimizations::SU_SD)
        .union(Optimizations::REP);
    let mut r = renderer_with(opts);
    r.set_fullscreen(true);
    let mut buf = RenderBuffer::new(10, 5);
    fill_alpha_rows(&mut buf);
    let _ = render_to_vec(&mut r, &mut buf);
    for y in 0..4u16 {
        for x in 0..10u16 {
            buf.set_ref((x, y), &Cell::narrow(char::from(b'B' + y as u8)));
        }
    }
    fill_row(&mut buf, 4, "F");
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\r\nF\x1b[8b\x1b[?7lF\x1b[?7h");
}

#[test]
fn scroll_up_by_one_in_fullscreen_without_su_sd_falls_back_to_lf() {
    let opts = Optimizations::none()
        .difference(Optimizations::SU_SD | Optimizations::IL_DL | Optimizations::CSR)
        .union(Optimizations::REP);
    let mut r = renderer_with(opts);
    r.set_fullscreen(true);
    let mut buf = RenderBuffer::new(10, 5);
    fill_alpha_rows(&mut buf);
    let _ = render_to_vec(&mut r, &mut buf);
    for y in 0..4u16 {
        for x in 0..10u16 {
            buf.set_ref((x, y), &Cell::narrow(char::from(b'B' + y as u8)));
        }
    }
    fill_row(&mut buf, 4, "F");
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\r\nF\x1b[8b\x1b[?7lF\x1b[?7h");
}

// --- ICH --------------------------------------------------------------------

#[test]
fn ich_does_not_kick_in_for_2_char_insert_full_repaint_is_shorter() {
    // Inserting "**" at col 2 requires shifting 8 cells right. The
    // planner finds a 10-byte overwrite ("AB**CDEFGH") is shorter
    // than ICH(2) + the two new glyphs + re-painting the shifted run.
    let opts = Optimizations::none() | Optimizations::ICH;
    let mut r = renderer_with(opts);
    let mut buf = RenderBuffer::new(20, 1);
    set_text(&mut buf, 0, "ABCDEFGHIJ");
    let _ = render_to_vec(&mut r, &mut buf);
    set_text(&mut buf, 0, "AB**CDEFGH");
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"AB**CDEFGH");
}

// --- BCE --------------------------------------------------------------------

#[test]
fn bce_on_with_colored_blanks_paints_explicit_run() {
    // BCE only helps when an EL/ECH would carry the current bg into
    // the cleared cells. Here the row's existing trailing cells are
    // *default* (no bg), so colored blanks must be painted explicitly
    // regardless of BCE.
    let opts = Optimizations::none() | Optimizations::ECH | Optimizations::BCE;
    let mut r = renderer_with(opts);
    let mut buf = RenderBuffer::new(20, 1);
    set_text(&mut buf, 0, "HELLO WORLD!!!!");
    let _ = render_to_vec(&mut r, &mut buf);
    let red_bg = Style {
        bg: Some(Color::Indexed(1)),
        ..Style::default()
    };
    set_text(&mut buf, 0, "HELLO");
    for x in 5..15u16 {
        buf.set_ref((x, 0), &Cell::narrow(' ').with_style(red_bg));
    }
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\x1b[5C\x1b[48;5;1m          \x1b[m");
}

#[test]
fn bce_off_with_colored_blanks_paints_explicit_run() {
    let opts = (Optimizations::none() | Optimizations::ECH).difference(Optimizations::BCE);
    let mut r = renderer_with(opts);
    let mut buf = RenderBuffer::new(20, 1);
    set_text(&mut buf, 0, "HELLO WORLD!!!!");
    let _ = render_to_vec(&mut r, &mut buf);
    let red_bg = Style {
        bg: Some(Color::Indexed(1)),
        ..Style::default()
    };
    set_text(&mut buf, 0, "HELLO");
    for x in 5..15u16 {
        buf.set_ref((x, 0), &Cell::narrow(' ').with_style(red_bg));
    }
    let actual = render_to_vec(&mut r, &mut buf);
    assert_golden(actual, b"\x1b[5C\x1b[48;5;1m          \x1b[m");
}

// ===========================================================================
// Preset matrix: medium-sized scenario rendered under every well-known
// preset.
// ===========================================================================

fn scenario_paragraph_edit(r: &mut Renderer) -> Vec<u8> {
    let mut buf = RenderBuffer::new(40, 6);
    set_text(&mut buf, 0, "hello world");
    set_text(&mut buf, 2, "aaaaaaaa");
    set_text(&mut buf, 5, "footer line");
    let _ = render_to_vec(r, &mut buf);

    set_text(&mut buf, 0, "hello, world!");
    for x in 0..8u16 {
        buf.set_ref((x, 2), &Cell::narrow(if x < 2 { 'a' } else { ' ' }));
    }
    set_text(&mut buf, 4, "middle");

    render_to_vec(r, &mut buf)
}

#[test]
fn preset_modern_paragraph_edit() {
    assert_both_same(
        Optimizations::modern(),
        scenario_paragraph_edit,
        b"\x1b[5A\x1b[5C, world!\r\n\naa\x1b[K\r\n\nmiddle",
    );
}

#[test]
fn preset_xterm_paragraph_edit() {
    assert_both_same(
        Optimizations::xterm(),
        scenario_paragraph_edit,
        b"\x1b[5A\x1b[5C, world!\r\n\naa\x1b[K\r\n\nmiddle",
    );
}

#[test]
fn preset_linux_paragraph_edit() {
    assert_both_same(
        Optimizations::linux(),
        scenario_paragraph_edit,
        b"\x1b[5A\x1b[5C, world!\r\n\naa\x1b[K\r\n\nmiddle",
    );
}

#[test]
fn preset_screen_paragraph_edit() {
    assert_both_same(
        Optimizations::screen(),
        scenario_paragraph_edit,
        b"\x1b[5A\x1b[5C, world!\r\n\naa\x1b[K\r\n\nmiddle",
    );
}

#[test]
fn preset_vt100_paragraph_edit() {
    assert_both_same(
        Optimizations::vt100(),
        scenario_paragraph_edit,
        b"\x1b[5A\x1b[5C, world!\r\n\naa\x1b[K\r\n\nmiddle",
    );
}

#[test]
fn preset_none_paragraph_edit() {
    assert_both_same(
        Optimizations::none(),
        scenario_paragraph_edit,
        b"\x1b[5A\x1b[5C, world!\r\n\naa\x1b[K\r\n\nmiddle",
    );
}

// ===========================================================================
// Long-run scenarios that *do* reveal preset differences.
// ===========================================================================

/// A row filled with a 30-character run of the same glyph. REP turns
/// this into a single `\x1b[Nb` whereas without REP the run is
/// emitted verbatim.
fn scenario_long_run(r: &mut Renderer) -> Vec<u8> {
    let mut buf = RenderBuffer::new(40, 1);
    for x in 0..30u16 {
        buf.set_ref((x, 0), &Cell::narrow('='));
    }
    render_to_vec(r, &mut buf)
}

#[test]
fn preset_modern_long_run_uses_rep() {
    assert_both_same(Optimizations::modern(), scenario_long_run, b"\r=\x1b[29b\r");
}

#[test]
fn preset_xterm_long_run_falls_back_to_literal() {
    // xterm preset disables REP.
    assert_both_same(
        Optimizations::xterm(),
        scenario_long_run,
        b"\r==============================\r",
    );
}

#[test]
fn preset_linux_long_run_falls_back_to_literal() {
    // linux preset disables REP.
    assert_both_same(
        Optimizations::linux(),
        scenario_long_run,
        b"\r==============================\r",
    );
}

#[test]
fn preset_screen_long_run_falls_back_to_literal() {
    // screen preset disables REP.
    assert_both_same(
        Optimizations::screen(),
        scenario_long_run,
        b"\r==============================\r",
    );
}

#[test]
fn preset_vt100_long_run_falls_back_to_literal() {
    assert_both_same(
        Optimizations::vt100(),
        scenario_long_run,
        b"\r==============================\r",
    );
}

/// Sparse-glyphs scenario: glyphs land on tab stops at columns 8, 16,
/// 24, 32. With TABS on the cursor advances via `\t`; without TABS
/// the advance is CUF.
fn scenario_sparse_glyphs(r: &mut Renderer) -> Vec<u8> {
    let mut buf = RenderBuffer::new(40, 1);
    buf.set_ref((8, 0), &Cell::narrow('A'));
    buf.set_ref((16, 0), &Cell::narrow('B'));
    buf.set_ref((24, 0), &Cell::narrow('C'));
    buf.set_ref((32, 0), &Cell::narrow('D'));
    render_to_vec(r, &mut buf)
}

#[test]
fn preset_modern_sparse_glyphs() {
    // With same-run skipping, the 7-blank gaps between glyphs are
    // jumped over with a tab rather than repainted via REP.
    assert_both(
        Optimizations::modern(),
        scenario_sparse_glyphs,
        b"\r\tA\tB\tC\tD\r",
        b"\r\x1b[8CA\x1b[7CB\x1b[7CC\x1b[7CD\r",
    );
}

#[test]
fn preset_xterm_sparse_glyphs() {
    assert_both(
        Optimizations::xterm(),
        scenario_sparse_glyphs,
        b"\r\tA\tB\tC\tD\r",
        b"\r\x1b[8CA\x1b[7CB\x1b[7CC\x1b[7CD\r",
    );
}

#[test]
fn preset_vt100_sparse_glyphs() {
    assert_both(
        Optimizations::vt100(),
        scenario_sparse_glyphs,
        b"\r\tA\tB\tC\tD\r",
        b"\r\x1b[8CA\x1b[7CB\x1b[7CC\x1b[7CD\r",
    );
}
