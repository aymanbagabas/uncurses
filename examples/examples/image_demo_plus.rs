//! Sixel image demo with a movable image and a typing surface.
//!
//! Loads the image at `argv[1]` and paints it scaled to fit a fixed
//! 8×4 cell rectangle. The rectangle can be moved around the screen
//! with the arrow keys; mouse clicks place the cursor at the
//! clicked cell; typing writes characters at the cursor position.
//! `Ctrl-C` exits.
//!
//! The terminal must report a non-zero cell pixel size for sixel
//! output to render. Most modern terminals report this via
//! `TIOCGWINSZ` (`ws_xpixel`/`ws_ypixel`).

use std::io::Write;

use image::DynamicImage;
use uncurses::ansi::mode::{MouseEncoding, MouseMode};
use uncurses::color::BasicColor;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, MouseButton, Source};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::size::Winsize;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses::text::WrapMode;
use uncurses_image::{ImageId, Painter, Resize, Sixel};

const IMG_W: u16 = 8;
const IMG_H: u16 = 4;
const HEADER_ROWS: u16 = 2;

fn cell_pixel_size(ws: &Winsize) -> (u16, u16) {
    if ws.col == 0 || ws.row == 0 || ws.xpixel == 0 || ws.ypixel == 0 {
        return (0, 0);
    }
    (ws.xpixel / ws.col, ws.ypixel / ws.row)
}

fn derive_cell_px(win_px: (u16, u16), win_cells: (u16, u16)) -> Option<(u16, u16)> {
    if win_px.0 == 0 || win_px.1 == 0 || win_cells.0 == 0 || win_cells.1 == 0 {
        return None;
    }
    Some((win_px.0 / win_cells.0, win_px.1 / win_cells.1))
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
    let mut ws = get_window_size(stdout()).unwrap_or_default();

    let mut screen = Screen::new(stdout()).with_size(ws.col, ws.row);
    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(true)?;
    screen.set_mouse_mode(MouseMode::Normal, MouseEncoding::Sgr)?;
    screen.request_cell_pixel_size()?;
    screen.request_window_pixel_size()?;
    screen.request_text_area_size()?;
    screen.flush()?;

    let mut painter = Sixel::new();
    let mut cell_px = cell_pixel_size(&ws);
    let mut win_px: (u16, u16) = (0, 0);
    let mut win_cells: (u16, u16) = (0, 0);
    let mut img_x: u16 = 1;
    let mut img_y: u16 = HEADER_ROWS;
    let mut cx: u16 = 0;
    let mut cy: u16 = HEADER_ROWS;
    let mut quit = false;

    let mut last_id = redraw(&mut screen, &image, &mut painter, img_x, img_y, cell_px)?;
    screen.set_cursor_position(cx, cy)?;
    screen.render()?;
    painter.draw(&mut screen)?;
    screen.flush()?;

    let mut events = Source::new(stdin())?;
    while !quit {
        let ev = events.read()?;
        let mut dirty = false;
        match ev {
            Event::KeyPress(Key {
                code: KeyCode::Char('c'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CTRL) => quit = true,
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
            Event::Resize(new_ws) => {
                ws = new_ws;
                screen.resize(ws.col, ws.row);
                let from_winsize = cell_pixel_size(&ws);
                if from_winsize != (0, 0) {
                    cell_px = from_winsize;
                }
                let (nx, ny) = clamp_image_position(ws.col, ws.row, img_x, img_y);
                img_x = nx;
                img_y = ny;
                let (cnx, cny) = clamp_cursor(ws.col, ws.row, cx, cy);
                cx = cnx;
                cy = cny;
                painter.clear();
                dirty = true;
            }
            Event::CellPixelSize { width, height } if width != 0 && height != 0 => {
                cell_px = (width, height);
                dirty = true;
            }
            Event::WindowPixelSize { width, height } => {
                win_px = (width, height);
                if let Some(derived) = derive_cell_px(win_px, win_cells)
                    && cell_px == (0, 0)
                {
                    cell_px = derived;
                    dirty = true;
                }
            }
            Event::WindowCellSize { width, height } => {
                win_cells = (width, height);
                if let Some(derived) = derive_cell_px(win_px, win_cells)
                    && cell_px == (0, 0)
                {
                    cell_px = derived;
                    dirty = true;
                }
            }
            _ => {}
        }
        if dirty && !quit {
            last_id = redraw(&mut screen, &image, &mut painter, img_x, img_y, cell_px)?;
        }
        if !quit {
            screen.render()?;
            painter.draw(&mut screen)?;
            screen.set_cursor_position(cx, cy)?;
            screen.flush()?;
        }
    }

    painter.forget(&mut screen, last_id)?;
    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state)?;
    Ok(())
}

fn redraw<W: Write>(
    screen: &mut Screen<W>,
    image: &DynamicImage,
    painter: &mut Sixel,
    img_x: u16,
    img_y: u16,
    cell_px: (u16, u16),
) -> std::io::Result<ImageId> {
    let header = format!(
        "arrows: move image | click: place cursor | type to write | C-c: quit | cell_px {}x{}",
        cell_px.0, cell_px.1
    );
    let header_style = Style::EMPTY.with_fg(BasicColor::BrightBlack.into());
    screen.set_str_with((0, 0), &header, WrapMode::Truncate, header_style);

    let area = uncurses::Rect {
        x: img_x,
        y: img_y,
        width: IMG_W,
        height: IMG_H,
    };
    painter.paint(screen, area, image, Resize::default(), cell_px)
}
