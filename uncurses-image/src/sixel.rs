//! Sixel backend.
//!
//! Encodes an image to a DCS sixel sequence and stamps it as a
//! single rect-anchored cell. The renderer emits the anchor's bytes
//! verbatim and skips every body cell, so the painted region never
//! interferes with surrounding text the differ owns.
//!
//! ## Host id contract
//!
//! Each [`Self::paint`] call is keyed by a `u64` host id plus the
//! cell rectangle dimensions. While the id and footprint are
//! unchanged the encoded sequence is reused from cache; changing
//! either re-encodes. The host must use a fresh id (or call
//! [`Self::forget`]) when the source pixels change.
//!
//! ## Per-cell pixel size
//!
//! Sixel images are sized in pixels. The painter consults
//! [`uncurses::screen::Screen::cell_pixel_size`] to translate
//! `area` (in cells) into the target pixel rectangle. When the
//! cache is unset, [`Self::paint`] returns without writing — sixel
//! has no meaningful fallback in cell space.

use std::io::{self, Write};

use icy_sixel::SixelImage;
use image::DynamicImage;
use rustc_hash::FxHashMap;
use uncurses::Rect;
use uncurses::ansi::graphics::write_sixel;
use uncurses::cell::Cell;
use uncurses::screen::Screen;
use uncurses::style::Style;

use crate::resize::Resize;

/// Sixel painter.
///
/// Caches the encoded sixel sequence per `(host_id, cell_rect)` so
/// repeated paints with the same id and footprint reuse the
/// previously encoded bytes. Stateless beyond the cache.
#[derive(Debug, Default)]
pub struct Sixel {
    cache: FxHashMap<(u64, (u16, u16)), String>,
}

impl Sixel {
    /// Construct a fresh painter with an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stamp `image` into `area` of `screen`, encoding the image as
    /// a sixel DCS sequence and storing it as a single rect-anchored
    /// cell at `(area.x, area.y)`.
    ///
    /// Returns I/O errors from sequence assembly. When the screen's
    /// cell pixel size is unknown, this is a no-op (returns `Ok`).
    pub fn paint<W: Write>(
        &mut self,
        screen: &mut Screen<W>,
        area: Rect,
        image: &DynamicImage,
        resize: Resize,
        host_id: u64,
    ) -> io::Result<()> {
        let area = clip_area(area, screen);
        if area.width == 0 || area.height == 0 {
            return Ok(());
        }
        let Some((cw, ch)) = screen.cell_pixel_size() else {
            return Ok(());
        };

        let key = (host_id, (area.width, area.height));
        let sequence = match self.cache.get(&key) {
            Some(s) => s.clone(),
            None => {
                let encoded = encode(image, area, (cw, ch), resize)?;
                self.cache.entry(key).or_insert(encoded).clone()
            }
        };

        stamp(screen, area, sequence);
        Ok(())
    }

    /// Drop every cached entry for `host_id`. The next paint with
    /// that id re-encodes from source pixels.
    pub fn forget(&mut self, host_id: u64) {
        self.cache.retain(|(id, _), _| *id != host_id);
    }
}

fn clip_area<W: Write>(area: Rect, screen: &Screen<W>) -> Rect {
    let sw = screen.width();
    let sh = screen.height();
    let x = area.x.min(sw);
    let y = area.y.min(sh);
    let width = area.width.min(sw.saturating_sub(x));
    let height = area.height.min(sh.saturating_sub(y));
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// Resize `image` to fit the cell-pixel rectangle implied by `area`
/// + `cell_px`, encode it to sixel, then re-emit the inner payload
///   through [`write_sixel`] so the framing comes from the ansi
///   helpers.
fn encode(
    image: &DynamicImage,
    area: Rect,
    cell_px: (u16, u16),
    resize: Resize,
) -> io::Result<String> {
    let cw = cell_px.0.max(1) as u32;
    let ch = cell_px.1.max(1) as u32;
    let target_w = (area.width as u32) * cw;
    let target_h = (area.height as u32) * ch;
    if target_w == 0 || target_h == 0 {
        return Ok(String::new());
    }

    let resized = match resize {
        Resize::Scale(filter) => image.resize_exact(target_w, target_h, filter).to_rgba8(),
        Resize::Fit(filter) => {
            let fit = image.resize(target_w, target_h, filter).to_rgba8();
            let mut canvas = image::RgbaImage::new(target_w, target_h);
            let dx = (target_w.saturating_sub(fit.width())) / 2;
            let dy = (target_h.saturating_sub(fit.height())) / 2;
            image::imageops::overlay(&mut canvas, &fit, dx as i64, dy as i64);
            canvas
        }
        Resize::Crop(_) => image
            .resize_to_fill(target_w, target_h, image::imageops::FilterType::Triangle)
            .to_rgba8(),
    };

    let (w, h) = (resized.width() as usize, resized.height() as usize);
    let raw = resized.into_raw();
    let sixel = SixelImage::from_rgba(raw, w, h);
    let dcs = sixel
        .encode()
        .map_err(|e| io::Error::other(e.to_string()))?;

    let payload = strip_dcs_frame(&dcs);
    let mut out = Vec::with_capacity(payload.len() + 8);
    write_sixel(&mut out, -1, 1, 0, payload)?;
    String::from_utf8(out).map_err(|e| io::Error::other(e.to_string()))
}

/// Extract the inner sixel payload from a complete `\x1bP…q…\x1b\\`
/// sequence. Returns the input unchanged (as bytes) when framing
/// markers are missing — callers re-wrap unconditionally so a
/// missing frame would only produce a malformed output, not unsafe
/// behavior.
fn strip_dcs_frame(dcs: &str) -> &[u8] {
    let bytes = dcs.as_bytes();
    let start = match bytes.iter().position(|&b| b == b'q') {
        Some(p) => p + 1,
        None => return bytes,
    };
    let end = if bytes.ends_with(b"\x1b\\") {
        bytes.len() - 2
    } else {
        bytes.len()
    };
    if end < start {
        return bytes;
    }
    &bytes[start..end]
}

fn stamp<W: Write>(screen: &mut Screen<W>, area: Rect, sequence: String) {
    let anchor = Cell::rect_anchor(area, sequence).with_style(Style::EMPTY);
    screen.set_cell((area.x, area.y), &anchor);
    let body = Cell::rect_body(area);
    for cy in 0..area.height {
        for cx in 0..area.width {
            if cx == 0 && cy == 0 {
                continue;
            }
            screen.set_cell((area.x + cx, area.y + cy), &body);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_dcs_frame_removes_intro_and_terminator() {
        let dcs = "\x1bP0;1;0qPAYLOAD\x1b\\";
        assert_eq!(strip_dcs_frame(dcs), b"PAYLOAD");
    }

    #[test]
    fn strip_dcs_frame_handles_missing_terminator() {
        let dcs = "\x1bP0;1;0qPAYLOAD";
        assert_eq!(strip_dcs_frame(dcs), b"PAYLOAD");
    }

    #[test]
    fn strip_dcs_frame_falls_back_when_q_missing() {
        let dcs = "no markers here";
        assert_eq!(strip_dcs_frame(dcs), dcs.as_bytes());
    }
}
