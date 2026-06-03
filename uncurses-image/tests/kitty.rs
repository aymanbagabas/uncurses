//! Smoke tests for the kitty unicode-placeholder painter.

use std::io::Write;

use image::{DynamicImage, Rgba, RgbaImage};
use uncurses::Rect;
use uncurses::screen::Screen;
use uncurses_image::{Kitty, Painter, Resize};

fn make_test_image() -> DynamicImage {
    let mut buf = RgbaImage::new(8, 8);
    for y in 0..8 {
        for x in 0..8 {
            buf.put_pixel(x, y, Rgba([(x * 32) as u8, (y * 32) as u8, 128, 255]));
        }
    }
    DynamicImage::ImageRgba8(buf)
}

fn screen_with_pixels() -> Screen<Vec<u8>> {
    let mut s: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(20, 10);
    // 20 cols × 10 rows × 10×20 px = 200×200 px window.
    s.set_window_size(uncurses::terminal::Winsize {
        row: 10,
        col: 20,
        xpixel: 200,
        ypixel: 200,
    });
    s
}

fn area() -> Rect {
    Rect {
        x: 1,
        y: 1,
        width: 4,
        height: 2,
    }
}

#[test]
fn paint_emits_transmit_and_placeholder() {
    let mut screen = screen_with_pixels();
    let mut painter = Kitty::new();

    painter
        .paint(&mut screen, area(), &make_test_image(), Resize::default())
        .unwrap();
    screen.render().unwrap();
    screen.flush().unwrap();

    let bytes = screen.writer().clone();
    let s = String::from_utf8_lossy(&bytes);
    assert!(
        s.contains("\x1b_G"),
        "missing kitty APC transmit, got: {s:?}"
    );
    assert!(
        s.contains('\u{10EEEE}'),
        "missing placeholder code-point, got: {s:?}"
    );
}

#[test]
fn repeat_paint_with_same_id_does_not_retransmit() {
    let mut screen = screen_with_pixels();
    let mut painter = Kitty::new();

    painter
        .paint(&mut screen, area(), &make_test_image(), Resize::default())
        .unwrap();
    screen.render().unwrap();
    screen.flush().unwrap();
    screen.writer_mut().clear();

    // Same host id, same cell rect → no new APC transmit (stamping
    // the same placeholder cells is a differ no-op too).
    painter
        .paint(&mut screen, area(), &make_test_image(), Resize::default())
        .unwrap();
    screen.render().unwrap();
    screen.flush().unwrap();
    let bytes = screen.writer().clone();
    let s = String::from_utf8_lossy(&bytes);
    assert!(
        !s.contains("\x1b_G"),
        "expected no retransmit on repeat paint, got: {s:?}"
    );
}

#[test]
fn forget_emits_delete_inline() {
    let mut screen = screen_with_pixels();
    let mut painter = Kitty::new();

    let id = painter
        .paint(&mut screen, area(), &make_test_image(), Resize::default())
        .unwrap();
    screen.render().unwrap();
    screen.flush().unwrap();
    screen.writer_mut().clear();

    painter.forget(&mut screen, id).unwrap();
    screen.flush().unwrap();
    let bytes = screen.writer().clone();
    let s = String::from_utf8_lossy(&bytes);
    assert!(
        s.contains("a=d,d=I"),
        "expected kitty delete sequence, got: {s:?}"
    );
}
