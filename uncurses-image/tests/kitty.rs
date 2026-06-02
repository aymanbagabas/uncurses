//! Smoke tests for the Kitty Unicode-placeholder backend.

use std::io::Write;

use image::{Rgba, RgbaImage};
use uncurses::Rect;
use uncurses::screen::{Capabilities, Screen};
use uncurses_image::{Image, ImageLayer, ImageProtocol, Resize};

fn solid_image(color: Rgba<u8>, w: u32, h: u32) -> Image {
    let mut buf = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            buf.put_pixel(x, y, color);
        }
    }
    Image::from_dynamic(image::DynamicImage::ImageRgba8(buf))
}

fn make_caps() -> Capabilities {
    Capabilities {
        kitty_graphics: Some(true),
        cell_pixel_size: Some((10, 20)),
        ..Default::default()
    }
}

#[test]
fn first_paint_emits_apc_transmit_and_placeholder_glyph() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(20, 5);
    let caps = make_caps();
    let mut layer = ImageLayer::new(&caps).with_protocol(ImageProtocol::Kitty);

    let id = layer.add(solid_image(Rgba([255, 0, 0, 255]), 20, 20));
    layer.place(
        id,
        Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        },
        Resize::Scale(image::imageops::FilterType::Triangle),
    );

    layer.render(&mut screen).expect("render");
    screen.flush().unwrap();
    let bytes = screen.writer().clone();
    let s = String::from_utf8_lossy(&bytes);

    // APC transmit: starts with ESC _G, ends with ESC \.
    assert!(s.contains("\x1b_G"), "expected APC transmit start in {s:?}");
    assert!(s.contains("U=1"), "expected unicode-placeholder mode flag");
    assert!(
        s.contains("a=T"),
        "expected virtual-placement transmit flag"
    );
    assert!(s.contains("f=32"), "expected RGBA format flag");
    // Placeholder code-point must reach the terminal.
    assert!(
        s.contains('\u{10EEEE}'),
        "expected placeholder code-point in frame"
    );
}

#[test]
fn auto_protocol_resolves_to_kitty_when_supported() {
    let caps = make_caps();
    let layer = ImageLayer::new(&caps);
    assert_eq!(layer.protocol(), ImageProtocol::Kitty);
}

#[test]
fn no_retransmit_for_unchanged_image() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(20, 5);
    let caps = make_caps();
    let mut layer = ImageLayer::new(&caps).with_protocol(ImageProtocol::Kitty);

    let id = layer.add(solid_image(Rgba([0, 200, 0, 255]), 20, 20));
    layer.place(
        id,
        Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        },
        Resize::Scale(image::imageops::FilterType::Triangle),
    );

    layer.render(&mut screen).unwrap();
    screen.flush().unwrap();
    screen.writer_mut().clear();

    // Steady-state frame: no transmit bytes.
    layer.render(&mut screen).unwrap();
    screen.flush().unwrap();
    let s = String::from_utf8_lossy(screen.writer());
    assert!(
        !s.contains("\x1b_G"),
        "steady-state frame should not retransmit, got: {s:?}"
    );
}

#[test]
fn remove_emits_terminal_side_delete() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(20, 5);
    let caps = make_caps();
    let mut layer = ImageLayer::new(&caps).with_protocol(ImageProtocol::Kitty);

    let id = layer.add(solid_image(Rgba([0, 0, 255, 255]), 20, 20));
    layer.place(
        id,
        Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        },
        Resize::default(),
    );
    layer.render(&mut screen).unwrap();
    screen.flush().unwrap();
    screen.writer_mut().clear();

    layer.remove(id);
    layer.render(&mut screen).unwrap();
    screen.flush().unwrap();
    let s = String::from_utf8_lossy(screen.writer());
    assert!(
        s.contains("a=d") && s.contains("d=I"),
        "expected image-delete APC after remove(), got: {s:?}"
    );
}

#[test]
fn resize_change_triggers_retransmit_for_crop_mode() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(20, 10);
    let caps = make_caps();
    let mut layer = ImageLayer::new(&caps).with_protocol(ImageProtocol::Kitty);

    let id = layer.add(solid_image(Rgba([100, 100, 100, 255]), 50, 50));
    layer.place(
        id,
        Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        },
        Resize::Crop(uncurses_image::CropAnchor::Center),
    );

    layer.render(&mut screen).unwrap();
    screen.flush().unwrap();
    screen.writer_mut().clear();

    // Move to a different sized area: Crop should pre-resize again.
    layer.place(
        id,
        Rect {
            x: 0,
            y: 0,
            width: 8,
            height: 4,
        },
        Resize::Crop(uncurses_image::CropAnchor::Center),
    );
    layer.render(&mut screen).unwrap();
    screen.flush().unwrap();
    let s = String::from_utf8_lossy(screen.writer());
    assert!(
        s.contains("\x1b_G"),
        "Crop placement resize should retransmit, got: {s:?}"
    );
}
