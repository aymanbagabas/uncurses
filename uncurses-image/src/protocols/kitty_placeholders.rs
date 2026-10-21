//! Kitty Unicode placeholders represented by ordinary cells.

use std::io::{self, Write};
use std::sync::Arc;

use uncurses::buffer::{Bounded, Buffer, Surface, SurfaceMut};
use uncurses::cell::Cell;
use uncurses::color::{Color, Profile};
use uncurses::layout::{Position, Rect, Size};
use uncurses::screen::{RegionResource, Screen};
use uncurses::style::Style;

use super::kitty;
use crate::{Image, ImageOptions, invalid};

pub(crate) const MAX_DIMENSION: usize = DIACRITICS.len();

/// A Kitty virtual placement and its ordinary placeholder cells.
///
/// Keep this value across frames. Cells contain complete row, column, and
/// image-ID information, so clipping and text edits work independently.
/// Each dimension can contain at most 297 cells, as specified by Kitty.
/// The output profile must preserve true color and underline colors.
///
/// Use [`draw`](Self::draw) for a screen. For another surface, copy
/// [`cells`](Self::cells) through `Surface::draw` and retain [`resource`](Self::resource)
/// on the destination screen before its next render.
#[derive(Clone, Debug)]
pub struct Placeholders {
    cells: Arc<Buffer>,
    resource: Arc<RegionResource>,
    image_id: u32,
}

impl Image {
    /// Prepare a Kitty virtual placement and its ordinary cells.
    ///
    /// This method performs no terminal I/O. Call only when the application
    /// confirms support for Kitty Unicode placeholders.
    pub fn kitty_placeholders(
        &self,
        size: Size,
        options: &ImageOptions,
    ) -> io::Result<Placeholders> {
        if size.width == 0
            || size.height == 0
            || usize::from(size.width) > MAX_DIMENSION
            || usize::from(size.height) > MAX_DIMENSION
        {
            return Err(invalid(
                "placeholder dimensions must be between 1 and 297 cells",
            ));
        }
        let (image_id, resource) = kitty::unicode_resource(self, size, options)?;
        let mut cells = Buffer::new(size.width, size.height);
        let [high, red, green, blue] = image_id.to_be_bytes();
        let style = Style::default()
            .fg(Color::Rgb(red, green, blue))
            .underline_color(Color::Rgb(0, 0, 1));
        for y in 0..size.height {
            for x in 0..size.width {
                let text: String = [
                    '\u{10eeee}',
                    DIACRITICS[usize::from(y)],
                    DIACRITICS[usize::from(x)],
                    DIACRITICS[usize::from(high)],
                ]
                .into_iter()
                .collect();
                cells.set_cell(
                    Position::new(x, y),
                    &Cell::new(text, 1).style(style.clone()),
                );
            }
        }
        Ok(Placeholders {
            cells: Arc::new(cells),
            resource,
            image_id,
        })
    }
}

impl Placeholders {
    pub(crate) fn matches(&self, cell: &Cell) -> bool {
        let Some(reference) = self.cells.cell(Position::ORIGIN) else {
            return false;
        };
        cell.style.fg == reference.style.fg
            && cell.style.underline_color == reference.style.underline_color
            && cell.content().starts_with('\u{10eeee}')
            && cell.content().chars().nth(3) == reference.content().chars().nth(3)
    }

    /// Borrow the ordinary cells for text composition or clipped copies.
    pub fn cells(&self) -> &Buffer {
        &self.cells
    }

    /// Borrow the shared terminal resource for an explicit screen binding.
    pub fn resource(&self) -> &Arc<RegionResource> {
        &self.resource
    }

    /// The Kitty image identifier encoded in the cells.
    pub fn image_id(&self) -> u32 {
        self.image_id
    }

    /// The virtual placement identifier encoded in the underline color.
    pub fn placement_id(&self) -> u32 {
        1
    }

    /// Retain the resource and copy its cells into a screen.
    ///
    /// Like ordinary text, a copy changes only its destination cells.
    /// Clear previous cells explicitly when moving the image. Full screen
    /// clear/fill removes resource bindings; call this method again each frame.
    /// Removing the binding releases ownership, while cell edits remove pixels.
    pub fn draw<W: Write>(
        &self,
        screen: &mut Screen<W>,
        key: &str,
        position: Position,
    ) -> io::Result<()> {
        let rect = Rect::new(
            position.x,
            position.y,
            self.cells.width(),
            self.cells.height(),
        );
        if rect.intersection(screen.bounds()) != rect {
            return Err(invalid("placeholder rectangle must fit inside the screen"));
        }
        if screen.color_profile() != Profile::TrueColor {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Kitty placeholders require true color",
            ));
        }
        screen.remove_region(key);
        screen.set_region_resource(key, Arc::clone(&self.resource));
        self.cells.draw(screen, position);
        Ok(())
    }
}

// Ordered ranges from Kitty's rowcolumn-diacritics.txt (Unicode 6.0 data).
const DIACRITICS: [char; 297] = {
    let ranges = [
        (0x305, 0x305),
        (0x30d, 0x30e),
        (0x310, 0x310),
        (0x312, 0x312),
        (0x33d, 0x33f),
        (0x346, 0x346),
        (0x34a, 0x34c),
        (0x350, 0x352),
        (0x357, 0x357),
        (0x35b, 0x35b),
        (0x363, 0x36f),
        (0x483, 0x487),
        (0x592, 0x595),
        (0x597, 0x599),
        (0x59c, 0x5a1),
        (0x5a8, 0x5a9),
        (0x5ab, 0x5ac),
        (0x5af, 0x5af),
        (0x5c4, 0x5c4),
        (0x610, 0x617),
        (0x657, 0x65b),
        (0x65d, 0x65e),
        (0x6d6, 0x6dc),
        (0x6df, 0x6e2),
        (0x6e4, 0x6e4),
        (0x6e7, 0x6e8),
        (0x6eb, 0x6ec),
        (0x730, 0x730),
        (0x732, 0x733),
        (0x735, 0x736),
        (0x73a, 0x73a),
        (0x73d, 0x73d),
        (0x73f, 0x741),
        (0x743, 0x743),
        (0x745, 0x745),
        (0x747, 0x747),
        (0x749, 0x74a),
        (0x7eb, 0x7f1),
        (0x7f3, 0x7f3),
        (0x816, 0x819),
        (0x81b, 0x823),
        (0x825, 0x827),
        (0x829, 0x82d),
        (0x951, 0x951),
        (0x953, 0x954),
        (0xf82, 0xf83),
        (0xf86, 0xf87),
        (0x135d, 0x135f),
        (0x17dd, 0x17dd),
        (0x193a, 0x193a),
        (0x1a17, 0x1a17),
        (0x1a75, 0x1a7c),
        (0x1b6b, 0x1b6b),
        (0x1b6d, 0x1b73),
        (0x1cd0, 0x1cd2),
        (0x1cda, 0x1cdb),
        (0x1ce0, 0x1ce0),
        (0x1dc0, 0x1dc1),
        (0x1dc3, 0x1dc9),
        (0x1dcb, 0x1dcc),
        (0x1dd1, 0x1de6),
        (0x1dfe, 0x1dfe),
        (0x20d0, 0x20d1),
        (0x20d4, 0x20d7),
        (0x20db, 0x20dc),
        (0x20e1, 0x20e1),
        (0x20e7, 0x20e7),
        (0x20e9, 0x20e9),
        (0x20f0, 0x20f0),
        (0x2cef, 0x2cf1),
        (0x2de0, 0x2dff),
        (0xa66f, 0xa66f),
        (0xa67c, 0xa67d),
        (0xa6f0, 0xa6f1),
        (0xa8e0, 0xa8f1),
        (0xaab0, 0xaab0),
        (0xaab2, 0xaab3),
        (0xaab7, 0xaab8),
        (0xaabe, 0xaabf),
        (0xaac1, 0xaac1),
        (0xfe20, 0xfe26),
        (0x10a0f, 0x10a0f),
        (0x10a38, 0x10a38),
        (0x1d185, 0x1d189),
        (0x1d1aa, 0x1d1ad),
        (0x1d242, 0x1d244),
    ];
    let mut result = ['\0'; 297];
    let mut i = 0;
    let mut r = 0;
    while r < ranges.len() {
        let mut codepoint = ranges[r].0;
        while codepoint <= ranges[r].1 {
            result[i] = char::from_u32(codepoint).unwrap();
            i += 1;
            codepoint += 1;
        }
        r += 1;
    }
    assert!(i == result.len());
    result
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_share_resources_and_preserve_identifier_colors() {
        let image = Image::from_rgba(1, 1, vec![255; 4]).unwrap();
        let a = image
            .kitty_placeholders(Size::new(3, 2), &ImageOptions::default())
            .unwrap();
        let b = image
            .kitty_placeholders(Size::new(3, 2), &ImageOptions::default())
            .unwrap();
        assert!(Arc::ptr_eq(a.resource(), b.resource()));
        let options = ImageOptions {
            dither: Some(std::sync::Arc::new(crate::dither::Noise::default())),
            ..Default::default()
        };
        let c = image.kitty_placeholders(Size::new(3, 2), &options).unwrap();
        assert!(Arc::ptr_eq(a.resource(), c.resource()));
        let cell = a.cells().cell(Position::new(2, 1)).unwrap();
        let marks: Vec<_> = cell.content().chars().collect();
        assert_eq!(&marks[..3], &['\u{10eeee}', '\u{30d}', '\u{30e}']);
        assert_eq!(DIACRITICS[296], '\u{1d244}');
        let mut screen = Screen::new(Vec::new(), (4, 3));
        screen.set_fullscreen(true);
        a.draw(&mut screen, "image", Position::ORIGIN).unwrap();
        screen.render().unwrap();
        screen.writer_mut().clear();
        screen.set_color_profile(Profile::Ansi256);
        assert_eq!(
            screen.render().unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        assert!(screen.writer().is_empty());
        screen.set_color_profile(Profile::TrueColor);
        screen.render().unwrap();
        screen.writer_mut().clear();
        a.draw(&mut screen, "image", Position::ORIGIN).unwrap();
        screen.render().unwrap();
        assert!(screen.writer().is_empty());
    }
}
