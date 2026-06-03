//! Image-rendering demo.
//!
//! Loads the image at `argv[1]` and paints it with one of three
//! backends. Press `1` for half-blocks, `2` for sixel, `3` for the
//! kitty unicode-placeholder protocol. `q`, `Esc`, or `Ctrl-C`
//! exits.
//!
//! Backends that need pixel dimensions (sixel, kitty) read the
//! terminal's reported cell-pixel size; if your terminal does not
//! advertise pixel dimensions, only half-blocks will produce
//! output.

use std::io::Write;

use image::DynamicImage;
use uncurses::SurfaceMut;
use uncurses::ansi::mode::{MouseEncoding, MouseMode};
use uncurses::color::BasicColor;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, Source};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses::text::WrapMode;
use uncurses_image::{Halfblocks, Kitty, Resize, Sixel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Halfblocks,
    Sixel,
    Kitty,
}

impl Backend {
    fn label(self) -> &'static str {
        match self {
            Backend::Halfblocks => "half-blocks",
            Backend::Sixel => "sixel",
            Backend::Kitty => "kitty",
        }
    }
}

const HOST_ID: u64 = 1;

fn main() -> std::io::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: image_demo <path-to-image>");
        std::process::exit(2);
    });

    let image = image::open(&path).unwrap_or_else(|e| {
        eprintln!("failed to open {path}: {e}");
        std::process::exit(1);
    });

    let state = enable_raw_mode(stdin(), stdout())?;
    let ws = get_window_size(stdout()).unwrap_or_default();

    let mut screen = Screen::new(stdout()).with_size(ws.col, ws.row);
    screen.set_window_size(ws);
    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(false)?;
    screen.set_mouse_mode(MouseMode::None, MouseEncoding::Sgr)?;

    let halfblocks = Halfblocks::new();
    let mut sixel = Sixel::new();
    let mut kitty = Kitty::new();

    let mut backend = Backend::Halfblocks;
    let mut quit = false;

    redraw(
        &mut screen,
        &image,
        backend,
        &halfblocks,
        &mut sixel,
        &mut kitty,
    )?;
    screen.render()?;
    screen.flush()?;

    let mut events = Source::new(stdin())?;
    while !quit {
        let ev = events.read()?;
        // Always feed size events into the screen's cache so
        // pixel-aware backends pick up live measurements.
        screen.update_window_size(&ev);
        let mut dirty = false;
        match ev {
            Event::KeyPress(Key {
                code: KeyCode::Char('q') | KeyCode::Escape,
                modifiers,
                ..
            }) if modifiers.is_empty() => quit = true,
            Event::KeyPress(Key {
                code: KeyCode::Char('c'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CTRL) => quit = true,
            Event::KeyPress(Key {
                code: KeyCode::Char('1'),
                ..
            }) => {
                backend = Backend::Halfblocks;
                dirty = true;
            }
            Event::KeyPress(Key {
                code: KeyCode::Char('2'),
                ..
            }) => {
                backend = Backend::Sixel;
                dirty = true;
            }
            Event::KeyPress(Key {
                code: KeyCode::Char('3'),
                ..
            }) => {
                backend = Backend::Kitty;
                dirty = true;
            }
            Event::Resize(ws) => {
                screen.resize(ws.col, ws.row);
                dirty = true;
            }
            Event::WindowPixelSize { .. } | Event::CellPixelSize { .. } => {
                dirty = true;
            }
            _ => {}
        }
        if dirty && !quit {
            redraw(
                &mut screen,
                &image,
                backend,
                &halfblocks,
                &mut sixel,
                &mut kitty,
            )?;
            screen.render()?;
            screen.flush()?;
        }
    }

    // Best-effort: ask kitty to forget any registered placement
    // before reset. Sixel has no terminal-side state.
    let _ = kitty.shutdown(&mut screen);
    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state)?;
    Ok(())
}

fn redraw<W: Write>(
    screen: &mut Screen<W>,
    image: &DynamicImage,
    backend: Backend,
    halfblocks: &Halfblocks,
    sixel: &mut Sixel,
    kitty: &mut Kitty,
) -> std::io::Result<()> {
    screen.clear();
    let w = screen.width();
    let h = screen.height();
    if w < 4 || h < 4 {
        return Ok(());
    }

    let header = format!(
        "[1] half-blocks  [2] sixel  [3] kitty   active: {}   q: quit",
        backend.label()
    );
    screen.set_str_with(
        (0, 0),
        &header,
        WrapMode::Truncate,
        Style::EMPTY.with_fg(BasicColor::BrightBlack.into()),
    );

    let area = uncurses::Rect {
        x: 1,
        y: 2,
        width: w.saturating_sub(2),
        height: h.saturating_sub(3),
    };
    if area.width == 0 || area.height == 0 {
        return Ok(());
    }

    match backend {
        Backend::Halfblocks => {
            halfblocks.paint(screen, area, image, Resize::default());
        }
        Backend::Sixel => {
            sixel.paint(screen, area, image, Resize::default(), HOST_ID)?;
        }
        Backend::Kitty => {
            kitty.paint(screen, area, image, Resize::default(), HOST_ID)?;
        }
    }
    Ok(())
}
