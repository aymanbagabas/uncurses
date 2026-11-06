//! Drag an immutable sized-text block in Kitty, in fullscreen or inline mode.

#[path = "support/placements_drag.rs"]
mod placements_drag;

use std::io;

use placements_drag::{Options, PixelGeometry, Visual};
use uncurses::color::Color;
use uncurses::screen::TextSizing;
use uncurses::style::Style;

fn main() -> io::Result<()> {
    let mut options = Options::default();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--inline" => options.inline = true,
            "--dump" => options.dump = true,
            "--overlay" | "--dialog" => options.dialog = true,
            "--dump-dialog" => {
                options.dump = true;
                options.dump_dialog = true;
            }
            "--cell-pixels" => {
                options.cell_pixels = Some(placements_drag::parse_cell_pixels(arguments.next())?);
            }
            "--help" | "-h" => {
                println!(
                    "sized_text_drag [--inline] [--overlay|--dialog] [--dump] [--dump-dialog]\nSpace toggles a dialog. Shift+arrows move it."
                );
                return Ok(());
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown argument: {argument}"),
                ));
            }
        }
    }
    let sizing = TextSizing {
        scale: 3,
        width: 4,
        ..TextSizing::default()
    };
    let style = Style::default()
        .bold()
        .fg(Color::Rgb(255, 255, 255))
        .bg(Color::Rgb(0, 0, 0));
    placements_drag::run("Sized text", options, PixelGeometry::Unused, |_| {
        Ok(Visual::SizedText {
            size: sizing.size()?,
            text: "Drag".to_owned(),
            sizing,
            style: style.clone(),
        })
    })
}
