//! Kitty graphics protocol — Unicode placeholder mode.
//!
//! Each image is transmitted once with `U=1` (virtual placement) and
//! a 32-bit kitty-side id. To render the image, the backend stamps
//! cells whose content is the placeholder code-point `U+10EEEE`
//! followed by row / column / id-extra combining diacritics, with
//! the cell's foreground color encoding the low three bytes of the
//! kitty id. The terminal binds those cells to the virtual placement
//! and draws the image where the placeholders appear.
//!
//! ## Cache identity
//!
//! [`Self::paint`] hashes the source pixel data to recognize "same
//! image as last paint" — the host does not supply an identity.
//! The painter maps the pixel hash to a 32-bit kitty-side id and
//! remembers the cell footprint last transmitted. Re-painting the
//! same image with the same footprint reuses the previous
//! transmission. A different cell footprint forces a re-transmit
//! (the registered placement scales to the c=/r= grid sent on
//! transmit, so changing the cell rect without re-sending leaves
//! the old image dimensions anchored).
//!
//! When the host stops drawing an image, it calls [`Kitty::forget`]
//! (passing the same `&DynamicImage`) to schedule a terminal-side
//! delete; pending deletes are flushed by [`Kitty::flush_deletes`]
//! (or [`Kitty::shutdown`] at teardown).
//!
//! References:
//! * <https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders>

use std::io::{self, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::DynamicImage;
use rustc_hash::FxHashMap;
use uncurses::Rect;
use uncurses::cell::Cell;
use uncurses::color::Color;
use uncurses::screen::Screen;
use uncurses::style::Style;

use crate::hash::pixel_hash;
use crate::resize::Resize;

/// Unicode placeholder code-point.
const PLACEHOLDER: char = '\u{10EEEE}';

/// Per-chunk encoded payload size, in base64 characters. The kitty
/// protocol recommends a maximum of 4096 base64 characters per
/// chunk.
const CHARS_PER_CHUNK: usize = 4096;
/// Equivalent raw-byte chunk size: each 3 bytes encode to 4 chars.
const CHUNK_SIZE: usize = (CHARS_PER_CHUNK / 4) * 3;

/// Kitty-graphics painter, Unicode placeholder mode.
///
/// Caches a per-image transmission record so repeated [`Self::paint`]
/// calls with the same pixel data and cell footprint reuse the
/// existing virtual placement and only re-stamp the placeholder
/// cells.
#[derive(Debug, Default)]
pub struct Kitty {
    /// Per-pixel-hash state for images currently registered with
    /// the terminal.
    images: FxHashMap<u64, Entry>,
    /// Kitty-side ids the terminal still has registered for images
    /// the host has called [`Kitty::forget`] on. Drained by
    /// [`Kitty::flush_deletes`] / [`Kitty::shutdown`].
    pending_deletes: Vec<u32>,
    /// Counter for next kitty-side id; id 0 is reserved.
    next_kitty_id: u32,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    /// Full 32-bit kitty id used in transmission and in placeholder
    /// cells. The high byte is the "extra" diacritic; low three
    /// bytes are encoded as the foreground RGB color of the cell.
    kitty_id: u32,
    /// Cell-rect dimensions at the time of the last transmit. The
    /// terminal scales the registered image to fill the placeholder
    /// cell rect, so changing the cell rect (e.g. on a window
    /// resize) requires re-transmitting.
    cell_rect: (u16, u16),
}

impl Kitty {
    /// Construct a fresh painter with no pending state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stamp `image` into `area` of `screen`, transmitting it first
    /// if this is the first paint of these pixels or if the cell
    /// footprint has changed since the last paint.
    ///
    /// Returns I/O errors from writing the transmission to `screen`.
    /// Cell stamping is infallible.
    pub fn paint<W: Write>(
        &mut self,
        screen: &mut Screen<W>,
        area: Rect,
        image: &DynamicImage,
        resize: Resize,
    ) -> io::Result<()> {
        let area = clip_area(area, screen);
        if area.width == 0 || area.height == 0 {
            return Ok(());
        }

        let cell_px = screen.cell_pixel_size();
        let (cell_rect, payload) = prepare_payload(image, resize, area, cell_px);
        let hash = pixel_hash(image);

        let needs_transmit = match self.images.get(&hash) {
            Some(existing) => existing.cell_rect != (cell_rect.width, cell_rect.height),
            None => true,
        };

        let kitty_id = match self.images.get(&hash) {
            Some(existing) => existing.kitty_id,
            None => self.assign_kitty_id(),
        };

        if needs_transmit {
            transmit(screen, kitty_id, &payload, cell_rect)?;
            self.images.insert(
                hash,
                Entry {
                    kitty_id,
                    cell_rect: (cell_rect.width, cell_rect.height),
                },
            );
        }

        stamp_placeholders(screen, cell_rect, kitty_id);
        Ok(())
    }

    /// Forget the host-side mapping for `image` and schedule a
    /// terminal-side delete on the next [`Self::flush_deletes`].
    pub fn forget(&mut self, image: &DynamicImage) {
        let hash = pixel_hash(image);
        if let Some(entry) = self.images.remove(&hash) {
            self.pending_deletes.push(entry.kitty_id);
        }
    }

    /// Emit any deferred image-delete commands. Call this at the
    /// end of a frame, after the renderer has flushed.
    pub fn flush_deletes<W: Write>(&mut self, screen: &mut Screen<W>) -> io::Result<()> {
        for kitty_id in self.pending_deletes.drain(..) {
            // a=d,d=I deletes the image and all of its placements
            // from the terminal-side registry. q=2 quiets the
            // protocol response.
            let id_opt = format!("i={kitty_id}");
            uncurses::ansi::graphics::write_kitty_graphics(
                screen,
                &["a=d", "d=I", id_opt.as_str(), "q=2"],
                &[],
            )?;
        }
        Ok(())
    }

    /// Forget every registered host id and emit deletes for all of
    /// them. Call at host teardown.
    pub fn shutdown<W: Write>(&mut self, screen: &mut Screen<W>) -> io::Result<()> {
        for entry in self.images.values() {
            self.pending_deletes.push(entry.kitty_id);
        }
        self.images.clear();
        self.flush_deletes(screen)
    }

    fn assign_kitty_id(&mut self) -> u32 {
        // Kitty image id 0 is reserved by the protocol; skip it.
        loop {
            self.next_kitty_id = self.next_kitty_id.wrapping_add(1);
            if self.next_kitty_id != 0 {
                return self.next_kitty_id;
            }
        }
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

/// Image bytes to ship for one transmission.
struct Payload {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

/// Decide which sub-rectangle of `area` the placeholder cells should
/// live in, and produce the pixel payload to transmit.
///
/// * [`Resize::Scale`] — use `area`; transmit the source pixels and
///   let the terminal stretch them to the cell grid.
/// * [`Resize::Fit`] — compute an aspect-preserving sub-rectangle
///   and transmit the source pixels. If `cell_px` is unknown, fall
///   back to the full area.
/// * [`Resize::Crop`] — use `area`; pre-resize-cover the source to
///   the cell-grid pixel dimensions so the terminal-side stretch is
///   a 1:1 copy. If `cell_px` is unknown, fall back to source pixels.
fn prepare_payload(
    image: &DynamicImage,
    resize: Resize,
    area: Rect,
    cell_px: Option<(u16, u16)>,
) -> (Rect, Payload) {
    let (sw, sh) = (image.width(), image.height());

    match resize {
        Resize::Scale(_) => {
            let payload = Payload {
                rgba: image.to_rgba8().into_raw(),
                width: sw,
                height: sh,
            };
            (area, payload)
        }
        Resize::Fit(_) => {
            let cell_rect = match cell_px {
                Some((cw, ch)) => fit_cell_rect(area, (sw, sh), (cw, ch)),
                None => area,
            };
            let payload = Payload {
                rgba: image.to_rgba8().into_raw(),
                width: sw,
                height: sh,
            };
            (cell_rect, payload)
        }
        Resize::Crop(_) => match cell_px {
            Some((cw, ch)) => {
                let target_w = (area.width as u32) * (cw.max(1) as u32);
                let target_h = (area.height as u32) * (ch.max(1) as u32);
                let resized = image
                    .resize_to_fill(
                        target_w.max(1),
                        target_h.max(1),
                        image::imageops::FilterType::Triangle,
                    )
                    .to_rgba8();
                let payload = Payload {
                    width: resized.width(),
                    height: resized.height(),
                    rgba: resized.into_raw(),
                };
                (area, payload)
            }
            None => {
                let payload = Payload {
                    rgba: image.to_rgba8().into_raw(),
                    width: sw,
                    height: sh,
                };
                (area, payload)
            }
        },
    }
}

/// Compute the largest sub-rectangle of `area` whose pixel ratio
/// matches the source aspect ratio, centered.
fn fit_cell_rect(area: Rect, src: (u32, u32), cell_px: (u16, u16)) -> Rect {
    let cw = cell_px.0.max(1) as u32;
    let ch = cell_px.1.max(1) as u32;
    let area_px_w = (area.width as u32) * cw;
    let area_px_h = (area.height as u32) * ch;
    if area_px_w == 0 || area_px_h == 0 || src.0 == 0 || src.1 == 0 {
        return area;
    }
    let sx = area_px_w as f64 / src.0 as f64;
    let sy = area_px_h as f64 / src.1 as f64;
    let s = sx.min(sy);
    let target_px_w = ((src.0 as f64) * s).round().max(1.0) as u32;
    let target_px_h = ((src.1 as f64) * s).round().max(1.0) as u32;
    let cells_w = (target_px_w / cw).max(1).min(area.width as u32) as u16;
    let cells_h = (target_px_h / ch).max(1).min(area.height as u32) as u16;
    let dx = area.width.saturating_sub(cells_w) / 2;
    let dy = area.height.saturating_sub(cells_h) / 2;
    Rect {
        x: area.x + dx,
        y: area.y + dy,
        width: cells_w,
        height: cells_h,
    }
}

/// Emit the chunked transmit + virtual-placement APC sequence.
///
/// `cell_rect` is the placeholder grid the image should fill, in
/// cells; it is sent as `c=` / `r=` on the first chunk so the
/// terminal scales the registered image to the grid dimensions
/// instead of using its natural pixel-derived cell footprint.
fn transmit<W: Write>(
    screen: &mut Screen<W>,
    kitty_id: u32,
    payload: &Payload,
    cell_rect: Rect,
) -> io::Result<()> {
    let chunks: Vec<&[u8]> = payload.rgba.chunks(CHUNK_SIZE).collect();
    let chunk_count = chunks.len().max(1);

    let header_first = format!(
        "i={id},a=T,U=1,f=32,t=d,s={w},v={h},c={c},r={r}",
        id = kitty_id,
        w = payload.width,
        h = payload.height,
        c = cell_rect.width,
        r = cell_rect.height,
    );

    let mut encoded = String::new();
    for (i, chunk) in chunks.iter().enumerate() {
        encoded.clear();
        STANDARD.encode_string(*chunk, &mut encoded);
        let more = if i + 1 < chunk_count { 1 } else { 0 };
        let m_opt = format!("m={more}");

        // `q=2` quiets all responses. The header keys (a, U, f, t,
        // s, v, c, r) are sent only on the first chunk; subsequent
        // chunks repeat only `q` and `m`.
        let opts: Vec<&str> = if i == 0 {
            vec!["q=2", header_first.as_str(), m_opt.as_str()]
        } else {
            vec!["q=2", m_opt.as_str()]
        };
        uncurses::ansi::graphics::write_kitty_graphics(screen, &opts, encoded.as_bytes())?;
    }
    Ok(())
}

/// Stamp placeholder cells filling `cell_rect` with the placement
/// matching `kitty_id`.
fn stamp_placeholders<W: Write>(screen: &mut Screen<W>, cell_rect: Rect, kitty_id: u32) {
    let [id_extra, r, g, b] = kitty_id.to_be_bytes();
    let style = Style::EMPTY.with_fg(Color::Rgb(r, g, b));

    for cy in 0..cell_rect.height {
        for cx in 0..cell_rect.width {
            let content = placeholder_grapheme(cy, cx, id_extra);
            let cell = Cell::narrow(content).with_style(style.clone());
            screen.set_cell((cell_rect.x + cx, cell_rect.y + cy), &cell);
        }
    }
}

/// Build the placeholder + diacritics grapheme cluster for cell
/// `(col, row)` of the image with `id_extra` set as the high byte
/// of the kitty id.
fn placeholder_grapheme(row: u16, col: u16, id_extra: u8) -> String {
    let row_d = diacritic(row.min(MAX_DIACRITIC));
    let col_d = diacritic(col.min(MAX_DIACRITIC));
    let id_d = diacritic(u16::from(id_extra).min(MAX_DIACRITIC));
    let mut s = String::with_capacity(4 + 3 * 3);
    s.push(PLACEHOLDER);
    s.push(row_d);
    s.push(col_d);
    s.push(id_d);
    s
}

const MAX_DIACRITIC: u16 = 296;

fn diacritic(n: u16) -> char {
    DIACRITICS[n as usize]
}

/// Combining marks accepted as row / column / id-extra indices in
/// the Unicode placeholder mode. Source:
/// <https://sw.kovidgoyal.net/kitty/_downloads/1792bad15b12979994cd6ecc54c967a6/rowcolumn-diacritics.txt>
static DIACRITICS: [char; 297] = [
    '\u{0305}',
    '\u{030D}',
    '\u{030E}',
    '\u{0310}',
    '\u{0312}',
    '\u{033D}',
    '\u{033E}',
    '\u{033F}',
    '\u{0346}',
    '\u{034A}',
    '\u{034B}',
    '\u{034C}',
    '\u{0350}',
    '\u{0351}',
    '\u{0352}',
    '\u{0357}',
    '\u{035B}',
    '\u{0363}',
    '\u{0364}',
    '\u{0365}',
    '\u{0366}',
    '\u{0367}',
    '\u{0368}',
    '\u{0369}',
    '\u{036A}',
    '\u{036B}',
    '\u{036C}',
    '\u{036D}',
    '\u{036E}',
    '\u{036F}',
    '\u{0483}',
    '\u{0484}',
    '\u{0485}',
    '\u{0486}',
    '\u{0487}',
    '\u{0592}',
    '\u{0593}',
    '\u{0594}',
    '\u{0595}',
    '\u{0597}',
    '\u{0598}',
    '\u{0599}',
    '\u{059C}',
    '\u{059D}',
    '\u{059E}',
    '\u{059F}',
    '\u{05A0}',
    '\u{05A1}',
    '\u{05A8}',
    '\u{05A9}',
    '\u{05AB}',
    '\u{05AC}',
    '\u{05AF}',
    '\u{05C4}',
    '\u{0610}',
    '\u{0611}',
    '\u{0612}',
    '\u{0613}',
    '\u{0614}',
    '\u{0615}',
    '\u{0616}',
    '\u{0617}',
    '\u{0657}',
    '\u{0658}',
    '\u{0659}',
    '\u{065A}',
    '\u{065B}',
    '\u{065D}',
    '\u{065E}',
    '\u{06D6}',
    '\u{06D7}',
    '\u{06D8}',
    '\u{06D9}',
    '\u{06DA}',
    '\u{06DB}',
    '\u{06DC}',
    '\u{06DF}',
    '\u{06E0}',
    '\u{06E1}',
    '\u{06E2}',
    '\u{06E4}',
    '\u{06E7}',
    '\u{06E8}',
    '\u{06EB}',
    '\u{06EC}',
    '\u{0730}',
    '\u{0732}',
    '\u{0733}',
    '\u{0735}',
    '\u{0736}',
    '\u{073A}',
    '\u{073D}',
    '\u{073F}',
    '\u{0740}',
    '\u{0741}',
    '\u{0743}',
    '\u{0745}',
    '\u{0747}',
    '\u{0749}',
    '\u{074A}',
    '\u{07EB}',
    '\u{07EC}',
    '\u{07ED}',
    '\u{07EE}',
    '\u{07EF}',
    '\u{07F0}',
    '\u{07F1}',
    '\u{07F3}',
    '\u{0816}',
    '\u{0817}',
    '\u{0818}',
    '\u{0819}',
    '\u{081B}',
    '\u{081C}',
    '\u{081D}',
    '\u{081E}',
    '\u{081F}',
    '\u{0820}',
    '\u{0821}',
    '\u{0822}',
    '\u{0823}',
    '\u{0825}',
    '\u{0826}',
    '\u{0827}',
    '\u{0829}',
    '\u{082A}',
    '\u{082B}',
    '\u{082C}',
    '\u{082D}',
    '\u{0951}',
    '\u{0953}',
    '\u{0954}',
    '\u{0F82}',
    '\u{0F83}',
    '\u{0F86}',
    '\u{0F87}',
    '\u{135D}',
    '\u{135E}',
    '\u{135F}',
    '\u{17DD}',
    '\u{193A}',
    '\u{1A17}',
    '\u{1A75}',
    '\u{1A76}',
    '\u{1A77}',
    '\u{1A78}',
    '\u{1A79}',
    '\u{1A7A}',
    '\u{1A7B}',
    '\u{1A7C}',
    '\u{1B6B}',
    '\u{1B6D}',
    '\u{1B6E}',
    '\u{1B6F}',
    '\u{1B70}',
    '\u{1B71}',
    '\u{1B72}',
    '\u{1B73}',
    '\u{1CD0}',
    '\u{1CD1}',
    '\u{1CD2}',
    '\u{1CDA}',
    '\u{1CDB}',
    '\u{1CE0}',
    '\u{1DC0}',
    '\u{1DC1}',
    '\u{1DC3}',
    '\u{1DC4}',
    '\u{1DC5}',
    '\u{1DC6}',
    '\u{1DC7}',
    '\u{1DC8}',
    '\u{1DC9}',
    '\u{1DCB}',
    '\u{1DCC}',
    '\u{1DD1}',
    '\u{1DD2}',
    '\u{1DD3}',
    '\u{1DD4}',
    '\u{1DD5}',
    '\u{1DD6}',
    '\u{1DD7}',
    '\u{1DD8}',
    '\u{1DD9}',
    '\u{1DDA}',
    '\u{1DDB}',
    '\u{1DDC}',
    '\u{1DDD}',
    '\u{1DDE}',
    '\u{1DDF}',
    '\u{1DE0}',
    '\u{1DE1}',
    '\u{1DE2}',
    '\u{1DE3}',
    '\u{1DE4}',
    '\u{1DE5}',
    '\u{1DE6}',
    '\u{1DFE}',
    '\u{20D0}',
    '\u{20D1}',
    '\u{20D4}',
    '\u{20D5}',
    '\u{20D6}',
    '\u{20D7}',
    '\u{20DB}',
    '\u{20DC}',
    '\u{20E1}',
    '\u{20E7}',
    '\u{20E9}',
    '\u{20F0}',
    '\u{2CEF}',
    '\u{2CF0}',
    '\u{2CF1}',
    '\u{2DE0}',
    '\u{2DE1}',
    '\u{2DE2}',
    '\u{2DE3}',
    '\u{2DE4}',
    '\u{2DE5}',
    '\u{2DE6}',
    '\u{2DE7}',
    '\u{2DE8}',
    '\u{2DE9}',
    '\u{2DEA}',
    '\u{2DEB}',
    '\u{2DEC}',
    '\u{2DED}',
    '\u{2DEE}',
    '\u{2DEF}',
    '\u{2DF0}',
    '\u{2DF1}',
    '\u{2DF2}',
    '\u{2DF3}',
    '\u{2DF4}',
    '\u{2DF5}',
    '\u{2DF6}',
    '\u{2DF7}',
    '\u{2DF8}',
    '\u{2DF9}',
    '\u{2DFA}',
    '\u{2DFB}',
    '\u{2DFC}',
    '\u{2DFD}',
    '\u{2DFE}',
    '\u{2DFF}',
    '\u{A66F}',
    '\u{A67C}',
    '\u{A67D}',
    '\u{A6F0}',
    '\u{A6F1}',
    '\u{A8E0}',
    '\u{A8E1}',
    '\u{A8E2}',
    '\u{A8E3}',
    '\u{A8E4}',
    '\u{A8E5}',
    '\u{A8E6}',
    '\u{A8E7}',
    '\u{A8E8}',
    '\u{A8E9}',
    '\u{A8EA}',
    '\u{A8EB}',
    '\u{A8EC}',
    '\u{A8ED}',
    '\u{A8EE}',
    '\u{A8EF}',
    '\u{A8F0}',
    '\u{A8F1}',
    '\u{AAB0}',
    '\u{AAB2}',
    '\u{AAB3}',
    '\u{AAB7}',
    '\u{AAB8}',
    '\u{AABE}',
    '\u{AABF}',
    '\u{AAC1}',
    '\u{FE20}',
    '\u{FE21}',
    '\u{FE22}',
    '\u{FE23}',
    '\u{FE24}',
    '\u{FE25}',
    '\u{FE26}',
    '\u{10A0F}',
    '\u{10A38}',
    '\u{1D185}',
    '\u{1D186}',
    '\u{1D187}',
    '\u{1D188}',
    '\u{1D189}',
    '\u{1D1AA}',
    '\u{1D1AB}',
    '\u{1D1AC}',
    '\u{1D1AD}',
    '\u{1D242}',
    '\u{1D243}',
    '\u{1D244}',
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diacritic_table_size_matches_protocol() {
        assert_eq!(DIACRITICS.len(), 297);
    }

    #[test]
    fn placeholder_grapheme_layout() {
        let s = placeholder_grapheme(0, 0, 0);
        let mut chars = s.chars();
        assert_eq!(chars.next(), Some(PLACEHOLDER));
        assert_eq!(chars.next(), Some(DIACRITICS[0]));
        assert_eq!(chars.next(), Some(DIACRITICS[0]));
        assert_eq!(chars.next(), Some(DIACRITICS[0]));
        assert!(chars.next().is_none());
    }

    #[test]
    fn placeholder_grapheme_clamps_oversize_indices() {
        let s = placeholder_grapheme(u16::MAX, u16::MAX, u8::MAX);
        let chars: Vec<_> = s.chars().collect();
        assert_eq!(chars[0], PLACEHOLDER);
        assert_eq!(chars[1], DIACRITICS[MAX_DIACRITIC as usize]);
        assert_eq!(chars[2], DIACRITICS[MAX_DIACRITIC as usize]);
        assert_eq!(chars[3], DIACRITICS[u8::MAX as usize]);
    }
}
