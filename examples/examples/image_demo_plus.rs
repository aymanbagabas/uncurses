//! Image-rendering demo with movable image and a typing surface.
//!
//! Loads the image at `argv[1]` and paints it scaled to fit a fixed
//! 5×3 cell rectangle. The rectangle can be moved around the screen
//! with the arrow keys; mouse clicks place the cursor at the
//! clicked cell; typing writes characters at the cursor position.
//!
//! Press `1` for half-blocks, `2` for sixel, `3` for the kitty
//! unicode-placeholder protocol. Ctrl-C exits.
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
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, MouseButton, Source};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses::text::WrapMode;
use uncurses_image::{Halfblocks, ImageId, Kitty, Painter, Resize, Sixel};

const IMG_W: u16 = 5;
const IMG_H: u16 = 3;
const HEADER_ROWS: u16 = 2;

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

fn clamp_image_position(screen_w: u16, screen_h: u16, x: u16, y: u16) -> (u16, u16) {
    let max_x = screen_w.saturating_sub(IMG_W);
    let max_y = screen_h.saturating_sub(IMG_H).max(HEADER_ROWS);
    let min_y = HEADER_ROWS;
    (x.min(max_x), y.clamp(min_y, max_y))
}

fn clamp_cursor(screen_w: u16, screen_h: u16, x: u16, y: u16) -> (u16, u16) {
    let max_x = screen_w.saturating_sub(1);
    let max_y = screen_h.saturating_sub(1);
    (x.min(max_x), y.min(max_y).max(HEADER_ROWS))
}

fn main() -> std::io::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: image_demo_plus <path-to-image>");
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
    screen.set_cursor_visible(true)?;
    screen.set_mouse_mode(MouseMode::Normal, MouseEncoding::Sgr)?;

    screen.request_cell_pixel_size()?;
    screen.request_window_pixel_size()?;

    let mut backend = Backend::Halfblocks(Halfblocks::new());
    let mut img_x: u16 = 1;
    let mut img_y: u16 = HEADER_ROWS;
    let mut cx: u16 = 0;
    let mut cy: u16 = HEADER_ROWS;
    let mut quit = false;

    let mut last_id = redraw(&mut screen, &image, &mut backend, img_x, img_y)?;
    screen.set_cursor_position(cx, cy)?;
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
                code: KeyCode::Char('c'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CTRL) => quit = true,
            Event::KeyPress(Key {
                code: KeyCode::Char('q'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CTRL) => {
                switch_to = Some(Backend::Halfblocks(Halfblocks::new()))
            }
            Event::KeyPress(Key {
                code: KeyCode::Char('w'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CTRL) => {
                switch_to = Some(Backend::Sixel(Sixel::new()))
            }
            Event::KeyPress(Key {
                code: KeyCode::Char('e'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CTRL) => {
                switch_to = Some(Backend::Kitty(Kitty::new()))
            }
            Event::KeyPress(key) | Event::KeyRepeat(key) => match key.code {
                KeyCode::Up => {
                    let (nx, ny) = clamp_image_position(
                        screen.width(),
                        screen.height(),
                        img_x,
                        img_y.saturating_sub(1),
                    );
                    if (nx, ny) != (img_x, img_y) {
                        img_x = nx;
                        img_y = ny;
                        dirty = true;
                    }
                }
                KeyCode::Down => {
                    let (nx, ny) = clamp_image_position(
                        screen.width(),
                        screen.height(),
                        img_x,
                        img_y.saturating_add(1),
                    );
                    if (nx, ny) != (img_x, img_y) {
                        img_x = nx;
                        img_y = ny;
                        dirty = true;
                    }
                }
                KeyCode::Left => {
                    let (nx, ny) = clamp_image_position(
                        screen.width(),
                        screen.height(),
                        img_x.saturating_sub(1),
                        img_y,
                    );
                    if (nx, ny) != (img_x, img_y) {
                        img_x = nx;
                        img_y = ny;
                        dirty = true;
                    }
                }
                KeyCode::Right => {
                    let (nx, ny) = clamp_image_position(
                        screen.width(),
                        screen.height(),
                        img_x.saturating_add(1),
                        img_y,
                    );
                    if (nx, ny) != (img_x, img_y) {
                        img_x = nx;
                        img_y = ny;
                        dirty = true;
                    }
                }
                KeyCode::Enter => {
                    let (nx, ny) =
                        clamp_cursor(screen.width(), screen.height(), 0, cy.saturating_add(1));
                    cx = nx;
                    cy = ny;
                }
                KeyCode::Backspace if cx > 0 => {
                    cx -= 1;
                    screen.set_str_with((cx, cy), " ", WrapMode::Truncate, Style::EMPTY);
                }
                _ => {
                    if let Some(text) = key.text.as_deref()
                        && !text.is_empty()
                    {
                        let end =
                            screen.set_str_with((cx, cy), text, WrapMode::Truncate, Style::EMPTY);
                        let (nx, ny) = clamp_cursor(screen.width(), screen.height(), end.x, end.y);
                        cx = nx;
                        cy = ny;
                    }
                }
            },
            Event::MouseClick(m) if m.button == MouseButton::Left => {
                let (nx, ny) = clamp_cursor(screen.width(), screen.height(), m.x, m.y);
                cx = nx;
                cy = ny;
            }
            Event::Resize(ws) => {
                screen.resize(ws.col, ws.row);
                let (nx, ny) = clamp_image_position(ws.col, ws.row, img_x, img_y);
                img_x = nx;
                img_y = ny;
                let (cnx, cny) = clamp_cursor(ws.col, ws.row, cx, cy);
                cx = cnx;
                cy = cny;
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
            last_id = redraw(&mut screen, &image, &mut backend, img_x, img_y)?;
        }
        if !quit {
            screen.set_cursor_position(cx, cy)?;
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
    img_x: u16,
    img_y: u16,
) -> std::io::Result<ImageId> {
    screen.clear();
    let w = screen.width();

    let header = format!(
        "arrows: move image | click: place cursor | type to write | C-q/w/e: backend ({}) | C-c: quit",
        backend.label()
    );
    screen.set_str_with(
        (0, 0),
        &header,
        WrapMode::Truncate,
        Style::EMPTY.with_fg(BasicColor::BrightBlack.into()),
    );
    let _ = w;

    let area = uncurses::Rect {
        x: img_x,
        y: img_y,
        width: IMG_W,
        height: IMG_H,
    };
    backend.paint(screen, area, image, Resize::default())
}
