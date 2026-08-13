use super::*;
use crate::cell::Cell;
use crate::renderer::RenderBuffer;

fn renderer() -> Renderer {
    renderer_with(Optimizations::none())
}

/// `TABS` and `BS` come from the host's line discipline, granted by
/// `Screen::init`, never by a `$TERM` baseline. Tests that exercise
/// either one ask for it here and pin the fallback separately.
fn renderer_with_line_discipline() -> Renderer {
    renderer_with(Optimizations::none().with_tabs(true).with_bs(true))
}

fn renderer_with(opts: Optimizations) -> Renderer {
    let mut renderer = Renderer::new();
    renderer.set_optimizations(opts);
    renderer
}

fn render_to_vec(renderer: &mut Renderer, buf: &mut RenderBuffer) -> Vec<u8> {
    let mut out = Vec::new();
    renderer.render(&mut out, buf).unwrap();
    out
}

fn assert_golden(actual: Vec<u8>, expected: &[u8]) {
    assert_eq!(
        actual,
        expected,
        "actual bytes: {:?}\nactual text: {}",
        String::from_utf8_lossy(&actual),
        String::from_utf8_lossy(&actual),
    );
}

fn set_text(buf: &mut RenderBuffer, y: u16, text: &str) {
    for (x, ch) in text.chars().enumerate() {
        buf.set_cell((x as u16, y), &Cell::narrow(ch.to_string()));
    }
}

fn fill_distinct_rows(buf: &mut RenderBuffer) {
    for y in 0..buf.height() {
        let text = format!("row {y:02}");
        set_text(buf, y, &text);
    }
}

#[test]
fn golden_empty_frame_80x24() {
    let mut renderer = renderer();
    let mut buf = RenderBuffer::new(80, 24);

    let actual = render_to_vec(&mut renderer, &mut buf);

    assert_golden(actual, b"");
}

#[test]
fn golden_single_cell_change_at_origin() {
    let mut renderer = renderer();
    let mut buf = RenderBuffer::new(80, 24);
    let _ = render_to_vec(&mut renderer, &mut buf);
    buf.set_cell((0, 0), &Cell::narrow("X"));

    let actual = render_to_vec(&mut renderer, &mut buf);

    assert_golden(
        actual,
        b"\rX\r\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n",
    );
}

#[test]
fn golden_single_cell_change_at_middle() {
    let mut renderer = renderer();
    let mut buf = RenderBuffer::new(80, 24);
    let _ = render_to_vec(&mut renderer, &mut buf);
    buf.set_cell((40, 12), &Cell::narrow("X"));

    let actual = render_to_vec(&mut renderer, &mut buf);

    assert_golden(
        actual,
        b"\r\n\n\n\n\n\n\n\n\n\n\n\n\x1b[40CX\r\n\n\n\n\n\n\n\n\n\n\n",
    );
}

fn scroll_up_by_1_full_width(renderer: &mut Renderer) -> Vec<u8> {
    let mut first = RenderBuffer::new(80, 24);
    fill_distinct_rows(&mut first);
    let _ = render_to_vec(renderer, &mut first);

    let mut second = RenderBuffer::new(80, 24);
    for y in 0..23 {
        let text = format!("row {:02}", y + 1);
        set_text(&mut second, y, &text);
    }

    render_to_vec(renderer, &mut second)
}

#[test]
fn golden_scroll_up_by_1_full_width() {
    let actual = scroll_up_by_1_full_width(&mut renderer_with_line_discipline());
    assert_golden(
        actual,
        b"\x1b[J\x1b[23A\x1b[5C1\n\x082\n\x083\n\x084\n\x085\n\x086\n\x087\n\x088\n\x089\n\x08\x0810\n\x081\n\x082\n\x083\n\x084\n\x085\n\x086\n\x087\n\x088\n\x089\n\x08\x0820\n\x081\n\x082\n\x083",
    );
}

/// Same frame with the host withholding `BS`: every one-column step back
/// onto the changed digit has to become an escape sequence instead.
#[test]
fn golden_scroll_up_by_1_full_width_without_line_discipline() {
    let actual = scroll_up_by_1_full_width(&mut renderer());
    assert_golden(actual, b"\x1b[J\x1b[23A\x1b[5C1\n\x1b[D2\n\x1b[D3\n\x1b[D4\n\x1b[D5\n\x1b[D6\n\x1b[D7\n\x1b[D8\n\x1b[D9\n\x1b[2D10\n\x1b[D1\n\x1b[D2\n\x1b[D3\n\x1b[D4\n\x1b[D5\n\x1b[D6\n\x1b[D7\n\x1b[D8\n\x1b[D9\n\x1b[2D20\n\x1b[D1\n\x1b[D2\n\x1b[D3");
}

#[test]
fn golden_force_clear_frame() {
    let mut renderer = renderer();
    let mut buf = RenderBuffer::new(80, 24);
    let _ = render_to_vec(&mut renderer, &mut buf);
    renderer.request_clear();

    let actual = render_to_vec(&mut renderer, &mut buf);

    assert_golden(
        actual,
        b"\r\x1b[J\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n",
    );
}

#[test]
fn golden_fullscreen_mode_first_frame() {
    let mut renderer = renderer();
    renderer.set_fullscreen(true);
    let mut buf = RenderBuffer::new(80, 24);
    set_text(&mut buf, 0, "top");
    set_text(&mut buf, 12, "middle");
    set_text(&mut buf, 23, "bottom");

    let actual = render_to_vec(&mut renderer, &mut buf);

    assert_golden(actual, b"top\r\x1b[12Bmiddle\r\x1b[11Bbottom");
}

#[test]
fn golden_relative_cursor_mode() {
    let mut renderer = renderer();
    renderer.set_relative_cursor(true);
    let mut buf = RenderBuffer::new(80, 24);
    let _ = render_to_vec(&mut renderer, &mut buf);
    buf.set_cell((0, 5), &Cell::narrow("X"));

    let actual = render_to_vec(&mut renderer, &mut buf);

    assert_golden(
        actual,
        b"\r\n\n\n\n\nX\r\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n",
    );
}
