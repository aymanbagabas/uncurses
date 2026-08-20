//! Truecolor gradient with a mouse-driven color inspector.
//!
//! Fills the window with a smooth 2D color field. Each character cell is split
//! into two side-by-side sub-pixels using the left-half block `▌`: its
//! foreground paints the left half and its background paints the right half, so
//! one column of cells carries two columns of color and the horizontal gradient
//! looks twice as smooth. Colors come from [`Color::hsl`] and the renderer
//! downsamples them to whatever the terminal's color
//! [`Profile`](uncurses::color::Profile) allows: exact on a true-color
//! terminal, quantized to 256 or 16 colors elsewhere, dropped entirely with no
//! color. Same code, every terminal.
//!
//! Hover shows the live pointer cell in a hint bar; **click** to open a
//! floating color panel (swatch, hex, RGB, HSL), like an image editor's info
//! readout. The panel prefers the cursor's bottom-right and flips to stay on
//! screen. Press `space` to toggle it back to the hint bar.
//!
//! When the terminal reports SGR-pixel mouse support
//! ([`Capabilities::mouse_sgr_pixel`](uncurses::screen::Capabilities)), the
//! example enables pixel-accurate tracking and resolves *which half* of a cell
//! the pointer is over, reading the exact sub-pixel color. Otherwise it
//! degrades seamlessly to cell coordinates and reads the cell's left
//! sub-pixel. No capability probing is done by hand — the screen reports what
//! it negotiated and the example adapts.
//!
//! Run with `cargo run --example gradient`. Resize to watch it reflow;
//! press `q` or `Ctrl-C` to quit.

use uncurses::buffer::Bounded;
use uncurses::cell::Cell;
use uncurses::color::Color;
use uncurses::event::{Event, Key, Mouse};
use uncurses::program::{MouseTracking, Program, ProgramOptions};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

/// Saturation of the gradient field, shared by the render and the readout.
const SATURATION: f32 = 0.9;
/// Floating panel dimensions.
const BOX_W: u16 = 22;
const BOX_H: u16 = 5;

struct State {
    pointer: Option<Mouse>,
    show_box: bool,
}

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    // Request motion tracking and pixel-accurate coordinates. The screen only
    // turns pixels on when the terminal actually supports SGR-pixel encoding,
    // so this degrades to cell coordinates on its own.
    program.init_with(ProgramOptions {
        mouse: Some(MouseTracking::MOTION | MouseTracking::PIXELS),
        ..ProgramOptions::default()
    })?;
    program.enter_alt_screen()?;
    program.hide_cursor()?;
    // `init` probes nothing, so ask for the capabilities this example needs.
    // The replies arrive as ordinary events; the run loop feeds every one back
    // through `observe_event`, and `resolve` reads `mouse_sgr_pixel` live once
    // the terminal has answered.
    program.query_capabilities(&[])?;
    // Seed the pixel size now, and refresh it on resize.
    program.request_window_pixel_size()?;

    let result = run(&mut program);
    program.finish()?;
    result
}

fn run(program: &mut Program<Stdin, Stdout>) -> std::io::Result<()> {
    let quit: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
    let space: Key = "space".parse().unwrap();
    let mut state = State {
        pointer: None,
        show_box: false,
    };
    render(program, &state);

    loop {
        let ev = program.read_event()?;
        match ev {
            Event::KeyPress(ref k) if quit.contains(k) => break,
            Event::KeyPress(ref k) if *k == space => state.show_box = !state.show_box,
            Event::MouseMove(m) => state.pointer = Some(m),
            // The first click switches from the hint bar to the inspector.
            Event::MouseClick(m) => {
                state.pointer = Some(m);
                state.show_box = true;
            }
            Event::Resize(ws) => {
                program.screen_mut().resize((ws.col, ws.row));
                // The cell↔pixel ratio changed; refresh the pixel size so the
                // converter stays accurate.
                program.request_window_pixel_size()?;
            }
            _ => continue,
        }
        render(program, &state);
    }
    Ok(())
}

fn render(program: &mut Program<Stdin, Stdout>, state: &State) {
    let w = program.screen().width();
    let h = program.screen().height();
    if w == 0 || h == 0 {
        return;
    }

    draw_gradient(program.screen_mut(), w, h);

    // The panel and the hint bar are mutually exclusive: when the panel is up,
    // it stands in for the hint.
    let mut panel_shown = false;
    if state.show_box
        && let Some((cx, cy, sub)) = resolve(program, state)
        && cx < w
        && cy < h
        && let Some((bx, by)) = place_box(cx, cy, w, h)
    {
        let color = color_at(sub.min(w * 2 - 1), cy, w, h);
        draw_info_box(program.screen_mut(), bx, by, color);
        panel_shown = true;
    }
    if !panel_shown {
        // While the inspector is off, the hint bar reports the live pointer
        // cell (resolved from pixels when available, raw otherwise).
        let coords = state.pointer.map(|m| {
            resolve(program, state)
                .map(|(cx, cy, _)| (cx, cy))
                .unwrap_or((m.x, m.y))
        });
        let where_ = match coords {
            Some((x, y)) => format!("({x}, {y})"),
            None => "(–, –)".to_string(),
        };
        let label = Style::default()
            .bold()
            .fg(Color::Black)
            .bg(Color::BrightWhite);
        program.screen_mut().set_str(
            (2, 1),
            &format!(" gradient {where_} — click: inspect · space: toggle · q: quit "),
            label,
        );
    }

    let _ = program.screen_mut().render();
}

/// Paint the half-block color field. Each cell is a left-half block `▌`: its
/// foreground is the left sub-pixel and its background the right one, so a grid
/// of `w` cells spans `2 * w` color columns. Hue sweeps across the (doubled)
/// columns; lightness sweeps down the rows.
fn draw_gradient(screen: &mut Screen<Stdout>, w: u16, h: u16) {
    for y in 0..h {
        for x in 0..w {
            let left = color_at(x * 2, y, w, h);
            let right = color_at(x * 2 + 1, y, w, h);
            let cell = Cell::narrow("▌").style(Style::default().fg(left).bg(right));
            screen.set_cell((x, y), &cell);
        }
    }
}

/// Color of sub-pixel column `sub` on row `y`, given the cell grid size.
fn color_at(sub: u16, y: u16, w: u16, h: u16) -> Color {
    let cols = u32::from(w) * 2;
    let hue = f32::from(sub) / cols.max(1) as f32 * 360.0;
    let light = 0.65 - (f32::from(y) / f32::from(h.max(1))) * 0.4;
    Color::hsl(hue, SATURATION, light)
}

/// Resolve the pointer to a `(cell_x, cell_y, sub_pixel_column)`.
///
/// With pixel-accurate mouse the raw event is in pixels. The screen's converter
/// floors it to a cell; the cell's *pixel* width — `window_pixels / window_cells`
/// — then tells which half of that cell the pointer sits in, selecting the left
/// or right sub-pixel column. Without pixel mouse the event is already a cell,
/// so the inspector falls back to the cell's left sub-pixel. Returns `None`
/// while pixel mouse is on but the pixel size is not known yet.
fn resolve(program: &Program<Stdin, Stdout>, state: &State) -> Option<(u16, u16, u16)> {
    let m = state.pointer?;
    // Read the capability live: it is detected asynchronously after init, so a
    // value cached at startup would be wrong. Mouse events only arrive once
    // tracking is enabled (which happens after detection), so by the time we
    // get here the capability reflects the encoding actually in use.
    if !program.capabilities().mouse_sgr_pixel {
        return Some((m.x, m.y, m.x.saturating_mul(2)));
    }
    let cell = program.mouse_pixels_to_cells(m)?;
    let pixels = program.window_pixels()?;
    let cells = program
        .window_cells()
        .unwrap_or_else(|| program.screen().size());
    // Cell width in pixels, then the pointer's offset within its cell.
    let cell_w = (pixels.width / cells.width.max(1)).max(1);
    let within = m.x.saturating_sub(cell.x * cell_w);
    let right = within >= cell_w / 2;
    let sub = cell.x * 2 + u16::from(right);
    Some((cell.x, cell.y, sub))
}

/// Place the panel near `(px, py)`, preferring the cursor's bottom-right and
/// flipping to the opposite side on each axis when it would fall off screen.
/// Returns `None` if the window is too small to hold the panel.
fn place_box(px: u16, py: u16, w: u16, h: u16) -> Option<(u16, u16)> {
    if w < BOX_W || h < BOX_H {
        return None;
    }
    let bx = if px + 2 + BOX_W <= w {
        px + 2
    } else if px > BOX_W {
        px - BOX_W - 1
    } else {
        w - BOX_W
    };
    let by = if py + 1 + BOX_H <= h {
        py + 1
    } else if py >= BOX_H {
        py - BOX_H
    } else {
        h - BOX_H
    };
    Some((bx, by))
}

/// Draw the floating color-info panel at `(bx, by)` for `color`.
fn draw_info_box(screen: &mut Screen<Stdout>, bx: u16, by: u16, color: Color) {
    let (r, g, b) = color.to_rgb();
    let (hue, sat, light) = color.to_hsl();

    let bg = Color::Rgb(24, 24, 32);
    let panel = Style::default().fg(Color::BrightWhite).bg(bg);
    let border = Style::default().fg(Color::BrightBlack).bg(bg);
    let dim = Style::default().fg(Color::BrightBlack).bg(bg);

    let span = usize::from(BOX_W - 2);
    screen.set_str((bx, by), &format!("╭{}╮", "─".repeat(span)), border.clone());
    screen.set_str(
        (bx, by + BOX_H - 1),
        &format!("╰{}╯", "─".repeat(span)),
        border.clone(),
    );
    for row in 1..BOX_H - 1 {
        screen.set_str((bx, by + row), "│", border.clone());
        screen.set_str((bx + 1, by + row), &" ".repeat(span), panel.clone());
        screen.set_str((bx + BOX_W - 1, by + row), "│", border.clone());
    }

    let cx = bx + 2;
    // Solid color swatch from the background color.
    screen.set_str((cx, by + 1), "    ", Style::default().bg(color));
    screen.set_str((cx + 5, by + 1), &color.to_hex(), panel.clone());
    screen.set_str(
        (cx, by + 2),
        &format!("rgb {r:>3} {g:>3} {b:>3}"),
        dim.clone(),
    );
    screen.set_str(
        (cx, by + 3),
        &format!(
            "hsl {:>3.0}° {:>3.0}% {:>3.0}%",
            hue,
            sat * 100.0,
            light * 100.0
        ),
        dim,
    );
}
