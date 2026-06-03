//! Smoke tests for the sixel painter.
//!
//! These tests only run when the `sixel` feature is enabled.

#![cfg(feature = "sixel")]

use std::io::Write as _;

use image::{DynamicImage, Rgba, RgbaImage};
use uncurses::Rect;
use uncurses::screen::Screen;
use uncurses_image::{Painter, Resize, Sixel};

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
fn paint_emits_dcs_sequence_at_anchor() {
    let mut screen = screen_with_pixels();
    let mut painter = Sixel::new();

    painter
        .paint(&mut screen, area(), &make_test_image(), Resize::default())
        .unwrap();
    screen.render().unwrap();
    screen.flush().unwrap();

    let bytes = screen.writer().clone();
    assert!(
        bytes.windows(2).any(|w| w == b"\x1bP"),
        "missing sixel DCS introducer"
    );
    assert!(
        bytes.windows(2).any(|w| w == b"\x1b\\"),
        "missing sixel DCS terminator"
    );
    assert!(bytes.contains(&b'q'), "missing DCS final byte 'q'");
}

#[test]
fn paint_no_op_when_cell_pixel_size_unknown() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(20, 10);
    let mut painter = Sixel::new();

    painter
        .paint(&mut screen, area(), &make_test_image(), Resize::default())
        .unwrap();
    screen.render().unwrap();
    screen.flush().unwrap();

    let bytes = screen.writer().clone();
    assert!(
        !bytes.windows(2).any(|w| w == b"\x1bP"),
        "expected no DCS without pixel size, got: {:?}",
        String::from_utf8_lossy(&bytes)
    );
}

#[test]
fn repeat_paint_reuses_cache() {
    let mut screen = screen_with_pixels();
    let mut painter = Sixel::new();

    painter
        .paint(&mut screen, area(), &make_test_image(), Resize::default())
        .unwrap();
    screen.render().unwrap();
    screen.flush().unwrap();
    let first = screen.writer().clone();
    screen.writer_mut().clear();

    painter
        .paint(&mut screen, area(), &make_test_image(), Resize::default())
        .unwrap();
    screen.render().unwrap();
    screen.flush().unwrap();
    let second = screen.writer().clone();

    // The renderer's diff sees the same anchor content + body cells
    // as the previous frame, so the second flush emits no sixel DCS
    // bytes at all.
    assert!(
        !second.windows(2).any(|w| w == b"\x1bP"),
        "second paint emitted a fresh DCS, expected diff to skip it"
    );
    assert!(
        first.windows(2).any(|w| w == b"\x1bP"),
        "first paint should emit DCS"
    );
}

#[test]
fn paint_with_different_resize_re_encodes() {
    use image::imageops::FilterType;

    let mut screen = screen_with_pixels();
    let mut painter = Sixel::new();

    painter
        .paint(
            &mut screen,
            area(),
            &make_test_image(),
            Resize::Fit(FilterType::Triangle),
        )
        .unwrap();
    screen.render().unwrap();
    screen.flush().unwrap();
    screen.writer_mut().clear();

    painter
        .paint(
            &mut screen,
            area(),
            &make_test_image(),
            Resize::Scale(FilterType::Nearest),
        )
        .unwrap();
    screen.render().unwrap();
    screen.flush().unwrap();
    let bytes = screen.writer().clone();
    assert!(
        bytes.windows(2).any(|w| w == b"\x1bP"),
        "switching Resize variant must produce a fresh DCS"
    );
}

#[test]
fn forget_drops_cache_and_re_encodes() {
    let mut screen = screen_with_pixels();
    let mut painter = Sixel::new();

    let id = painter
        .paint(&mut screen, area(), &make_test_image(), Resize::default())
        .unwrap();
    screen.render().unwrap();
    screen.flush().unwrap();
    screen.writer_mut().clear();

    painter.forget(&mut screen, id).unwrap();
    // Stamp a different image so the diff actually changes; without
    // a different anchor payload the renderer wouldn't re-emit even
    // though the cache was dropped.
    let mut alt = RgbaImage::new(8, 8);
    for y in 0..8 {
        for x in 0..8 {
            alt.put_pixel(x, y, Rgba([255 - (x * 32) as u8, 0, 0, 255]));
        }
    }
    painter
        .paint(
            &mut screen,
            area(),
            &DynamicImage::ImageRgba8(alt),
            Resize::default(),
        )
        .unwrap();
    screen.render().unwrap();
    screen.flush().unwrap();
    let bytes = screen.writer().clone();
    assert!(
        bytes.windows(2).any(|w| w == b"\x1bP"),
        "expected fresh DCS after forget + paint with different pixels"
    );
}
