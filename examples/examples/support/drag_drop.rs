use std::io::Write;

use uncurses::event::Key;
use uncurses_dragdrop::{DragDrop, Error, ThumbnailFormat};

#[derive(Clone, Copy, Default)]
pub enum Thumbnail {
    None,
    #[default]
    Image,
    Text,
}

impl Thumbnail {
    pub fn select(&mut self, key: &Key) -> bool {
        for (binding, thumbnail) in [("0", Self::None), ("1", Self::Image), ("2", Self::Text)] {
            if *key == binding.parse().unwrap() {
                *self = thumbnail;
                return true;
            }
        }
        false
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Image => "image",
            Self::Text => "text",
        }
    }

    pub fn upload(self, controller: &mut DragDrop, out: &mut impl Write) -> Result<(), Error> {
        match self {
            Self::None => {}
            Self::Image => {
                controller.add_thumbnail(
                    out,
                    ThumbnailFormat::Rgba {
                        width: 16,
                        height: 16,
                    },
                    &[0x88; 16 * 16 * 4],
                )?;
            }
            Self::Text => {
                controller.add_thumbnail(
                    out,
                    ThumbnailFormat::Text {
                        numerator: 1,
                        denominator: 1,
                        opacity: 512,
                    },
                    b"uncurses drag",
                )?;
            }
        }
        Ok(())
    }
}
