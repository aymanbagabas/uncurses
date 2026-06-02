//! Smoke tests for the half-blocks pipeline against a `Screen<Vec<u8>>`.

use image::{Rgba, RgbaImage};
use uncurses::Rect;
use uncurses::screen::{Capabilities, Screen};
use uncurses_image::{Image, ImageLayer, ImageProtocol, Resize};

fn make_test_image() -> Image {
    // 4x4 image: top half red, bottom half blue.
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
    Image::from_dynamic(image::DynamicImage::ImageRgba8(buf))
}

#[test]
fn halfblocks_round_trip_writes_bytes() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(10, 5);
    let caps = Capabilities::default();
    let mut layer = ImageLayer::new(&caps).with_protocol(ImageProtocol::HalfBlocks);
    assert_eq!(layer.protocol(), ImageProtocol::HalfBlocks);

    let id = layer.add(make_test_image());
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

    layer.render(&mut screen).expect("render");
    use std::io::Write;
    screen.flush().unwrap();
    let bytes = screen.writer().clone();
    assert!(!bytes.is_empty(), "expected the renderer to emit bytes");
    let s = String::from_utf8_lossy(&bytes);
    assert!(
        s.contains('\u{2580}'),
        "expected upper-half-block glyph, got: {s:?}"
    );
}

#[test]
fn auto_protocol_falls_back_without_caps() {
    let caps = Capabilities::default();
    let layer = ImageLayer::new(&caps);
    // Default capabilities = no raster support → halfblocks.
    assert_eq!(layer.protocol(), ImageProtocol::HalfBlocks);
}

#[test]
fn unplace_queues_erasure_and_paint_clears() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(10, 5);
    let caps = Capabilities::default();
    let mut layer = ImageLayer::new(&caps).with_protocol(ImageProtocol::HalfBlocks);

    let id = layer.add(make_test_image());
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
    use std::io::Write;
    screen.flush().unwrap();
    screen.writer_mut().clear();

    // Unplacing should queue an erasure; the next render wipes those
    // cells back to blanks.
    layer.unplace(id);
    layer.render(&mut screen).unwrap();
    screen.flush().unwrap();
    let bytes = screen.writer().clone();
    let s = String::from_utf8_lossy(&bytes);
    assert!(
        !s.contains('\u{2580}'),
        "expected the half-block glyph to be wiped, got: {s:?}"
    );
}

#[test]
fn invalidate_forces_repaint() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(10, 5);
    let caps = Capabilities::default();
    let mut layer = ImageLayer::new(&caps).with_protocol(ImageProtocol::HalfBlocks);

    let id = layer.add(make_test_image());
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
    use std::io::Write;
    screen.flush().unwrap();
    screen.writer_mut().clear();

    // No state change → next render is a no-op.
    layer.render(&mut screen).unwrap();
    screen.flush().unwrap();
    assert!(screen.writer().is_empty(), "no-op frame should be empty");

    // After invalidate the placement re-paints.
    layer.invalidate();
    screen.invalidate();
    layer.render(&mut screen).unwrap();
    screen.flush().unwrap();
    assert!(
        !screen.writer().is_empty(),
        "post-invalidate frame should emit bytes"
    );
}
