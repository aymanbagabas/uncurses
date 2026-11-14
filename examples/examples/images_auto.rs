//! Automatic protocol selection with managed image uploads and cleanup.

use std::io::{self, Write};

use uncurses::{
    buffer::{Surface, SurfaceMut},
    event::{Event, Input},
    layout::{Rect, Size},
    program::Program,
    screen::Screen,
    style::Style,
    text::TextSurface,
};
use uncurses_images::{Image, ImageCapabilities, ImageOptions, ImageProtocol, ImageRenderer};

fn query_images<I: Input, W: Write>(program: &mut Program<I, W>, query: &[u8]) -> io::Result<()> {
    if program
        .env()
        .get("TMUX")
        .is_some_and(|value| !value.is_empty())
    {
        uncurses::ansi::passthrough::write_tmux_passthrough(program.screen_mut(), query)?;
    } else {
        program.screen_mut().write_all(query)?;
    }
    program.screen_mut().flush()
}

fn paint<W: Write>(
    screen: &mut Screen<W>,
    images: &mut ImageRenderer,
    source: &Image,
    step: usize,
) -> io::Result<()> {
    screen.clear();
    screen.set_str(
        (0, 0),
        "Automatic images. Space: move/remove. Q: quit.",
        Style::EMPTY,
    );
    let size = screen.size();
    if size.width < 32 || size.height < 9 {
        images.remove(screen, 1);
        screen.set_str((0, 1), "Resize to at least 32x9.", Style::EMPTY);
    } else if step == 2 {
        images.remove(screen, 1);
        screen.set_str((0, 1), "Removed", Style::EMPTY);
    } else {
        let x = if step == 0 { 2 } else { 18 };
        let protocol = images.draw(
            screen,
            1,
            Rect::new(x, 3, 12, 4),
            source,
            &ImageOptions::default(),
        )?;
        screen.set_str((0, 1), &format!("{protocol:?}"), Style::EMPTY);
    }
    screen.render()
}

fn dump(source: &Image) -> io::Result<()> {
    let mut caps = ImageCapabilities {
        kitty_placeholders: true,
        kitty: true,
        iterm2: true,
        sixel: true,
        cell_pixels: Some(Size::new(8, 16)),
    };
    for protocol in [
        ImageProtocol::KittyPlaceholders,
        ImageProtocol::Kitty,
        ImageProtocol::Iterm2,
        ImageProtocol::Sixel,
        ImageProtocol::Symbols,
    ] {
        let mut screen = Screen::new(Vec::new(), (40, 12));
        screen.set_fullscreen(true);
        screen.set_color_profile(uncurses::color::Profile::TrueColor);
        let mut images = ImageRenderer::new(caps);
        for (name, step) in [("shown", 0), ("unchanged", 0), ("moved", 1), ("removed", 2)] {
            paint(&mut screen, &mut images, source, step)?;
            record(&mut screen, &format!("{protocol:?} {name}"));
        }
        images.remove(&mut screen, 1);
        screen.render()?;
        assert!(screen.writer().is_empty());
        match protocol {
            ImageProtocol::KittyPlaceholders => caps.kitty_placeholders = false,
            ImageProtocol::Kitty => caps.kitty = false,
            ImageProtocol::Iterm2 => caps.iterm2 = false,
            ImageProtocol::Sixel => caps.sixel = false,
            ImageProtocol::Symbols => {}
        }
    }
    Ok(())
}

fn record(screen: &mut Screen<Vec<u8>>, name: &str) {
    print!("{name}\t");
    for byte in screen.writer_mut().drain(..) {
        print!("{byte:02x}");
    }
    println!();
}

fn dump_placeholders(source: &Image) -> io::Result<()> {
    let mut screen = Screen::new(Vec::new(), (40, 12));
    screen.set_fullscreen(true);
    screen.set_synchronized_output(true);
    screen.set_color_profile(uncurses::color::Profile::TrueColor);
    screen.set_optimizations(screen.optimizations() | uncurses::screen::Optimizations::REP);
    let mut images = ImageRenderer::new(ImageCapabilities {
        kitty_placeholders: true,
        cell_pixels: Some(Size::new(8, 16)),
        ..Default::default()
    });
    let options = ImageOptions::default();
    let area = Rect::new(2, 3, 12, 4);
    images.draw(&mut screen, 1, area, source, &options)?;
    screen.render()?;
    record(&mut screen, "shown");
    screen.render()?;
    record(&mut screen, "unchanged");

    screen.set_str((2, 4), "XX", Style::EMPTY);
    screen.render()?;
    record(&mut screen, "origin covered");
    screen.set_str((6, 5), "TOP", Style::EMPTY);
    screen.render()?;
    record(&mut screen, "middle covered");
    images.draw(&mut screen, 2, Rect::new(8, 4, 4, 2), source, &options)?;
    screen.render()?;
    record(&mut screen, "second image");
    images.remove(&mut screen, 2);
    screen.render()?;
    record(&mut screen, "second removed");
    images.draw(&mut screen, 1, area, source, &options)?;
    screen.render()?;
    record(&mut screen, "restored");

    let copied = screen.cell((7, 4).into()).unwrap().clone();
    screen.set_cell((2, 4), &copied);
    screen.render()?;
    record(&mut screen, "source offset");
    images.draw(&mut screen, 1, area, source, &options)?;
    screen.render()?;
    record(&mut screen, "offset restored");
    images.draw(&mut screen, 1, Rect::new(2, 5, 12, 4), source, &options)?;
    screen.render()?;
    record(&mut screen, "vertical");
    images.draw(&mut screen, 1, Rect::new(28, 8, 12, 4), source, &options)?;
    screen.render()?;
    record(&mut screen, "bottom right");

    screen.resize((40, 10));
    images.draw(&mut screen, 1, Rect::new(26, 5, 12, 4), source, &options)?;
    screen.render()?;
    record(&mut screen, "resized");
    screen.set_color_profile(uncurses::color::Profile::Ansi256);
    images.draw(&mut screen, 1, Rect::new(26, 5, 12, 4), source, &options)?;
    screen.render()?;
    record(&mut screen, "symbols");
    screen.set_color_profile(uncurses::color::Profile::TrueColor);
    images.draw(&mut screen, 1, Rect::new(26, 5, 12, 4), source, &options)?;
    screen.render()?;
    record(&mut screen, "placeholders restored");
    images.remove(&mut screen, 1);
    screen.render()?;
    record(&mut screen, "removed");
    Ok(())
}

fn dump_committed(source: &Image) -> io::Result<()> {
    for placeholders in [false, true] {
        let protocol = if placeholders {
            "placeholders"
        } else {
            "kitty"
        };
        let mut screen = Screen::new(Vec::new(), (40, 12));
        screen.set_color_profile(uncurses::color::Profile::TrueColor);
        let mut images = ImageRenderer::new(ImageCapabilities {
            kitty: true,
            kitty_placeholders: placeholders,
            cell_pixels: Some(Size::new(8, 16)),
            ..Default::default()
        });
        images.draw(
            &mut screen,
            1,
            Rect::new(2, 1, 12, 4),
            source,
            &ImageOptions::default(),
        )?;
        screen.render()?;
        record(&mut screen, &format!("{protocol} shown"));
        screen.commit_rows(if placeholders { 3 } else { 5 })?;
        record(&mut screen, &format!("{protocol} committed"));
        images.draw(
            &mut screen,
            1,
            Rect::new(18, 1, 12, 4),
            source,
            &ImageOptions::default(),
        )?;
        screen.render()?;
        record(&mut screen, &format!("{protocol} reused ID"));
        images.remove(&mut screen, 1);
        screen.render()?;
        record(&mut screen, &format!("{protocol} removed"));
        screen.invalidate();
        screen.render()?;
        record(&mut screen, &format!("{protocol} invalidated"));
        images.draw(
            &mut screen,
            2,
            Rect::new(18, 1, 12, 4),
            source,
            &ImageOptions::default(),
        )?;
        screen.render()?;
        record(&mut screen, &format!("{protocol} new image"));
        images.remove(&mut screen, 2);
        screen.render()?;
        record(&mut screen, &format!("{protocol} removed new image"));
        screen.commit_rows(screen.size().height)?;
        record(&mut screen, &format!("{protocol} committed rest"));
    }
    Ok(())
}

fn dump_shared(source: &Image) -> io::Result<()> {
    let blue = Image::from_rgb(1, 1, vec![0, 0, 255])?;
    let green = Image::from_rgb(1, 1, vec![0, 255, 0])?;
    for placeholders in [false, true] {
        let protocol = if placeholders {
            "placeholders"
        } else {
            "kitty"
        };
        let mut screen = Screen::new(Vec::new(), (40, 12));
        screen.set_fullscreen(true);
        screen.set_color_profile(uncurses::color::Profile::TrueColor);
        let mut images = ImageRenderer::new(ImageCapabilities {
            kitty: true,
            kitty_placeholders: placeholders,
            cell_pixels: Some(Size::new(8, 16)),
            ..Default::default()
        });
        for (stage, name) in [
            "shared",
            "unchanged",
            "overlap",
            "reordered",
            "dialog",
            "removed first",
            "replaced",
            "removed all",
        ]
        .into_iter()
        .enumerate()
        {
            screen.clear();
            if stage >= 5 {
                images.remove(&mut screen, 1);
            }
            if stage == 7 {
                images.remove(&mut screen, 2);
            } else {
                let second = if stage < 2 {
                    source
                } else if stage < 6 {
                    &blue
                } else {
                    &green
                };
                let second_area = if stage < 2 {
                    Rect::new(14, 2, 8, 4)
                } else {
                    Rect::new(6, 3, 8, 4)
                };
                let order = if stage >= 3 { [2, 1] } else { [1, 2] };
                for id in order {
                    if id == 1 && stage >= 5 {
                        continue;
                    }
                    let (area, source) = if id == 1 {
                        (Rect::new(2, 2, 8, 4), source)
                    } else {
                        (second_area, second)
                    };
                    images.draw(&mut screen, id, area, source, &ImageOptions::default())?;
                }
                if stage == 4 {
                    let style = Style::new()
                        .fg(uncurses::color::Color::White)
                        .bg(uncurses::color::Color::Black);
                    screen.fill_rect(
                        Rect::new(4, 3, 6, 2),
                        &uncurses::cell::Cell::BLANK.style(&style),
                    );
                    screen.set_str((4, 3), "Dialog", style);
                }
            }
            screen.render()?;
            record(&mut screen, &format!("{protocol} {name}"));
        }
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let mut dump_output = false;
    let mut placeholders = false;
    let mut committed = false;
    let mut shared = false;
    let mut path = None;
    for argument in std::env::args().skip(1) {
        if argument == "--dump" {
            dump_output = true;
        } else if argument == "--dump-placeholders" {
            placeholders = true;
        } else if argument == "--dump-committed" {
            committed = true;
        } else if argument == "--dump-shared" {
            shared = true;
        } else if path.is_none() && !argument.starts_with('-') {
            path = Some(argument);
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: images_auto [--dump|--dump-placeholders|--dump-committed|--dump-shared] [image]",
            ));
        }
    }
    let source = match path {
        Some(path) => Image::open(path)?,
        None => Image::from_rgb(
            16,
            16,
            (0..16)
                .flat_map(|y| (0..16).flat_map(move |x| [x * 16, y * 16, 128]))
                .collect(),
        )?,
    };
    if shared {
        return dump_shared(&source);
    }
    if committed {
        return dump_committed(&source);
    }
    if placeholders {
        return dump_placeholders(&source);
    }
    if dump_output {
        return dump(&source);
    }
    let mut program = Program::stdio()?;
    let mut images = ImageRenderer::from_program(&program);
    let result = (|| {
        program.init()?;
        program.enter_alt_screen()?;
        program.query_capabilities(&[])?;
        query_images(
            &mut program,
            b"\x1b[>q\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\\x1b[16t",
        )?;
        let mut step = 0;
        images.capabilities = ImageCapabilities::from_program(&program);
        paint(program.screen_mut(), &mut images, &source, step)?;
        loop {
            match program.read_event()? {
                Event::KeyPress(key) if key.matches_any(["q", "esc", "ctrl+c"]) => break,
                Event::KeyPress(key) if key.matches("space") => step = (step + 1) % 3,
                Event::Resize(_) => {
                    program.autoresize()?;
                    query_images(&mut program, uncurses::ansi::winop::REQUEST_CELL_PIXEL_SIZE)?;
                }
                _ => {}
            }
            images.capabilities = ImageCapabilities::from_program(&program);
            paint(program.screen_mut(), &mut images, &source, step)?;
        }
        images.remove(program.screen_mut(), 1);
        program.screen_mut().render()
    })();
    result.and(program.finish())
}
