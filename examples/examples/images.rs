//! Image fitting, shared native resources, text overlays, and symbols.
//!
//! Run with `--kitty`, `--iterm2`, or `--sixel` only on a compatible terminal.
//! The default uses automatic selection with unknown support, so it uses cells.
//! Use `--auto-kitty` to enable Kitty support and select placeholders automatically.
//! Use `--kitty-placeholders` to select placeholder cells explicitly.
//! Add `--inline` for the normal buffer, or `--dump` for native parser tests.
//! Use `--dither noise`, `--dither-grain 1x1`, and `--dither-intensity 1` for Sixel.

use std::io::{self, Write};
use std::time::Duration;

use uncurses::ansi::mode::Mode;
use uncurses::buffer::SurfaceMut;
use uncurses::layout::{Rect, Size};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;
use uncurses_image::{
    Crop, Fit, Image, ImageGroup, ImageOptions, ImageRenderer, Protocol, SymbolSet,
};

#[path = "support/image_dither_args.rs"]
mod image_dither_args;

fn picture() -> io::Result<Image> {
    let pixels = (0..32)
        .flat_map(|y| {
            (0..48).flat_map(move |x| {
                if x < 16 {
                    [255, 0, 0, 255]
                } else if x < 32 {
                    [0, 255, 0, 255]
                } else {
                    [0, 0, 255, if y < 16 { 128 } else { 255 }]
                }
            })
        })
        .collect();
    Image::from_rgba(48, 32, pixels)
}

fn demonstrate(
    renderer: ImageRenderer,
    options: ImageOptions,
    inline: bool,
    mut show: impl FnMut(&str, &[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let mut screen = Screen::new(Vec::new(), Size::new(40, 12));
    screen.set_fullscreen(!inline);
    screen.set_synchronized_output(true);
    screen.set_cursor_position((0, 11));
    let image = picture()?;
    let rect = Rect::new(2, 2, 12, 4);
    let mut selected = Protocol::Auto;
    let present = |name: &str, screen: &mut Screen<Vec<u8>>| {
        screen.render()?;
        let bytes = std::mem::take(screen.writer_mut());
        let length = bytes.len();
        show(name, &bytes)?;
        Ok::<_, io::Error>(length)
    };
    let mut present = present;
    for stage in ["contain", "identical rebuild"] {
        screen.clear();
        screen.set_str((0, 0), "Images: contain / equal rebuild", Style::default());
        screen.set_str((2, 2), "underlying text", Style::default());
        selected = renderer.draw(&mut screen, "image", rect, &image, &options)?;
        let length = present(stage, &mut screen)?;
        if stage == "identical rebuild" {
            assert_eq!(length, 0);
        }
    }
    renderer.draw(
        &mut screen,
        "copy",
        Rect::new(22, 2, 12, 4),
        &image,
        &options,
    )?;
    present("shared image", &mut screen)?;
    if matches!(selected, Protocol::Symbols(_) | Protocol::KittyPlaceholders) {
        screen.clear_rect(rect);
    }
    renderer.draw(
        &mut screen,
        "image",
        Rect::new(2, 6, 12, 4),
        &image,
        &options,
    )?;
    present("move", &mut screen)?;
    screen.set_str((5, 7), "TEXT", Style::default().bold());
    present("text overlay", &mut screen)?;
    screen.clear_rect(Rect::new(6, 8, 3, 1));
    present("partial clear", &mut screen)?;
    screen.set_str((5, 7), "EDIT", Style::default());
    screen.insert_above("image log")?;
    show("replay", &std::mem::take(screen.writer_mut()))?;
    let mut present = |name: &str, screen: &mut Screen<Vec<u8>>| {
        screen.render()?;
        show(name, &std::mem::take(screen.writer_mut()))
    };
    present("pending edit", &mut screen)?;
    screen.remove_region("image");
    screen.remove_region("copy");
    screen.clear_rect(Rect::new(2, 6, 12, 4));
    screen.clear_rect(Rect::new(22, 2, 12, 4));
    present("remove", &mut screen)?;

    let cases = [
        (
            "cover",
            ImageOptions {
                fit: Fit::Cover,
                ..options.clone()
            },
        ),
        (
            "stretch",
            ImageOptions {
                fit: Fit::Stretch,
                ..options.clone()
            },
        ),
        (
            "original",
            ImageOptions {
                fit: Fit::Original,
                ..options.clone()
            },
        ),
        (
            "scale",
            ImageOptions {
                scale: 0.5,
                ..options.clone()
            },
        ),
        (
            "ratio",
            ImageOptions {
                aspect_ratio: Some((1, 1)),
                ..options.clone()
            },
        ),
        (
            "crop",
            ImageOptions {
                crop: Some(Crop {
                    x: 16,
                    y: 0,
                    width: 16,
                    height: 32,
                }),
                ..options.clone()
            },
        ),
    ];
    for (name, options) in cases {
        screen.clear();
        screen.set_str((0, 0), name, Style::default());
        renderer.draw(&mut screen, "image", rect, &image, &options)?;
        present(name, &mut screen)?;
    }
    screen.invalidate();
    present("invalidate", &mut screen)?;
    screen.release_regions()?;
    show("handoff", &std::mem::take(screen.writer_mut()))?;
    screen.render()?;
    show("resume", &std::mem::take(screen.writer_mut()))?;
    screen.clear();
    screen.render()?;
    show("cleanup", &std::mem::take(screen.writer_mut()))?;
    if matches!(
        renderer.protocol,
        Protocol::Kitty | Protocol::KittyPlaceholders
    ) {
        let placeholders = image.kitty_placeholders(Size::new(12, 4), &options)?;
        let mut present = |name: &str, screen: &mut Screen<Vec<u8>>| {
            screen.render()?;
            show(name, &std::mem::take(screen.writer_mut()))
        };
        for name in ["placeholders", "placeholder rebuild"] {
            screen.clear();
            placeholders.draw(&mut screen, "cells", (2, 2).into())?;
            placeholders.draw(&mut screen, "copy", (22, 2).into())?;
            present(name, &mut screen)?;
        }
        screen.clear_rect(Rect::new(2, 2, 12, 4));
        placeholders.draw(&mut screen, "cells", (2, 6).into())?;
        present("placeholder move", &mut screen)?;
        screen.set_str((5, 7), "TEXT", Style::default());
        present("placeholder text", &mut screen)?;
        screen.set_str((5, 7), "EDIT", Style::default());
        screen.insert_above("placeholder log")?;
        show("placeholder replay", &std::mem::take(screen.writer_mut()))?;
        screen.render()?;
        show("placeholder edit", &std::mem::take(screen.writer_mut()))?;
        screen.clear();
        screen.render()?;
        show("placeholder cleanup", &std::mem::take(screen.writer_mut()))?;
        let mut group = ImageGroup::new()?;
        group.set("base", None, (0, 0), Size::new(8, 3), &image, &options)?;
        group.set(
            "child",
            Some("base"),
            (4, 1),
            Size::new(8, 3),
            &image,
            &options,
        )?;
        let mut present = |name: &str, screen: &mut Screen<Vec<u8>>| {
            screen.render()?;
            show(name, &std::mem::take(screen.writer_mut()))
        };
        group.draw(&mut screen, "group", (2, 2).into())?;
        present("relative group", &mut screen)?;
        group.draw(&mut screen, "group", (2, 2).into())?;
        present("relative rebuild", &mut screen)?;
        group.draw(&mut screen, "group", (22, 6).into())?;
        present("relative move", &mut screen)?;
        screen.set_str((26, 7), "TEXT", Style::default());
        present("relative text", &mut screen)?;
        screen.remove_region("group");
        present("relative remove", &mut screen)?;
        let anchor = image.kitty_placeholders(Size::new(1, 1), &options)?;
        anchor.draw(&mut screen, "anchor", (2, 2).into())?;
        group.attach(&mut screen, "attached", &anchor)?;
        present("attached group", &mut screen)?;
        screen.delete_lines(0, 1, 12, &uncurses::cell::Cell::BLANK);
        present("anchor row move", &mut screen)?;
        screen.insert_cells((2, 1).into(), 3, 40, &uncurses::cell::Cell::BLANK);
        present("anchor column move", &mut screen)?;
        screen.set_str((9, 2), "TEXT", Style::default());
        present("attached text", &mut screen)?;
        screen.clear_rect(Rect::new(5, 1, 1, 1));
        present("anchor erase", &mut screen)?;
        anchor.draw(&mut screen, "anchor", (2, 6).into())?;
        present("anchor return", &mut screen)?;
        screen.set_str((6, 7), "EDIT", Style::default());
        screen.insert_above("relative log")?;
        show("attached replay", &std::mem::take(screen.writer_mut()))?;
        screen.render()?;
        show("attached edit", &std::mem::take(screen.writer_mut()))?;
        screen.resize((10, 8));
        screen.render()?;
        show("attached shrink", &std::mem::take(screen.writer_mut()))?;
        screen.resize((40, 12));
        screen.render()?;
        show("attached grow", &std::mem::take(screen.writer_mut()))?;
        screen.clear();
        screen.render()?;
        show("relative cleanup", &std::mem::take(screen.writer_mut()))?;
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let mut renderer = ImageRenderer::default();
    let mut options = ImageOptions::default();
    let mut image_args = image_dither_args::ImageArgs::default();
    let mut dump = false;
    let mut preview = false;
    let mut inline = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--kitty" => renderer.protocol = Protocol::Kitty,
            "--kitty-placeholders" => {
                renderer.protocol = Protocol::KittyPlaceholders;
            }
            "--auto-kitty" => {
                renderer.protocol = Protocol::Auto;
                renderer.capabilities.kitty = true;
            }
            "--iterm2" => renderer.protocol = Protocol::Iterm2,
            "--sixel" => renderer.protocol = Protocol::Sixel,
            "--symbols" => {
                let selection = args.next().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "--symbols needs a selector")
                })?;
                renderer.protocol = Protocol::Symbols(selection.parse()?);
            }
            "--half-blocks" => renderer.protocol = Protocol::Symbols(SymbolSet::UPPER_LOWER),
            "--dump" => dump = true,
            "--preview" => preview = true,
            "--inline" => inline = true,
            _ if image_dither_args::parse(&arg, &mut args, &mut image_args)? => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown argument: {arg}"),
                ));
            }
        }
    }
    image_args.apply(&mut options)?;
    let live = !dump && !preview;
    let mut terminal = uncurses::terminal::Terminal::stdio();
    let mut out = io::stdout().lock();
    if live {
        let size = terminal.get_window_size()?;
        if size.col < 40 || size.row < 12 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the example needs 40 columns and 12 rows",
            ));
        }
        terminal.make_raw()?;
    }
    let result = (|| {
        if live && !inline {
            Mode::ALT_SCREEN_SAVE_CURSOR.set(&mut out)?;
        }
        demonstrate(renderer, options, inline, |name, bytes| {
            if dump {
                write!(out, "{name}\t")?;
                for byte in bytes {
                    write!(out, "{byte:02x}")?;
                }
                writeln!(out)?;
            } else if preview {
                writeln!(out, "{name}: {} bytes", bytes.len())?;
            } else {
                out.write_all(bytes)?;
                out.flush()?;
                std::thread::sleep(Duration::from_millis(900));
            }
            Ok(())
        })
    })();
    let restore = if live && !inline {
        Mode::ALT_SCREEN_SAVE_CURSOR
            .reset(&mut out)
            .and_then(|()| out.flush())
    } else if live {
        out.write_all(b"\r\n").and_then(|()| out.flush())
    } else {
        Ok(())
    };
    let raw_restore = if live { terminal.restore() } else { Ok(()) };
    result?;
    restore?;
    raw_restore
}
