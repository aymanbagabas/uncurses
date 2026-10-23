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

fn main() -> io::Result<()> {
    let mut dump_output = false;
    let mut placeholders = false;
    let mut path = None;
    for argument in std::env::args().skip(1) {
        if argument == "--dump" {
            dump_output = true;
        } else if argument == "--dump-placeholders" {
            placeholders = true;
        } else if path.is_none() && !argument.starts_with('-') {
            path = Some(argument);
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: images_auto [--dump|--dump-placeholders] [image]",
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
