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
//!
//! Demonstrates how a host implements runtime backend selection
//! against the [`Painter`] trait. The crate ships no built-in
//! dispatch wrapper; the local `Backend` enum below is the host's
//! own.

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
use uncurses_image::{Halfblocks, ImageId, Kitty, Painter, Resize, Sixel};

/// Runtime-selectable backend. Each variant owns its concrete
/// painter; the host dispatches manually because the crate does not
/// ship an enum.
enum Backend {
    Halfblocks(Halfblocks),
    Sixel(Sixel),
    Kitty(Kitty),
}

impl Backend {
    fn label(&self) -> &'static str {
        match self {
            Backend::Halfblocks(_) => "half-blocks",
            Backend::Sixel(_) => "sixel",
            Backend::Kitty(_) => "kitty",
        }
    }

    fn paint<W: Write>(
        &mut self,
        screen: &mut Screen<W>,
        area: uncurses::Rect,
        image: &DynamicImage,
        resize: Resize,
    ) -> std::io::Result<ImageId> {
        match self {
            Backend::Halfblocks(p) => p.paint(screen, area, image, resize),
            Backend::Sixel(p) => p.paint(screen, area, image, resize),
            Backend::Kitty(p) => p.paint(screen, area, image, resize),
        }
    }

    fn forget<W: Write>(&mut self, screen: &mut Screen<W>, id: ImageId) -> std::io::Result<()> {
        match self {
            Backend::Halfblocks(p) => p.forget(screen, id),
            Backend::Sixel(p) => p.forget(screen, id),
            Backend::Kitty(p) => p.forget(screen, id),
        }
    }
}

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

    let mut backend = Backend::Halfblocks(Halfblocks::new());
    // Id of the most recent paint. Recorded so the host can forget
    // the cached state when leaving the active backend.
    let mut last_id;
    let mut quit = false;

    // Ask the terminal for its pixel dimensions. Some platforms
    // (notably Windows) don't fill xpixel/ypixel via the local
    // size syscall; the response arrives as a CellPixelSize /
    // WindowPixelSize event, which triggers a redraw below.
    screen.request_cell_pixel_size()?;
    screen.request_window_pixel_size()?;

    last_id = redraw(&mut screen, &image, &mut backend)?;
    screen.render()?;
    screen.flush()?;

    let mut events = Source::new(stdin())?;
    while !quit {
        let ev = events.read()?;
        screen.update_window_size(&ev);
        let mut switch_to: Option<Backend> = None;
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
            }) => switch_to = Some(Backend::Halfblocks(Halfblocks::new())),
            Event::KeyPress(Key {
                code: KeyCode::Char('2'),
                ..
            }) => switch_to = Some(Backend::Sixel(Sixel::new())),
            Event::KeyPress(Key {
                code: KeyCode::Char('3'),
                ..
            }) => switch_to = Some(Backend::Kitty(Kitty::new())),
            Event::Resize(ws) => {
                screen.resize(ws.col, ws.row);
                dirty = true;
            }
            Event::WindowPixelSize { .. } | Event::CellPixelSize { .. } => {
                dirty = true;
            }
            _ => {}
        }
        if let Some(next) = switch_to {
            backend.forget(&mut screen, last_id)?;
            backend = next;
            last_id = ImageId::NONE;
            dirty = true;
        }
        if dirty && !quit {
            last_id = redraw(&mut screen, &image, &mut backend)?;
            screen.render()?;
            screen.flush()?;
        }
    }

    backend.forget(&mut screen, last_id)?;
    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state)?;
    Ok(())
}

fn redraw<W: Write>(
    screen: &mut Screen<W>,
    image: &DynamicImage,
    backend: &mut Backend,
) -> std::io::Result<ImageId> {
    screen.clear();
    let w = screen.width();
    let h = screen.height();
    if w < 4 || h < 4 {
        return Ok(ImageId::NONE);
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
        return Ok(ImageId::NONE);
    }

    backend.paint(screen, area, image, Resize::default())
}
