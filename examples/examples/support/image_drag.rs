//! Shared image viewer for fullscreen and inline drag examples.

use std::io::{self, Write};
use std::{any::Any, sync::Arc};

use uncurses::ansi::graphics::write_kitty_graphics;
use uncurses::buffer::SurfaceMut;
use uncurses::cell::Cell;
use uncurses::color::Color;
use uncurses::event::{Event, Key, MouseButton};
use uncurses::layout::{Position, Rect, Size};
use uncurses::program::{MouseTracking, Program, ProgramOptions};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;
use uncurses_image::{
    Fit, Image, ImageCapabilities, ImageOptions, ImageRenderer, Protocol, SymbolSet,
    dither::{Bayer8x8, FloydSteinberg, Noise},
};

#[path = "image_symbol_sets.rs"]
mod image_symbol_sets;
use image_symbol_sets::SYMBOL_SETS;

const IMAGE_SIZE: Size = Size::new(24, 8);
const INLINE_HEIGHT: u16 = 16;
const TOP: u16 = 2;
const PROTOCOLS: [Protocol; 6] = [
    Protocol::Auto,
    Protocol::Symbols(SymbolSet::DEFAULT),
    Protocol::Iterm2,
    Protocol::Sixel,
    Protocol::Kitty,
    Protocol::KittyPlaceholders,
];

fn protocol_name(protocol: &Protocol) -> &'static str {
    match protocol {
        Protocol::Auto => "Auto",
        Protocol::Symbols(_) => "Symbols",
        Protocol::Iterm2 => "iTerm2",
        Protocol::Sixel => "Sixel",
        Protocol::Kitty => "Kitty native",
        Protocol::KittyPlaceholders => "Kitty Unicode",
    }
}

fn dither_name(algorithm: Option<&dyn uncurses_image::dither::Dither>) -> &'static str {
    match algorithm.map(|d| d as &dyn Any) {
        None => "None",
        Some(d) if d.is::<Bayer8x8>() => "Bayer8x8",
        Some(d) if d.is::<FloydSteinberg>() => "Floyd-Steinberg",
        Some(d) if d.is::<Noise>() => "Noise",
        _ => "Custom",
    }
}

struct Drag {
    rect: Rect,
    offset: Option<Position>,
}

impl Drag {
    fn fits(&self, size: Size) -> bool {
        size.width >= self.rect.width && size.height > TOP + self.rect.height
    }

    fn clamp(&mut self, size: Size) {
        self.rect.x = self.rect.x.min(size.width.saturating_sub(self.rect.width));
        self.rect.y = self.rect.y.clamp(
            TOP,
            size.height.saturating_sub(self.rect.height + 1).max(TOP),
        );
    }

    fn mouse(&mut self, event: Event, size: Size) -> bool {
        match event {
            Event::MouseClick(mouse)
                if mouse.button == MouseButton::Left
                    && self.fits(size)
                    && self.rect.contains((mouse.x, mouse.y)) =>
            {
                self.offset = Some(Position::new(mouse.x - self.rect.x, mouse.y - self.rect.y));
            }
            Event::MouseMove(mouse)
                if mouse.button == MouseButton::Left && self.offset.is_some() =>
            {
                self.move_to((mouse.x, mouse.y).into(), size);
            }
            Event::MouseRelease(mouse)
                if matches!(mouse.button, MouseButton::Left | MouseButton::None)
                    && self.offset.is_some() =>
            {
                self.move_to((mouse.x, mouse.y).into(), size);
                self.offset = None;
            }
            _ => return false,
        }
        true
    }

    fn move_to(&mut self, pointer: Position, size: Size) {
        if let Some(offset) = self.offset {
            self.rect.x = pointer.x.saturating_sub(offset.x);
            self.rect.y = pointer.y.saturating_sub(offset.y);
            self.clamp(size);
        }
    }
}

struct App {
    image: Image,
    drag: Drag,
    renderer: ImageRenderer,
    options: ImageOptions,
    measured_pixels: bool,
    symbol_index: usize,
}

impl App {
    fn new(image: Image, size: Size) -> Self {
        Self {
            image,
            drag: Drag {
                rect: Rect::new(
                    size.width.saturating_sub(IMAGE_SIZE.width) / 2,
                    TOP + size.height.saturating_sub(TOP + IMAGE_SIZE.height + 1) / 2,
                    IMAGE_SIZE.width,
                    IMAGE_SIZE.height,
                ),
                offset: None,
            },
            renderer: ImageRenderer::default(),
            options: ImageOptions::default(),
            measured_pixels: false,
            symbol_index: 0,
        }
    }

    fn key(&mut self, key: &Key) -> bool {
        if let Some(index) = ["0", "1", "2", "3", "4", "5"]
            .iter()
            .position(|binding| *key == binding.parse::<Key>().unwrap())
        {
            self.select_protocol(index);
        } else if *key == "tab".parse::<Key>().unwrap() {
            let index = PROTOCOLS
                .iter()
                .position(|protocol| {
                    *protocol == self.renderer.protocol
                        || matches!(
                            (protocol, &self.renderer.protocol),
                            (Protocol::Symbols(_), Protocol::Symbols(_))
                        )
                })
                .unwrap();
            self.select_protocol((index + 1) % PROTOCOLS.len());
        } else if *key == "s".parse::<Key>().unwrap() {
            self.symbol_index = (self.symbol_index + 1) % SYMBOL_SETS.len();
            self.renderer.protocol = Protocol::Symbols(SYMBOL_SETS[self.symbol_index].1.clone());
        } else if *key == "f".parse::<Key>().unwrap() {
            self.options.fit = match self.options.fit {
                Fit::Contain => Fit::Cover,
                Fit::Cover => Fit::Stretch,
                Fit::Stretch => Fit::Original,
                Fit::Original => Fit::Contain,
            };
        } else if *key == "d".parse::<Key>().unwrap() {
            self.options.dither = match self.options.dither.as_deref().map(|d| d as &dyn Any) {
                None => Some(Arc::new(Bayer8x8::default())),
                Some(d) if d.is::<Bayer8x8>() => Some(Arc::new(FloydSteinberg::default())),
                Some(d) if d.is::<FloydSteinberg>() => Some(Arc::new(Noise::default())),
                _ => None,
            };
        } else {
            return false;
        }
        true
    }

    fn select_protocol(&mut self, index: usize) {
        self.renderer.protocol = match &PROTOCOLS[index] {
            Protocol::Symbols(_) => Protocol::Symbols(SYMBOL_SETS[self.symbol_index].1.clone()),
            protocol => protocol.clone(),
        };
    }

    fn render(&self, screen: &mut Screen<impl Write>) -> io::Result<()> {
        let background = Style::default()
            .fg(Color::rgb(58, 72, 89))
            .bg(Color::rgb(14, 20, 28));
        screen.fill(&Cell::new(".", 1).style(background));
        let size = screen.size();
        let selected = if self.drag.fits(size) {
            Some(self.renderer.draw(
                screen,
                "picture",
                self.drag.rect,
                &self.image,
                &self.options,
            )?)
        } else {
            None
        };
        let bar = Style::default().fg(Color::White).bg(Color::rgb(30, 48, 65));
        let messages = if screen.fullscreen() {
            [
                "Drag | 0 Auto  1 Symbols  2 iTerm2  3 Sixel  4 Kitty native  5 Kitty Unicode",
                "Tab protocols | s symbols | d dither | f fit | q/Esc quits",
            ]
        } else {
            [
                "Inline images | 0 Auto  1 Symbols  2 iTerm2  3 Sixel  4 Kitty  5 Placeholders",
                "Tab protocols | Space log | s symbols | d dither | f fit | q/Esc quits",
            ]
        };
        for (y, message) in messages.into_iter().enumerate() {
            if y < usize::from(size.height) {
                let row = Rect::new(0, y as u16, size.width, 1);
                screen.fill_rect(row, &Cell::BLANK.style(bar.clone()));
                screen.set_str(row.position(), message, bar.clone());
            }
        }
        if size.height > 0 {
            let status = if let Some(selected) = selected {
                format!(
                    "{} -> {}{} | {:?} | {:?} | ({}, {}) | {}x{} px{}{}",
                    protocol_name(&self.renderer.protocol),
                    protocol_name(&selected),
                    if matches!(selected, Protocol::Symbols(_)) {
                        let index = if self.renderer.protocol == Protocol::Auto {
                            0
                        } else {
                            self.symbol_index
                        };
                        format!(" ({})", SYMBOL_SETS[index].0)
                    } else {
                        String::new()
                    },
                    self.options.fit,
                    dither_name(self.options.dither.as_deref()),
                    self.drag.rect.x,
                    self.drag.rect.y,
                    self.options.cell_pixels.0,
                    self.options.cell_pixels.1,
                    if self.measured_pixels {
                        ""
                    } else {
                        " estimate"
                    },
                    if self.drag.offset.is_some() {
                        " | dragging"
                    } else {
                        ""
                    },
                )
            } else {
                format!(
                    "Resize to at least {}x{}",
                    IMAGE_SIZE.width,
                    TOP + IMAGE_SIZE.height + 1
                )
            };
            let row = Rect::new(0, size.height - 1, size.width, 1);
            screen.fill_rect(row, &Cell::BLANK.style(bar.clone()));
            screen.set_str(row.position(), &status, bar);
        }
        screen.render()
    }
}

fn picture() -> io::Result<Image> {
    let pixels = (0..64)
        .flat_map(|y| {
            (0..96).flat_map(move |x| {
                [
                    (x * 255 / 95) as u8,
                    (y * 255 / 63) as u8,
                    if (x / 8 + y / 8) % 2 == 0 { 200 } else { 60 },
                    255,
                ]
            })
        })
        .collect();
    Image::from_rgba(96, 64, pixels)
}

fn viewport(size: Size, inline: bool) -> Size {
    Size::new(
        size.width,
        if inline {
            size.height.min(INLINE_HEIGHT)
        } else {
            size.height
        },
    )
}

fn run_program(program: &mut Program<Stdin, Stdout>, image: Image, inline: bool) -> io::Result<()> {
    if inline {
        let size = viewport(program.screen().size(), true);
        program.screen_mut().resize(size);
    } else {
        program.enter_alt_screen()?;
    }
    program.hide_cursor()?;
    let mut probe = Vec::new();
    write_kitty_graphics(&mut probe, &["a=q,t=d,f=24,s=1,v=1,i=1"], b"AAAA")?;
    program.query_capabilities(&probe)?;
    program.request_cell_pixel_size()?;
    let mut app = App::new(image, program.screen().size());
    let quit: [Key; 3] = ["q", "esc", "ctrl+c"].map(|key| key.parse().unwrap());
    let space: Key = "space".parse().unwrap();
    let mut log_lines = 0;
    let mut request_origin = inline;
    loop {
        app.renderer.capabilities =
            ImageCapabilities::detect(program.capabilities(), program.env());
        let pixels = program.cell_pixels();
        app.measured_pixels = pixels.is_some();
        app.options.cell_pixels = pixels.map_or((8, 16), |p| (p.width, p.height));
        app.render(program.screen_mut())?;
        if request_origin {
            program.request_origin()?;
            request_origin = false;
        }
        match program.read_event()? {
            Event::KeyPress(ref key) if quit.contains(key) => return Ok(()),
            Event::KeyPress(ref key) if inline && *key == space => {
                log_lines += 1;
                program.screen_mut().insert_above(&format!(
                    "Image log {log_lines}: drag stays inside the inline viewport."
                ))?;
                request_origin = true;
            }
            Event::KeyPress(ref key) => {
                if !app.key(key) {
                    continue;
                }
            }
            Event::Resize(ws) => {
                if inline {
                    let size = viewport(Size::new(ws.col, ws.row), true);
                    if program.screen().size() != size {
                        program.screen_mut().resize(size);
                    }
                } else {
                    program.autoresize()?;
                }
                app.drag.offset = None;
                app.drag.clamp(program.screen().size());
                program.request_cell_pixel_size()?;
                request_origin = inline;
            }
            event @ (Event::MouseClick(_) | Event::MouseMove(_) | Event::MouseRelease(_)) => {
                let event = match event {
                    Event::MouseClick(mouse) => Event::MouseClick(program.mouse_to_origin(mouse)),
                    Event::MouseMove(mouse) => Event::MouseMove(program.mouse_to_origin(mouse)),
                    Event::MouseRelease(mouse) => {
                        Event::MouseRelease(program.mouse_to_origin(mouse))
                    }
                    _ => unreachable!(),
                };
                if !app.drag.mouse(event, program.screen().size()) {
                    continue;
                }
            }
            _ => {}
        }
    }
}

fn clear_inline(screen: &mut Screen<impl Write>) -> io::Result<()> {
    screen.clear();
    screen.set_cursor_position(Position::ORIGIN);
    screen.render()
}

pub fn run(inline: bool) -> io::Result<()> {
    let example = if inline {
        "images_inline"
    } else {
        "images_drag"
    };
    let mut args = std::env::args_os().skip(1);
    let path = args.next();
    if args.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("usage: {example} [image.png]"),
        ));
    }
    if path.as_deref() == Some(std::ffi::OsStr::new("--help")) {
        println!(
            "Usage: {example} [image.png]\nDrag with the left mouse. 0 Auto, 1 symbols, 2 iTerm2, 3 Sixel, 4 Kitty native, 5 Kitty placeholders.\nTab cycles representations; s cycles all symbol sets; d cycles dithers; f cycles fit modes; q/Esc quits.\nNative selections require a compatible terminal. Kitty placeholders also require true color."
        );
        if inline {
            println!(
                "Uses up to {INLINE_HEIGHT} rows in the normal screen. Space adds a log line above the image."
            );
        }
        return Ok(());
    }
    let image = match path {
        Some(path) => Image::open(path)?,
        None => picture()?,
    };
    let mut program = Program::stdio()?;
    program.init_with(ProgramOptions {
        mouse: Some(MouseTracking::empty()),
        ..Default::default()
    })?;
    let result = run_program(&mut program, image, inline);
    let cleanup = if inline {
        clear_inline(program.screen_mut())
    } else {
        Ok(())
    };
    let finish = program.finish();
    result?;
    cleanup?;
    finish
}

#[cfg(test)]
mod tests {
    use super::*;
    use uncurses::buffer::Surface;
    use uncurses::color::Profile;
    use uncurses::event::{KeyModifiers, Mouse};

    fn mouse(x: u16, y: u16) -> Mouse {
        Mouse {
            x,
            y,
            button: MouseButton::Left,
            modifiers: KeyModifiers::empty(),
        }
    }

    #[test]
    fn inline_viewport_tracks_terminal_size() {
        assert_eq!(viewport(Size::new(80, 24), true), Size::new(80, 16));
        assert_eq!(viewport(Size::new(40, 10), true), Size::new(40, 10));
        assert_eq!(viewport(Size::new(80, 24), false), Size::new(80, 24));
    }

    #[test]
    fn inline_images_survive_logs_drag_and_resize() {
        for protocol in [
            Protocol::Symbols(SymbolSet::UPPER_LOWER),
            Protocol::Kitty,
            Protocol::KittyPlaceholders,
            Protocol::Iterm2,
            Protocol::Sixel,
        ] {
            let size = viewport(Size::new(80, 24), true);
            let mut screen = Screen::new(Vec::new(), size);
            screen.set_fullscreen(false);
            screen.set_color_profile(Profile::TrueColor);
            let mut app = App::new(picture().unwrap(), size);
            app.renderer.protocol = protocol;
            app.render(&mut screen).unwrap();
            screen.writer_mut().clear();
            app.render(&mut screen).unwrap();
            assert!(screen.writer().is_empty());

            screen.insert_above("Image log 1").unwrap();
            app.render(&mut screen).unwrap();
            let start = app.drag.rect.position();
            app.drag
                .mouse(Event::MouseClick(mouse(start.x + 3, start.y + 2)), size);
            app.drag.mouse(Event::MouseMove(mouse(3, 4)), size);
            app.drag.mouse(Event::MouseRelease(mouse(3, 4)), size);
            assert_eq!(app.drag.rect.position(), Position::new(0, TOP));
            assert!(app.drag.offset.is_none());
            app.render(&mut screen).unwrap();
            assert_eq!(screen.cell(start).unwrap().content(), ".");

            for terminal_size in [Size::new(20, 6), Size::new(80, 30)] {
                let size = viewport(terminal_size, true);
                screen.resize(size);
                app.drag.clamp(size);
                app.render(&mut screen).unwrap();
                assert_eq!(screen.size(), size);
                assert!(!screen.fullscreen());
            }
            assert!(
                !screen
                    .writer()
                    .windows(8)
                    .any(|bytes| bytes == b"\x1b[?1049h")
            );
            clear_inline(&mut screen).unwrap();
            assert_eq!(screen.cell(Position::ORIGIN), Some(&Cell::BLANK));
        }
    }

    #[test]
    fn every_symbol_set_renders_and_survives_protocol_switches() {
        let mut screen = Screen::new(Vec::new(), (80, 24));
        screen.set_fullscreen(true);
        let mut app = App::new(picture().unwrap(), screen.size());
        app.key(&"1".parse().unwrap());
        for (index, (_, symbols)) in SYMBOL_SETS.iter().enumerate() {
            assert_eq!(app.symbol_index, index);
            assert_eq!(app.renderer.protocol, Protocol::Symbols(symbols.clone()));
            app.render(&mut screen).unwrap();
            screen.writer_mut().clear();
            app.render(&mut screen).unwrap();
            assert!(screen.writer().is_empty());
            app.key(&"tab".parse().unwrap());
            assert_eq!(app.renderer.protocol, Protocol::Iterm2);
            app.key(&"1".parse().unwrap());
            assert_eq!(app.renderer.protocol, Protocol::Symbols(symbols.clone()));
            app.key(&"s".parse().unwrap());
        }
        assert_eq!(app.symbol_index, 0);
        assert_eq!(app.renderer.protocol, Protocol::Symbols(SymbolSet::DEFAULT));
    }

    #[test]
    fn drag_keeps_the_grab_offset_and_reserves_headers_and_footer() {
        let size = Size::new(80, 24);
        let mut app = App::new(picture().unwrap(), size);
        assert!(!app.drag.mouse(Event::MouseClick(mouse(0, 0)), size));
        let start = app.drag.rect.position();
        app.drag
            .mouse(Event::MouseClick(mouse(start.x + 3, start.y + 2)), size);
        app.drag.mouse(Event::MouseMove(mouse(13, 8)), size);
        assert_eq!(app.drag.rect.position(), Position::new(10, 6));
        app.drag.mouse(Event::MouseMove(mouse(79, 23)), size);
        assert_eq!(app.drag.rect.position(), Position::new(56, 15));
        app.drag.mouse(Event::MouseRelease(mouse(0, 0)), size);
        assert_eq!(app.drag.rect.position(), Position::new(0, TOP));
        assert!(app.drag.offset.is_none());
        assert!(!app.drag.fits(Size::new(23, 10)));
    }

    #[test]
    fn switching_protocols_and_moving_rebuilds_the_background() {
        let size = Size::new(80, 24);
        let mut screen = Screen::new(Vec::new(), size);
        screen.set_fullscreen(true);
        screen.set_color_profile(Profile::TrueColor);
        let mut app = App::new(picture().unwrap(), size);
        for key in [
            "0", "1", "4", "2", "3", "4", "tab", "4", "5", "1", "5", "tab",
        ] {
            assert!(app.key(&key.parse().unwrap()));
            let old = app.drag.rect.position();
            app.render(&mut screen).unwrap();
            let bytes = std::mem::take(screen.writer_mut());
            match app.renderer.protocol {
                Protocol::Kitty => {
                    assert!(bytes.windows(3).any(|b| b == b"a=p"))
                }
                Protocol::Iterm2 => {
                    assert!(bytes.windows(7).any(|b| b == b"a=d,d=I"));
                    assert!(bytes.windows(7).any(|b| b == b"]1337;F"));
                }
                Protocol::Sixel => assert!(bytes.windows(2).any(|b| b == b"\x1bP")),
                Protocol::KittyPlaceholders => {
                    assert!(
                        screen
                            .cell(old)
                            .unwrap()
                            .content()
                            .starts_with('\u{10eeee}')
                    );
                    assert!(bytes.windows(3).any(|b| b == b"U=1"));
                }
                _ => {}
            }
            app.render(&mut screen).unwrap();
            assert!(screen.writer().is_empty(), "equal frame for {key}");
            app.drag.rect.x = if old.x == 0 { 40 } else { 0 };
            app.render(&mut screen).unwrap();
            assert_eq!(screen.cell(old).unwrap().content(), ".");
            if matches!(
                app.renderer.protocol,
                Protocol::Kitty | Protocol::KittyPlaceholders
            ) {
                assert!(!screen.writer().windows(3).any(|b| b == b"a=t"));
            }
            screen.writer_mut().clear();
        }
        assert_eq!(app.renderer.protocol, Protocol::Auto);
        for fit in [Fit::Cover, Fit::Stretch, Fit::Original, Fit::Contain] {
            app.key(&"f".parse().unwrap());
            assert_eq!(app.options.fit, fit);
        }
        screen.resize((10, 5));
        app.drag.clamp(screen.size());
        app.render(&mut screen).unwrap();
        assert_eq!(screen.cell((0, 4).into()).unwrap().content(), "R");
    }

    #[test]
    fn placeholders_follow_fit_and_cell_geometry_changes() {
        let mut screen = Screen::new(Vec::new(), (80, 24));
        screen.set_fullscreen(true);
        let mut app = App::new(picture().unwrap(), screen.size());
        app.key(&"5".parse().unwrap());
        screen.set_color_profile(Profile::Ansi256);
        assert_eq!(
            app.render(&mut screen).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        assert!(screen.writer().is_empty());
        screen.set_color_profile(Profile::TrueColor);
        app.render(&mut screen).unwrap();
        let position = app.drag.rect.position();
        let mut previous = screen.cell(position).unwrap().clone();
        for fit in [Fit::Cover, Fit::Stretch, Fit::Original, Fit::Contain] {
            app.key(&"f".parse().unwrap());
            app.render(&mut screen).unwrap();
            assert_eq!(app.options.fit, fit);
            let cell = screen.cell(position).unwrap();
            assert_ne!(cell, &previous);
            previous = cell.clone();
        }
        app.options.cell_pixels = (10, 20);
        app.render(&mut screen).unwrap();
        assert_eq!(app.options.cell_pixels, (10, 20));
        assert_ne!(screen.cell(position).unwrap(), &previous);
        screen.writer_mut().clear();
        screen.resize((30, 10));
        app.drag.clamp(screen.size());
        app.render(&mut screen).unwrap();
        assert!(screen.writer().windows(7).any(|b| b == b"a=d,d=I"));
    }
}
