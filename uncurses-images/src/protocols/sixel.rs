//! Sixel encoding of indexed pixels and clipped fragments.

use std::io::{self, Write};

use crate::quant::IndexedImage;
use uncurses::ansi::graphics;

pub(crate) fn encode(image: &IndexedImage, rect: (u32, u32, u32, u32)) -> io::Result<Vec<u8>> {
    let (x, y, width, height) = rect;
    debug_assert!(x + width <= image.width && y + height <= image.height);
    let mut payload = Vec::new();
    write!(payload, "\"1;1;{width};{height}")?;
    for (index, color) in image.palette.iter().enumerate() {
        write!(
            payload,
            "#{index};2;{};{};{}",
            (u32::from(color[0]) * 100 + 127) / 255,
            (u32::from(color[1]) * 100 + 127) / 255,
            (u32::from(color[2]) * 100 + 127) / 255
        )?;
    }

    encode_sixel_bands(image, rect, &mut payload)?;
    let mut bytes = Vec::new();
    graphics::write_sixel(&mut bytes, 7, 1, 0, &payload)?;
    Ok(bytes)
}

fn encode_sixel_bands(
    image: &IndexedImage,
    rect: (u32, u32, u32, u32),
    payload: &mut Vec<u8>,
) -> io::Result<()> {
    let (left, top, width, height) = (
        rect.0 as usize,
        rect.1 as usize,
        rect.2 as usize,
        rect.3 as usize,
    );
    let mut patterns = vec![0u8; width * image.palette.len()];
    for y in (0..height).step_by(6) {
        let mut starts = [usize::MAX; 256];
        let mut ends = [0; 256];
        for dy in 0..6.min(height - y) {
            let start = (top + y + dy) * image.width as usize + left;
            for (x, &color) in image.indices[start..start + width].iter().enumerate() {
                let color = usize::from(color);
                if starts[color] == usize::MAX {
                    patterns[color * width..(color + 1) * width].fill(0);
                }
                patterns[color * width + x] |= 1 << dy;
                starts[color] = starts[color].min(x);
                ends[color] = ends[color].max(x + 1);
            }
        }
        let mut first = true;
        for (color, start) in starts.into_iter().enumerate() {
            if start == usize::MAX {
                continue;
            }
            if !first {
                payload.push(b'$');
            }
            first = false;
            write!(payload, "#{color}")?;
            write_run(payload, b'?', start)?;
            let row = &patterns[color * width..(color + 1) * width];
            let mut x = start;
            while x < ends[color] {
                let bits = row[x];
                let next = row[x..ends[color]]
                    .iter()
                    .position(|&v| v != bits)
                    .map_or(ends[color], |n| x + n);
                write_run(payload, bits + b'?', next - x)?;
                x = next;
            }
        }
        if y + 6 < height {
            payload.push(b'-');
        }
    }
    Ok(())
}

fn write_run(payload: &mut Vec<u8>, byte: u8, count: usize) -> io::Result<()> {
    if count > 3 {
        write!(payload, "!{count}{}", char::from(byte))
    } else {
        payload.extend(std::iter::repeat_n(byte, count));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbImage;

    #[test]
    fn clipped_dithered_fragments_preserve_complete_image_indices() {
        let pixels = RgbImage::from_fn(37, 19, |x, y| {
            image::Rgb([(x * 7) as u8, (y * 13) as u8, ((x + y) * 4) as u8])
        });
        for algorithm in crate::dither::test_algorithms() {
            let options = crate::ImageOptions {
                dither: algorithm,
                ..Default::default()
            };
            let full = crate::quant::sixel(&pixels, &options).unwrap();
            for (x, y, width, height) in [(3, 2, 13, 11), (36, 18, 1, 1), (0, 0, 37, 19)] {
                let indices = (y..y + height)
                    .flat_map(|yy| {
                        let start = (yy * full.width + x) as usize;
                        full.indices[start..start + width as usize].iter().copied()
                    })
                    .collect();
                let crop = IndexedImage {
                    width,
                    height,
                    palette: full.palette.clone(),
                    indices,
                };
                assert_eq!(
                    encode(&full, (x, y, width, height)).unwrap(),
                    encode(&crop, (0, 0, width, height)).unwrap(),
                );
            }
        }
    }

    #[test]
    fn sixel_bands_repeats_and_palette_decode_independently() {
        for (width, height) in [(1, 1), (7, 5), (8, 6), (13, 14)] {
            let pixels = RgbImage::from_fn(width, height, |x, y| {
                image::Rgb(if x < width / 2 {
                    [255, 0, 0]
                } else if y < height / 2 {
                    [0, 255, 0]
                } else {
                    [0, 0, 255]
                })
            });
            let indexed = crate::quant::sixel(&pixels, &crate::ImageOptions::default()).unwrap();
            let bytes = encode(&indexed, (0, 0, width, height)).unwrap();
            let start = bytes.iter().position(|&b| b == b'q').unwrap() + 1;
            let mut data = &bytes[start..bytes.len() - 2];
            let mut palette = [[0u8; 3]; 256];
            let mut decoded = vec![None; (width * height) as usize];
            let (mut x, mut y, mut color) = (0, 0, 0);
            fn number(data: &mut &[u8]) -> usize {
                let mut n = 0;
                while data.first().is_some_and(u8::is_ascii_digit) {
                    n = n * 10 + usize::from(data[0] - b'0');
                    *data = &data[1..];
                }
                n
            }
            while let Some((&command, rest)) = data.split_first() {
                data = rest;
                match command {
                    b'"' => {
                        let mut values = vec![number(&mut data)];
                        while data.first() == Some(&b';') {
                            data = &data[1..];
                            values.push(number(&mut data));
                        }
                        assert_eq!(values, [1, 1, width as usize, height as usize]);
                    }
                    b'#' => {
                        color = number(&mut data);
                        if data.starts_with(b";2;") {
                            data = &data[3..];
                            for (i, component) in palette[color].iter_mut().enumerate() {
                                *component = ((number(&mut data) * 255 + 50) / 100) as u8;
                                if i < 2 {
                                    assert_eq!(data[0], b';');
                                    data = &data[1..];
                                }
                            }
                        }
                    }
                    b'$' => x = 0,
                    b'-' => {
                        x = 0;
                        y += 6;
                    }
                    b'!' | b'?'..=b'~' => {
                        let (repeat, bits) = if command == b'!' {
                            let repeat = number(&mut data);
                            let bits = data[0] - b'?';
                            data = &data[1..];
                            (repeat, bits)
                        } else {
                            (1, command - b'?')
                        };
                        for _ in 0..repeat {
                            for dy in 0..6 {
                                if bits & (1 << dy) != 0 {
                                    assert!(x < width as usize && y + dy < height as usize);
                                    decoded[(y + dy) * width as usize + x] = Some(palette[color]);
                                }
                            }
                            x += 1;
                        }
                    }
                    _ => panic!("unexpected Sixel byte {command}"),
                }
            }
            for (actual, expected) in decoded.into_iter().zip(pixels.pixels()) {
                let actual = actual.expect("unpainted pixel");
                assert!(
                    actual
                        .into_iter()
                        .zip(expected.0)
                        .all(|(a, b)| a.abs_diff(b) < 12),
                    "{actual:?} != {expected:?}"
                );
            }
        }
    }
}
