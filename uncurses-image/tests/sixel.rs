//! Smoke tests for the Sixel backend (only built with `--features sixel`).

#![cfg(feature = "sixel")]

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

fn caps_with_sixel() -> Capabilities {
    Capabilities {
        sixel: Some(true),
        cell_pixel_size: Some((10, 20)),
        ..Default::default()
    }
}

#[test]
fn auto_resolves_to_sixel_when_supported() {
    let caps = caps_with_sixel();
    let layer = ImageLayer::new();
    assert_eq!(layer.protocol(&caps), ImageProtocol::Sixel);
}

#[test]
fn first_paint_emits_dcs_sixel_sequence() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(20, 5);
    let caps = caps_with_sixel();
    let mut layer = ImageLayer::new().with_protocol(ImageProtocol::Sixel);

    let id = layer.add(solid_image(Rgba([200, 100, 50, 255]), 20, 20));
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

    layer.render(&caps, &mut screen).expect("render");
    screen.flush().unwrap();
    let bytes = screen.writer().clone();
    let s = String::from_utf8_lossy(&bytes);

    // DCS sixel introducer is `\x1bP…q` and string terminator `\x1b\\`.
    assert!(s.contains("\x1bP"), "expected DCS introducer in {s:?}");
    assert!(s.contains('q'), "expected sixel mode `q` parameter");
    // DECSC / DECRC wrap so cursor state survives.
    assert!(s.contains("\x1b7"), "expected DECSC before sixel");
    assert!(s.contains("\x1b8"), "expected DECRC after sixel");
}

#[test]
fn no_re_encode_for_unchanged_image() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(20, 5);
    let caps = caps_with_sixel();
    let mut layer = ImageLayer::new().with_protocol(ImageProtocol::Sixel);

    let id = layer.add(solid_image(Rgba([0, 200, 0, 255]), 20, 20));
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

    layer.render(&caps, &mut screen).unwrap();
    screen.flush().unwrap();
    screen.writer_mut().clear();

    // Steady-state frame: no diff, no paint, no sixel re-emission.
    layer.render(&caps, &mut screen).unwrap();
    screen.flush().unwrap();
    let s = String::from_utf8_lossy(screen.writer());
    assert!(
        !s.contains("\x1bP"),
        "steady-state frame should not re-emit sixel, got: {s:?}"
    );
}

#[test]
fn sixel_explicitly_falls_back_to_halfblocks_without_cell_pixels() {
    let caps = Capabilities {
        sixel: Some(true),
        cell_pixel_size: None,
        ..Default::default()
    };
    let layer = ImageLayer::new().with_protocol(ImageProtocol::Sixel);
    assert_eq!(layer.protocol(&caps), ImageProtocol::HalfBlocks);
}
