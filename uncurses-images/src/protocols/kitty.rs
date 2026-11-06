//! Kitty uploads with direct, chunked RGB data.

use std::{
    collections::hash_map::RandomState,
    hash::BuildHasher,
    io,
    sync::{
        LazyLock,
        atomic::{AtomicU32, Ordering},
    },
};

use base64::{Engine, engine::general_purpose::STANDARD};
use image::RgbImage;
use uncurses::ansi::graphics;

static NEXT_ID: LazyLock<AtomicU32> = LazyLock::new(|| {
    let seed = RandomState::new().hash_one(std::process::id()) as u32;
    AtomicU32::new((seed & 0x7fff_ffff) + 1)
});

pub(crate) fn next_id() -> io::Result<u32> {
    allocate_id(&NEXT_ID)
}

fn allocate_id(counter: &AtomicU32) -> io::Result<u32> {
    let mut id = counter.load(Ordering::Relaxed);
    loop {
        let next = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Kitty graphics identifiers exhausted"))?;
        match counter.compare_exchange_weak(id, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(id),
            Err(current) => id = current,
        }
    }
}

pub(crate) fn encode(pixels: &RgbImage, id: u32) -> io::Result<Vec<u8>> {
    let encoded = STANDARD.encode(pixels.as_raw());
    let chunks = encoded.as_bytes().chunks(4096);
    let count = chunks.len();
    let mut bytes = Vec::new();
    for (index, chunk) in chunks.enumerate() {
        let more = usize::from(index + 1 < count);
        let header = if index == 0 {
            format!(
                "a=t,t=d,f=24,s={},v={},i={id},q=2,m={more}",
                pixels.width(),
                pixels.height()
            )
        } else {
            format!("m={more},q=2")
        };
        graphics::write_kitty_graphics(&mut bytes, &[&header], chunk)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_upload_exact_pixels_without_display() {
        let pixels = RgbImage::from_fn(73, 53, |x, y| image::Rgb([x as u8, y as u8, 42]));
        let encoded = encode(&pixels, 123).unwrap();
        let text = std::str::from_utf8(&encoded).unwrap();
        let chunks: Vec<_> = text.split("\x1b\\").filter(|s| !s.is_empty()).collect();
        assert!(chunks.len() > 1);
        let mut payload = String::new();
        for (i, chunk) in chunks.iter().enumerate() {
            let (header, data) = chunk.split_once(';').unwrap();
            assert!(data.len() <= 4096);
            assert_eq!(data.len() % 4, 0);
            assert!(header.contains(if i + 1 == chunks.len() { "m=0" } else { "m=1" }));
            if i == 0 {
                assert_eq!(header, "\x1b_Ga=t,t=d,f=24,s=73,v=53,i=123,q=2,m=1");
            }
            payload.push_str(data);
        }
        assert_eq!(STANDARD.decode(payload).unwrap(), *pixels.as_raw());
        assert!(!text.contains("a=p"));
        assert!(!text.contains("a=T"));
    }

    #[test]
    fn exhausted_ids_do_not_wrap() {
        let counter = AtomicU32::new(u32::MAX - 1);
        assert_eq!(allocate_id(&counter).unwrap(), u32::MAX - 1);
        assert!(allocate_id(&counter).is_err());
        assert_eq!(counter.load(Ordering::Relaxed), u32::MAX);
    }
}
