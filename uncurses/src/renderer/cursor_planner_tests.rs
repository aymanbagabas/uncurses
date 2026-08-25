use super::tabstops::TabStops;
use super::*;
use crate::layout::Position;
use crate::cell::Cell;
use crate::style::Style;

fn movement_opts() -> Optimizations {
    let mut opts = Optimizations::none();
    opts.remove(Optimizations::TABS);
    opts.remove(Optimizations::CBT);
    opts.remove(Optimizations::BS);
    opts.remove(Optimizations::CHA);
    opts.remove(Optimizations::HPA);
    opts.remove(Optimizations::VPA);
    opts
}

fn setup_renderer(width: u16, height: u16, opts: Optimizations) -> Renderer {
    let mut r = Renderer::new();
    r.set_optimizations(opts);
    r.last_width = width;
    r.last_height = height;
    r.tabs = TabStops::default_for(width);
    r.cur_buf = Some(RenderBuffer::new(width, height));
    r.old_hashes = vec![0u64; height as usize];
    r.cur.x = Some(0);
    r.cur.y = Some(0);
    r
}

fn planned_move(r: &mut Renderer, from: Position, to: Position) -> Vec<u8> {
    let mut actual = Vec::new();
    r.write_optimal_move(&mut actual, from, to, None).unwrap();
    actual
}

fn relative_move(
    r: &mut Renderer,
    from: Position,
    to: Position,
    line: Option<&[Cell]>,
    use_tabs: bool,
    use_backspace: bool,
) -> Vec<u8> {
    let mut actual = Vec::new();
    r.relative_cursor_move(&mut actual, from, to, line, use_tabs, use_backspace)
        .unwrap();
    actual
}

fn assert_bytes_eq(actual: &[u8], expected: &[u8]) {
    assert_eq!(
        actual,
        expected,
        "actual = {:?}",
        std::str::from_utf8(actual)
    );
}

#[test]
fn prefix_none_wins_when_already_at_target_column() {
    let mut r = setup_renderer(80, 24, movement_opts());
    r.set_relative_cursor(false);

    let actual = planned_move(&mut r, Position { x: 5, y: 3 }, Position { x: 5, y: 7 });

    assert_bytes_eq(&actual, b"\n\n\n\n");
}

#[test]
fn prefix_cr_wins_when_target_column_zero_distant() {
    let mut r = setup_renderer(80, 24, movement_opts());
    r.set_relative_cursor(false);

    let actual = planned_move(&mut r, Position { x: 50, y: 3 }, Position { x: 0, y: 5 });

    assert_bytes_eq(&actual, b"\r\n\n");
}

#[test]
fn prefix_cup_wins_when_far_diagonal_jump() {
    let mut r = setup_renderer(120, 40, movement_opts());
    r.set_relative_cursor(false);

    let actual = planned_move(&mut r, Position { x: 0, y: 0 }, Position { x: 60, y: 20 });

    assert_bytes_eq(&actual, b"\x1b[21;61H");
}

#[test]
fn prefix_tie_favors_earlier_prefix() {
    let mut opts = movement_opts();
    opts.insert(Optimizations::BS);
    let mut r = setup_renderer(80, 24, opts);

    let actual = planned_move(&mut r, Position { x: 1, y: 0 }, Position { x: 0, y: 0 });

    assert_bytes_eq(&actual, b"\r");
}

#[test]
fn cup_fast_path_skipped_in_relative_mode() {
    let mut r = setup_renderer(120, 40, movement_opts());
    r.set_relative_cursor(true);

    let actual = planned_move(&mut r, Position { x: 0, y: 0 }, Position { x: 60, y: 20 });

    assert!(!actual.windows(9).any(|w| w == b"\x1b[21;61H"));
    assert_bytes_eq(&actual, b"\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\x1b[60C");
}

#[test]
fn last_width_zero_forces_cup_fast_path() {
    let mut r = setup_renderer(0, 24, movement_opts());
    r.set_relative_cursor(false);

    let actual = planned_move(&mut r, Position { x: 0, y: 0 }, Position { x: 1, y: 0 });

    assert_bytes_eq(&actual, b"\x1b[1;2H");
}

#[test]
fn planner_with_no_tabs_no_backspace() {
    let mut r = setup_renderer(80, 24, movement_opts());

    let actual = planned_move(&mut r, Position { x: 0, y: 0 }, Position { x: 8, y: 0 });

    assert_bytes_eq(&actual, b"\x1b[8C");
}

#[test]
fn planner_with_tabs_only() {
    let mut opts = movement_opts();
    opts.insert(Optimizations::TABS);
    let mut r = setup_renderer(80, 24, opts);

    let actual = planned_move(&mut r, Position { x: 0, y: 0 }, Position { x: 8, y: 0 });

    assert_bytes_eq(&actual, b"\t");
    assert!(!actual.contains(&b'\x08'));
}

#[test]
fn planner_with_backspace_only() {
    let mut opts = movement_opts();
    opts.insert(Optimizations::BS);
    let mut r = setup_renderer(80, 24, opts);

    let actual = planned_move(&mut r, Position { x: 10, y: 0 }, Position { x: 8, y: 0 });

    assert_bytes_eq(&actual, b"\x08\x08");
    assert!(!actual.contains(&b'\t'));
}

#[test]
fn planner_with_tabs_and_backspace() {
    let mut opts = movement_opts();
    opts.insert(Optimizations::TABS);
    opts.insert(Optimizations::CBT);
    opts.insert(Optimizations::BS);
    let mut r = setup_renderer(80, 24, opts);

    let actual = planned_move(&mut r, Position { x: 9, y: 0 }, Position { x: 8, y: 0 });

    assert_bytes_eq(&actual, b"\x08");
}

#[test]
fn tabs_beat_cuf_for_horizontal_jump() {
    let mut r = setup_renderer(16, 4, movement_opts());

    let actual = relative_move(
        &mut r,
        Position { x: 0, y: 0 },
        Position { x: 8, y: 0 },
        None,
        true,
        false,
    );

    assert_bytes_eq(&actual, b"\t");
}

#[test]
fn backspace_beats_cub_for_short_leftward() {
    let mut r = setup_renderer(80, 24, movement_opts());

    let actual = relative_move(
        &mut r,
        Position { x: 10, y: 0 },
        Position { x: 8, y: 0 },
        None,
        false,
        true,
    );

    assert_bytes_eq(&actual, b"\x08\x08");
}

#[test]
fn styled_overwrite_beats_cuf() {
    let mut r = setup_renderer(80, 24, movement_opts());
    let style = Style::default().bold();
    r.cur.set_style(style);
    let mut line = vec![Cell::default(); 80];
    line[0] = Cell::narrow('A').with_style(style);
    line[1] = Cell::narrow('B').with_style(style);

    let actual = relative_move(
        &mut r,
        Position { x: 0, y: 0 },
        Position { x: 2, y: 0 },
        Some(&line),
        false,
        false,
    );

    assert_bytes_eq(&actual, b"AB");
}

#[test]
fn ri_used_at_row_one() {
    let mut r = setup_renderer(80, 24, movement_opts());

    let actual = relative_move(
        &mut r,
        Position { x: 0, y: 1 },
        Position { x: 0, y: 0 },
        None,
        false,
        false,
    );

    assert_bytes_eq(&actual, b"\x1b[A");
}

#[test]
fn vpa_beats_cud_for_long_vertical() {
    let mut opts = movement_opts();
    opts.insert(Optimizations::VPA);
    let mut r = setup_renderer(80, 120, opts);
    r.set_relative_cursor(false);
    r.set_fullscreen(true);

    let actual = relative_move(
        &mut r,
        Position { x: 0, y: 0 },
        Position { x: 0, y: 20 },
        None,
        false,
        false,
    );

    assert_bytes_eq(&actual, b"\x1b[21d");
}

#[test]
fn hpa_beats_cha_when_both_available() {
    let mut opts = movement_opts();
    opts.insert(Optimizations::HPA);
    opts.insert(Optimizations::CHA);
    let mut r = setup_renderer(140, 24, opts);
    r.set_relative_cursor(false);

    let actual = relative_move(
        &mut r,
        Position { x: 0, y: 0 },
        Position { x: 100, y: 0 },
        None,
        false,
        false,
    );

    assert_bytes_eq(&actual, b"\x1b[101`");
}

#[test]
fn cha_used_when_only_cha_available() {
    let mut opts = movement_opts();
    opts.insert(Optimizations::CHA);
    let mut r = setup_renderer(140, 24, opts);
    r.set_relative_cursor(false);

    let actual = relative_move(
        &mut r,
        Position { x: 0, y: 0 },
        Position { x: 100, y: 0 },
        None,
        false,
        false,
    );

    assert_bytes_eq(&actual, b"\x1b[101G");
}

#[test]
fn lf_preferred_for_one_down_inline() {
    let mut r = setup_renderer(80, 24, movement_opts());

    let actual = relative_move(
        &mut r,
        Position { x: 0, y: 5 },
        Position { x: 0, y: 6 },
        None,
        false,
        false,
    );

    assert_bytes_eq(&actual, b"\n");
}

#[test]
fn cht_collapses_many_tabs() {
    let mut opts = movement_opts();
    opts.insert(Optimizations::TABS);
    opts.insert(Optimizations::CHT);
    let mut r = setup_renderer(200, 24, opts);

    // 0 -> 128 spans 16 tab stops. CUF=`\x1b[128C` (7 bytes),
    // raw tabs=16 bytes, CHT=`\x1b[16I` (6 bytes) wins.
    let actual = planned_move(&mut r, Position { x: 0, y: 0 }, Position { x: 128, y: 0 });

    assert_bytes_eq(&actual, b"\x1b[16I");
}

#[test]
fn cht_skipped_when_raw_tabs_are_shorter() {
    let mut opts = movement_opts();
    opts.insert(Optimizations::TABS);
    opts.insert(Optimizations::CHT);
    let mut r = setup_renderer(80, 24, opts);

    // 0 -> 16 spans 2 tab stops. Two `\t` bytes beat both
    // CUF (`\x1b[16C`, 5 bytes) and CHT (`\x1b[2I`, 4 bytes).
    let actual = planned_move(&mut r, Position { x: 0, y: 0 }, Position { x: 16, y: 0 });

    assert_bytes_eq(&actual, b"\t\t");
}

#[test]
fn cht_off_falls_back_to_cuf_for_long_runs() {
    let mut opts = movement_opts();
    opts.insert(Optimizations::TABS);
    opts.remove(Optimizations::CHT);
    let mut r = setup_renderer(200, 24, opts);

    // 0 -> 128: CUF (7 bytes) beats 16 raw tabs (16 bytes) when CHT is off.
    let actual = planned_move(&mut r, Position { x: 0, y: 0 }, Position { x: 128, y: 0 });

    assert_bytes_eq(&actual, b"\x1b[128C");
}
