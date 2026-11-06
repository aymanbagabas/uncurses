//! Kitty Unicode placeholders as ordinary cells with explicit resources.

use std::io::{self, Write};
use std::sync::Arc;

use uncurses::buffer::{Buffer, SurfaceMut};
use uncurses::cell::Cell;
use uncurses::color::{Color, Profile};
use uncurses::layout::{Position, Size};
use uncurses::screen::{PlacementOptions, Screen};
use uncurses::style::Style;

use crate::{Image, ImageOptions, invalid, native::KittyResource};

use super::kitty;

use uncurses::ansi::graphics::KITTY_PLACEHOLDER_DIACRITICS as DIACRITICS;

/// Ordinary image cells with shared placement resources.
///
/// Pass [`placement`](Self::placement) to `Screen::set_placement`, then render.
/// The screen owns movement, removal, uploads, and resource deletion.
/// This value performs no terminal I/O.
///
/// Kitty placeholders require [`Profile::TrueColor`] and underline-color support.
/// For manual output, copy [`cells`](Self::cells), upload before rendering,
/// and delete after clearing and rendering the cells.
#[derive(Clone, Debug)]
pub struct CellImage {
    cells: Arc<Buffer>,
    resource: Arc<KittyResource>,
}

impl CellImage {
    /// Borrow the ordinary cells for composition or clipped copies.
    pub fn cells(&self) -> &Buffer {
        &self.cells
    }

    /// The shared image identifier.
    pub fn resource_id(&self) -> u32 {
        self.resource.id
    }

    /// Raw upload bytes for manual output, including the virtual placement.
    ///
    /// Screen placements already include these bytes.
    pub fn upload(&self) -> &[u8] {
        &self.resource.upload
    }

    /// Raw deletion bytes for manual output after its cells are cleared.
    ///
    /// Screen placements already manage resource deletion.
    pub fn delete(&self) -> &[u8] {
        &self.resource.delete
    }

    /// Prepare a cell placement under the destination screen's color profile.
    ///
    /// Requires true color to preserve encoded image identifiers.
    /// The options share cells and resources with this image.
    /// Preparation leaves the screen unchanged.
    pub fn placement<W: Write>(&self, screen: &Screen<W>) -> io::Result<PlacementOptions> {
        if screen.color_profile() != Profile::TrueColor {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Kitty placeholders require true color",
            ));
        }
        Ok(PlacementOptions {
            cells: Some(self.cells.clone()),
            upload: self.resource.upload.clone(),
            delete: self.resource.delete.clone(),
            ..Default::default()
        })
    }
}

impl Image {
    /// Prepare a Kitty virtual placement and ordinary placeholder cells.
    ///
    /// Each dimension must contain 1 through 297 cells. Every cell encodes
    /// the complete image identifier, row, and column for independent clipping.
    /// A zero high image-ID byte uses the protocol's implicit zero value.
    /// The resource uploads the pixels and creates a virtual placement.
    /// Resource release deletes only this image's identifier.
    ///
    /// ```
    /// use uncurses::{color::Profile, layout::Rect, screen::Screen};
    /// use uncurses_images::{Image, ImageOptions};
    ///
    /// let source = Image::from_rgb(1, 1, vec![255, 0, 0])?;
    /// let image = source.kitty_placeholders(
    ///     (2, 1).into(), (8, 16).into(), &ImageOptions::default(),
    /// )?;
    /// let mut screen = Screen::new(Vec::new(), (8, 4));
    /// screen.set_color_profile(Profile::TrueColor);
    /// screen.set_placement(1, Rect::new(1, 1, 2, 1), &image.placement(&screen)?)?;
    /// screen.render()?;
    /// let output = String::from_utf8_lossy(screen.writer());
    /// assert!(output.contains("U=1"));
    /// assert!(output.contains('\u{10eeee}'));
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn kitty_placeholders(
        &self,
        size: Size,
        cell_pixels: Size,
        options: &ImageOptions,
    ) -> io::Result<CellImage> {
        if size.width == 0
            || size.height == 0
            || usize::from(size.width) > DIACRITICS.len()
            || usize::from(size.height) > DIACRITICS.len()
        {
            return Err(invalid(
                "placeholder dimensions must be between 1 and 297 cells",
            ));
        }
        let pixels = self.rasterize(size, cell_pixels, options)?;
        let id = kitty::next_id()?;
        let mut upload = kitty::encode(&pixels, id)?;
        use std::io::Write;
        write!(
            upload,
            "\x1b_Ga=p,U=1,i={id},p={},c={},r={},C=1,q=2\x1b\\",
            placement_id(id),
            size.width,
            size.height,
        )?;
        Ok(CellImage {
            cells: Arc::new(placeholder_cells(size, id)),
            resource: Arc::new(KittyResource::new(id, upload)),
        })
    }
}

fn placement_id(id: u32) -> u8 {
    // Separate zero and nonzero high bytes so omitted zero cannot inherit a neighboring image's nonzero byte.
    1 + u8::from(id >> 24 == 0)
}

fn placeholder_cells(size: Size, id: u32) -> Buffer {
    let mut cells = Buffer::new(size.width, size.height);
    let [high, red, green, blue] = id.to_be_bytes();
    let style = Style::default()
        .fg(Color::Rgb(red, green, blue))
        .underline_color(Color::Rgb(0, 0, placement_id(id)));
    for y in 0..size.height {
        for x in 0..size.width {
            let mut text: String = [
                '\u{10eeee}',
                DIACRITICS[usize::from(y)],
                DIACRITICS[usize::from(x)],
            ]
            .into_iter()
            .collect();
            if high != 0 {
                text.push(DIACRITICS[usize::from(high)]);
            }
            cells.set_cell(
                Position::new(x, y),
                &Cell::new(text, 1).style(style.clone()),
            );
        }
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use uncurses::buffer::Surface;
    use uncurses::screen::Screen;

    fn image() -> Image {
        Image::from_rgb(1, 1, vec![255, 0, 0]).unwrap()
    }

    #[test]
    fn largest_image_id_and_coordinates_keep_all_bits() {
        let cells = placeholder_cells((297, 297).into(), u32::MAX);
        let cell = cells.cell((296, 296).into()).unwrap();
        assert_eq!(
            cell.content().chars().collect::<Vec<_>>(),
            [
                '\u{10eeee}',
                DIACRITICS[296],
                DIACRITICS[296],
                DIACRITICS[255]
            ],
        );
        assert_eq!(cell.style.fg, Some(Color::Rgb(255, 255, 255)));
        assert_eq!(cell.style.underline_color, Some(Color::Rgb(0, 0, 1)));
    }

    #[test]
    fn zero_high_bytes_are_omitted_without_cross_image_inheritance() {
        let low = placeholder_cells((2, 1).into(), 1);
        let high = placeholder_cells((2, 1).into(), 0x0100_0001);
        let low = low.cell((1, 0).into()).unwrap();
        let high = high.cell((0, 0).into()).unwrap();
        assert_eq!(
            low.content().chars().collect::<Vec<_>>(),
            ['\u{10eeee}', DIACRITICS[0], DIACRITICS[1]],
        );
        assert_eq!(low.style.fg, high.style.fg);
        assert_ne!(low.style.underline_color, high.style.underline_color);
        assert_eq!(
            high.content().chars().collect::<Vec<_>>(),
            ['\u{10eeee}', DIACRITICS[0], DIACRITICS[0], DIACRITICS[1]],
        );
    }

    #[test]
    fn placeholder_cells_encode_full_identifiers_and_coordinates() {
        let image = image()
            .kitty_placeholders((3, 2).into(), (1, 1).into(), &ImageOptions::default())
            .unwrap();
        assert_eq!((image.cells().width(), image.cells().height()), (3, 2));
        assert_eq!(image.clone().resource_id(), image.resource_id());
        let mut screen = Screen::new(Vec::new(), (5, 4));
        screen.write_all(image.upload()).unwrap();
        screen.set_color_profile(Profile::TrueColor);
        image.cells().draw(&mut screen, (1, 1).into());
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert_eq!(output.matches("a=t,").count(), 1);
        assert_eq!(output.matches("U=1,").count(), 1);
        assert!(output.find("a=t,").unwrap() < output.find("U=1,").unwrap());
        assert!(output.find("U=1,").unwrap() < output.find('\u{10eeee}').unwrap());
        assert!(output.contains(&format!("p={},", placement_id(image.resource_id()))));
        for y in 0..2 {
            for x in 0..3 {
                let cell = image.cells().cell((x, y).into()).unwrap();
                let chars: Vec<_> = cell.content().chars().collect();
                assert_eq!(cell.width(), 1);
                assert_eq!(chars.len(), 3 + usize::from(image.resource_id() >> 24 != 0));
                assert_eq!(chars[0], '\u{10eeee}');
                assert_eq!(chars[1], DIACRITICS[usize::from(y)]);
                assert_eq!(chars[2], DIACRITICS[usize::from(x)]);
                let high = chars.get(3).map_or(0, |&high| {
                    DIACRITICS.iter().position(|&c| c == high).unwrap() as u8
                });
                let Some(Color::Rgb(red, green, blue)) = cell.style.fg else {
                    panic!()
                };
                let id = u32::from_be_bytes([high, red, green, blue]);
                assert!(output.contains(&format!("i={id},")));
                assert_eq!(
                    cell.style.underline_color,
                    Some(Color::Rgb(0, 0, placement_id(id)))
                );
            }
        }
    }

    #[test]
    fn dimensions_and_profiles_are_explicit() {
        for size in [(0, 1), (1, 0), (298, 1), (1, 298)] {
            assert!(
                image()
                    .kitty_placeholders(size.into(), (1, 1).into(), &ImageOptions::default())
                    .is_err()
            );
        }
        let cells = image()
            .kitty_placeholders((297, 1).into(), (1, 1).into(), &ImageOptions::default())
            .unwrap();
        let mut screen = Screen::new(Vec::new(), (300, 1));
        screen.set_color_profile(Profile::TrueColor);
        let options = cells.placement(&screen).unwrap();
        assert!(Arc::ptr_eq(options.cells.as_ref().unwrap(), &cells.cells));
        assert!(Arc::ptr_eq(&options.upload, &cells.resource.upload));
        assert!(Arc::ptr_eq(&options.delete, &cells.resource.delete));
        for profile in [
            Profile::Ansi256,
            Profile::Ansi,
            Profile::Ascii,
            Profile::Disabled,
        ] {
            screen.set_color_profile(profile);
            assert_eq!(
                cells.placement(&screen).unwrap_err().kind(),
                io::ErrorKind::Unsupported
            );
        }
        assert!(screen.writer().is_empty());
    }

    #[test]
    fn cell_images_do_not_reserve_rows_and_release_after_cell_removal() {
        use uncurses::text::TextSurface;
        let cells = image()
            .kitty_placeholders((2, 1).into(), (1, 1).into(), &ImageOptions::default())
            .unwrap();
        let mut screen = Screen::new(Vec::new(), (6, 2));
        screen.set_color_profile(Profile::TrueColor);
        screen
            .set_placement(
                1,
                uncurses::layout::Rect::new(0, 0, 2, 1),
                &cells.placement(&screen).unwrap(),
            )
            .unwrap();
        assert!(screen.writer().is_empty());
        screen.render().unwrap();
        screen.writer_mut().clear();
        screen.set_str((4, 0), "X", Style::default());
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert!(output.contains('X'));
        assert!(!output.contains("a=t,"));
        assert!(!output.contains('\u{10eeee}'));
        screen.writer_mut().clear();
        screen.clear();
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert_eq!(output.matches("a=d,d=I,").count(), 1);
    }
}
