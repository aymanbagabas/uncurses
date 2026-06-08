//! Image demo with a movable image, a typing surface, and a
//! cyclable image backend.
//!
//! Loads the image at `argv[1]` and paints it scaled to fit a fixed
//! 8×4 cell rectangle. The rectangle can be moved around the screen
//! with the arrow keys; mouse clicks place the cursor at the
//! clicked cell; typing writes characters at the cursor position.
//! `Alt-1` / `Alt-2` / `Alt-3` switch between the sixel, iTerm2,
//! and half-block backends respectively. `Ctrl-C` exits.
//!
//! Sixel and iTerm2 need the terminal to report a non-zero cell
//! pixel size; most modern terminals report it via `TIOCGWINSZ`
//! (`ws_xpixel` / `ws_ypixel`). The half-block backend always
//! works because it renders into the cell grid directly.

use std::io::Write;

use image::DynamicImage;
use uncurses::ansi::mode::{MouseEncoding, MouseMode};
use uncurses::color::BasicColor;
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, MouseButton, Source};
use uncurses::screen::{RegionId, Screen};
use uncurses::style::Style;
use uncurses::terminal::size::Winsize;
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses::text::WrapMode;
use uncurses_image::{HalfBlocks, Iterm2, Painter, Resize, Sixel};

const IMG_W: u16 = 8;
const IMG_H: u16 = 4;
const HEADER_ROWS: u16 = 2;

/// Available image backends, cycled with `Alt-1` / `Alt-2` /
/// `Alt-3`. The enum exists because [`Painter`] uses a generic
/// writer parameter and so isn't object-safe — host code that
/// wants runtime selection dispatches over a concrete enum.
enum Backend {
    Sixel(Sixel),
    Iterm2(Iterm2),
    HalfBlocks(HalfBlocks),
}

impl Backend {
    fn name(&self) -> &'static str {
        match self {
            Self::Sixel(_) => "sixel",
            Self::Iterm2(_) => "iterm2",
            Self::HalfBlocks(_) => "halfblocks",
        }
    }

    fn paint<W: Write>(
        &mut self,
        screen: &mut Screen<W>,
        id: RegionId,
        area: uncurses::Rect,
        image: &DynamicImage,
        resize: Resize,
        cell_px: (u16, u16),
    ) -> std::io::Result<()> {
        match self {
            Self::Sixel(p) => p.paint(screen, id, area, image, resize, cell_px),
            Self::Iterm2(p) => p.paint(screen, id, area, image, resize, cell_px),
            Self::HalfBlocks(p) => p.paint(screen, id, area, image, resize, cell_px),
        }
    }

    fn forget<W: Write>(&mut self, screen: &mut Screen<W>, id: RegionId) -> std::io::Result<()> {
        match self {
            Self::Sixel(p) => p.forget(screen, id),
            Self::Iterm2(p) => p.forget(screen, id),
            Self::HalfBlocks(p) => p.forget(screen, id),
        }
    }

    fn clear(&mut self) {
        match self {
            Self::Sixel(p) => p.clear(),
            Self::Iterm2(p) => p.clear(),
            Self::HalfBlocks(p) => p.clear(),
        }
    }
}

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

    let mut backend = Backend::Sixel(Sixel::new());
    let mut cell_px = cell_pixel_size(&ws);
    let mut win_px: (u16, u16) = (0, 0);
    let mut win_cells: (u16, u16) = (0, 0);
    let mut img_x: u16 = 1;
    let mut img_y: u16 = HEADER_ROWS;
    let mut cx: u16 = 0;
    let mut cy: u16 = HEADER_ROWS;
    let mut quit = false;

    let image_id = RegionId(1);
    redraw(
        &mut screen,
        &image,
        &mut backend,
        image_id,
        img_x,
        img_y,
        cell_px,
    )?;
    screen.set_cursor_position(cx, cy)?;
    screen.render()?;
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
            Event::KeyPress(Key {
                code: KeyCode::Char(digit @ ('1' | '2' | '3')),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::ALT) => {
                let next = match digit {
                    '1' => Backend::Sixel(Sixel::new()),
                    '2' => Backend::Iterm2(Iterm2::new()),
                    _ => Backend::HalfBlocks(HalfBlocks::new()),
                };
                if next.name() != backend.name() {
                    backend.forget(&mut screen, image_id)?;
                    backend = next;
                    dirty = true;
                }
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
                        && !key.modifiers.contains(KeyModifiers::ALT)
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
                backend.clear();
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
            redraw(
                &mut screen,
                &image,
                &mut backend,
                image_id,
                img_x,
                img_y,
                cell_px,
            )?;
        }
        if !quit {
            screen.render()?;
            screen.set_cursor_position(cx, cy)?;
            screen.flush()?;
        }
    }

    backend.forget(&mut screen, image_id)?;
    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state)?;
    Ok(())
}

fn redraw<W: Write>(
    screen: &mut Screen<W>,
    image: &DynamicImage,
    backend: &mut Backend,
    id: RegionId,
    img_x: u16,
    img_y: u16,
    cell_px: (u16, u16),
) -> std::io::Result<()> {
    let header_style = Style::EMPTY.with_fg(BasicColor::BrightBlack.into());
    let line1 = format!(
        "arrows: move | click: cursor | type to write | C-c: quit | cell_px {}x{}",
        cell_px.0, cell_px.1
    );
    let line2 = format!(
        "protocol: {}  |  alt-1 sixel  alt-2 iterm2  alt-3 halfblocks",
        backend.name()
    );
    let blank_line = " ".repeat(screen.width() as usize);
    screen.set_str_with(
        (0, 0),
        &blank_line,
        WrapMode::Truncate,
        header_style.clone(),
    );
    screen.set_str_with(
        (0, 1),
        &blank_line,
        WrapMode::Truncate,
        header_style.clone(),
    );
    screen.set_str_with((0, 0), &line1, WrapMode::Truncate, header_style.clone());
    screen.set_str_with((0, 1), &line2, WrapMode::Truncate, header_style);

    let area = uncurses::Rect {
        x: img_x,
        y: img_y,
        width: IMG_W,
        height: IMG_H,
    };
    backend.paint(screen, id, area, image, Resize::default(), cell_px)
}
