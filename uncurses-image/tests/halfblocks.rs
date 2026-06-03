//! Smoke tests for the half-blocks painter against a `Screen<Vec<u8>>`.

use std::io::Write;

use image::{DynamicImage, Rgba, RgbaImage};
use uncurses::Rect;
use uncurses::screen::Screen;
use uncurses_image::{Halfblocks, Painter, Resize};

fn make_test_image() -> DynamicImage {
    let mut buf = RgbaImage::new(4, 4);
    for y in 0..4 {
        for x in 0..4 {
            let pixel = if y < 2 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            };
            buf.put_pixel(x, y, pixel);
        }
    }
    DynamicImage::ImageRgba8(buf)
}

fn area() -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: 4,
        height: 2,
    }
}

#[test]
fn paint_writes_upper_half_glyph() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(10, 5);
    let mut painter = Halfblocks::new();

    painter
        .paint(&mut screen, area(), &make_test_image(), Resize::default())
        .unwrap();

    screen.render().unwrap();
    screen.flush().unwrap();
    let bytes = screen.writer().clone();
    let s = String::from_utf8_lossy(&bytes);
    assert!(
        s.contains('\u{2580}'),
        "expected upper-half-block glyph, got: {s:?}"
    );
}

#[test]
fn second_render_with_no_changes_is_a_no_op() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(10, 5);
    let mut painter = Halfblocks::new();

    painter
        .paint(&mut screen, area(), &make_test_image(), Resize::default())
        .unwrap();
    screen.render().unwrap();
    screen.flush().unwrap();
    screen.writer_mut().clear();

    // Stamping the same cells with the same content is a no-op for
    // the differ.
    painter
        .paint(&mut screen, area(), &make_test_image(), Resize::default())
        .unwrap();
    screen.render().unwrap();
    screen.flush().unwrap();
    assert!(screen.writer().is_empty(), "no-op frame should be empty");
}

#[test]
fn out_of_bounds_area_is_clipped_silently() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(4, 4);
    let mut painter = Halfblocks::new();

    let area = Rect {
        x: 100,
        y: 100,
        width: 50,
        height: 50,
    };
    painter
        .paint(&mut screen, area, &make_test_image(), Resize::default())
        .unwrap();

    screen.render().unwrap();
    screen.flush().unwrap();
    let bytes = screen.writer().clone();
    let s = String::from_utf8_lossy(&bytes);
    assert!(
        !s.contains('\u{2580}'),
        "clipped paint should emit no glyphs, got: {s:?}"
    );
}
