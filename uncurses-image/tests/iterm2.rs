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
    let layer = ImageLayer::new();
    assert_eq!(layer.protocol(&caps), ImageProtocol::Iterm2);
}

#[test]
fn iterm2_works_without_cell_pixel_size() {
    // The terminal does cell-to-pixel scaling, so iTerm2 doesn't
    // need cell_pixel_size to be advertised.
    let caps = caps_with_iterm2(None);
    let layer = ImageLayer::new();
    assert_eq!(layer.protocol(&caps), ImageProtocol::Iterm2);
}

#[test]
fn first_paint_emits_osc_1337_inline_image() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(20, 5);
    let caps = caps_with_iterm2(Some((10, 20)));
    let mut layer = ImageLayer::new().with_protocol(ImageProtocol::Iterm2);

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

    layer.render(&caps, &mut screen).expect("render");
    screen.flush().unwrap();
    let bytes = screen.writer().clone();
    let s = String::from_utf8_lossy(&bytes);

    // OSC 1337 introducer + File= key.
    assert!(s.contains("\x1b]1337;File="), "expected OSC 1337 in {s:?}");
    assert!(s.contains("inline=1"), "expected inline=1 flag");
    // With cell_pixel_size=(10, 20) and a 4x2 cell area we should
    // see `width=40px;height=40px`.
    assert!(s.contains("width=40px"), "expected width=40px");
    assert!(s.contains("height=40px"), "expected height=40px");
    assert!(
        s.contains("preserveAspectRatio=0"),
        "Scale → preserveAspectRatio=0"
    );
    assert!(
        s.ends_with('\x07') || s.contains('\x07'),
        "expected BEL terminator"
    );
}

#[test]
fn fit_uses_preserve_aspect_ratio_one() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(20, 5);
    let caps = caps_with_iterm2(Some((10, 20)));
    let mut layer = ImageLayer::new().with_protocol(ImageProtocol::Iterm2);

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
    layer.render(&caps, &mut screen).unwrap();
    screen.flush().unwrap();
    let s = String::from_utf8_lossy(screen.writer());
    // The image is fit (and padded) to the exact cell-box pixel
    // dimensions on the host side, so the OSC always asks the
    // terminal to blit at exactly those dimensions without applying
    // its own aspect-ratio fitting on top.
    assert!(
        s.contains("preserveAspectRatio=0"),
        "Fit → preserveAspectRatio=0, got: {s:?}"
    );
}

#[test]
fn no_re_encode_for_unchanged_placement() {
    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(20, 5);
    let caps = caps_with_iterm2(Some((10, 20)));
    let mut layer = ImageLayer::new().with_protocol(ImageProtocol::Iterm2);

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

    layer.render(&caps, &mut screen).unwrap();
    screen.flush().unwrap();
    screen.writer_mut().clear();

    layer.render(&caps, &mut screen).unwrap();
    screen.flush().unwrap();
    let s = String::from_utf8_lossy(screen.writer());
    assert!(
        !s.contains("\x1b]1337"),
        "steady-state frame should not re-emit, got: {s:?}"
    );
}

#[test]
fn large_payload_uses_multipart_form() {
    use image::imageops::FilterType;

    // Build an image whose PNG encoding (+ base64) easily exceeds
    // the 1 MiB per-OSC limit. A 1024x1024 RGBA image with
    // pseudo-random pixel content compresses poorly; the resulting
    // PNG is well over 1 MB.
    let w = 1024;
    let h = 1024;
    let mut buf = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let r = (x ^ y) as u8;
            let g = (x.wrapping_mul(31) ^ y.wrapping_mul(17)) as u8;
            let b = (x.wrapping_add(y.wrapping_mul(7))) as u8;
            buf.put_pixel(x, y, Rgba([r, g, b, 255]));
        }
    }
    let img = Image::from_dynamic(image::DynamicImage::ImageRgba8(buf));

    let mut screen: Screen<Vec<u8>> = Screen::new(Vec::new()).with_size(200, 60);
    let caps = caps_with_iterm2(Some((10, 20)));
    let mut layer = ImageLayer::new().with_protocol(ImageProtocol::Iterm2);

    let id = layer.add(img);
    layer.place(
        id,
        Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        },
        Resize::Scale(FilterType::Nearest),
    );

    layer.render(&caps, &mut screen).expect("render");
    screen.flush().unwrap();
    let bytes = screen.writer().clone();
    let s = String::from_utf8_lossy(&bytes);

    assert!(
        s.contains("\x1b]1337;MultipartFile="),
        "expected MultipartFile header in multipart burst"
    );
    assert!(
        s.contains("\x1b]1337;FilePart="),
        "expected at least one FilePart chunk"
    );
    assert!(
        s.contains("\x1b]1337;FileEnd\x07"),
        "expected FileEnd terminator"
    );
    // Single-shot `File=` must not appear when multipart is used.
    assert!(
        !s.contains("\x1b]1337;File="),
        "single-shot File= must not appear in multipart burst"
    );

    // Every OSC 1337 sub-sequence must respect the documented
    // 1 MiB per-control-sequence limit.
    let mut idx = 0;
    while let Some(start) = s[idx..].find("\x1b]1337;") {
        let abs = idx + start;
        let end = s[abs..]
            .find('\x07')
            .expect("OSC 1337 sequence is BEL-terminated");
        let total = end + 1;
        assert!(
            total <= 1_048_576,
            "OSC sub-sequence exceeds 1 MiB limit: {total} bytes"
        );
        idx = abs + total;
    }
}
