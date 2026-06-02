//! Smoke tests for the iTerm2 inline-image backend.

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

fn caps_with_iterm2(cell_pixel_size: Option<(u16, u16)>) -> Capabilities {
    Capabilities {
        iterm2_graphics: Some(true),
        cell_pixel_size,
        ..Default::default()
    }
}

#[test]
fn auto_resolves_to_iterm2_when_supported() {
    let caps = caps_with_iterm2(Some((10, 20)));
    let layer = ImageLayer::new(&caps);
    assert_eq!(layer.protocol(), ImageProtocol::Iterm2);
}

#[test]
fn iterm2_works_without_cell_pixel_size() {
    // The terminal does cell-to-pixel scaling, so iTerm2 doesn't
    // need cell_pixel_size to be advertised.
    let caps = caps_with_iterm2(None);
    let layer = ImageLayer::new(&caps);
    assert_eq!(layer.protocol(), ImageProtocol::Iterm2);
}

#[test]
fn first_paint_emits_osc_1337_inline_image() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(20, 5);
    let caps = caps_with_iterm2(Some((10, 20)));
    let mut layer = ImageLayer::new(&caps).with_protocol(ImageProtocol::Iterm2);

    let id = layer.add(solid_image(Rgba([0, 200, 100, 255]), 20, 20));
    layer.place(
        id,
        Rect {
            x: 1,
            y: 1,
            width: 4,
            height: 2,
        },
        Resize::Scale(image::imageops::FilterType::Triangle),
    );

    layer.render(&mut screen).expect("render");
    screen.flush().unwrap();
    let bytes = screen.writer().clone();
    let s = String::from_utf8_lossy(&bytes);

    // OSC 1337 introducer + File= key.
    assert!(s.contains("\x1b]1337;File="), "expected OSC 1337 in {s:?}");
    assert!(s.contains("inline=1"), "expected inline=1 flag");
    assert!(s.contains("width=4"), "expected width=4 cells");
    assert!(s.contains("height=2"), "expected height=2 cells");
    assert!(
        s.contains("preserveAspectRatio=0"),
        "Scale → preserveAspectRatio=0"
    );
    assert!(
        s.ends_with('\x07') || s.contains('\x07'),
        "expected BEL terminator"
    );
    // DECSC / DECRC wrap.
    assert!(s.contains("\x1b7"), "expected DECSC");
    assert!(s.contains("\x1b8"), "expected DECRC");
}

#[test]
fn fit_uses_preserve_aspect_ratio_one() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(20, 5);
    let caps = caps_with_iterm2(Some((10, 20)));
    let mut layer = ImageLayer::new(&caps).with_protocol(ImageProtocol::Iterm2);

    let id = layer.add(solid_image(Rgba([100, 100, 100, 255]), 50, 50));
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
    let s = String::from_utf8_lossy(screen.writer());
    assert!(
        s.contains("preserveAspectRatio=1"),
        "Fit → preserveAspectRatio=1, got: {s:?}"
    );
}

#[test]
fn no_re_encode_for_unchanged_placement() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(20, 5);
    let caps = caps_with_iterm2(Some((10, 20)));
    let mut layer = ImageLayer::new(&caps).with_protocol(ImageProtocol::Iterm2);

    let id = layer.add(solid_image(Rgba([0, 0, 200, 255]), 20, 20));
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

    layer.render(&mut screen).unwrap();
    screen.flush().unwrap();
    let s = String::from_utf8_lossy(screen.writer());
    assert!(
        !s.contains("\x1b]1337"),
        "steady-state frame should not re-emit, got: {s:?}"
    );
}
