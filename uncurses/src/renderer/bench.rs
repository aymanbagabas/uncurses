//! Renderer micro-benchmarks (nightly only).
//!
//! These run against the crate-private renderer internals through the
//! built-in libtest harness, so `Renderer` and `RenderBuffer` never need to
//! be exposed outside the crate. Build and run them with:
//!
//! ```sh
//! RUSTFLAGS="--cfg uncurses_bench" cargo +nightly bench
//! ```
//!
//! On a stable toolchain this module is not compiled at all, so it has no
//! effect on normal builds, tests, or downstream consumers.
//!
//! ## What the groups measure
//!
//! Frame cost is driven by three roughly independent things, so the suite
//! varies one at a time:
//!
//! - **How much changed.** From nothing at all through one cell, one row, a
//!   scattered tenth, up to every cell. Real applications live at the low
//!   end, so those cases matter more than the full repaint.
//! - **What the content is.** ASCII, wide CJK, and multi-scalar clusters
//!   take different paths: only a cluster reaches the arena's table.
//! - **How the style moves.** One style for the frame, styled runs, or a
//!   distinct style per cell. This separates SGR emission from glyph
//!   output.
//!
//! A second grid size runs the headline cases so the numbers can be read
//! for scaling rather than as a single point.
//!
//! Note the difference between `untouched_frame_early_out` and
//! `touched_frame_no_diff`: the first measures the "nothing was drawn"
//! check, which never looks at a cell, while the second dirties every row
//! and makes the renderer prove cell by cell that there is nothing to emit.

extern crate test;

use test::{Bencher, black_box};

use crate::color::Color;
use crate::cell::Cell;
use crate::renderer::{RenderBuffer, Renderer};
use crate::style::Style;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

/// A second size, roughly a maximised window, for reading scaling.
const BIG_WIDTH: u16 = 200;
const BIG_HEIGHT: u16 = 50;

/// Deterministic glyph for a cell, so two frames built with different
/// offsets differ everywhere.
fn glyph(x: u16, y: u16, offset: u8) -> char {
    char::from(b'a' + ((x as u8).wrapping_add(y as u8).wrapping_add(offset) % 26))
}

fn filled(w: u16, h: u16, offset: u8) -> RenderBuffer {
    let mut buf = RenderBuffer::new(w, h);
    for y in 0..h {
        for x in 0..w {
            buf.set_ref((x, y), &Cell::narrow(glyph(x, y, offset)));
        }
    }
    buf
}

fn filled_buffer(offset: u8) -> RenderBuffer {
    filled(WIDTH, HEIGHT, offset)
}

fn shifted_up_buffer() -> RenderBuffer {
    let mut buf = RenderBuffer::new(WIDTH, HEIGHT);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let source_y = (y + 1) % HEIGHT;
            buf.set_ref((x, y), &Cell::narrow(glyph(x, source_y, 0)));
        }
    }
    buf
}

/// Every cell carries a distinct truecolor style, so the pen changes on
/// every cell and SGR emission dominates.
fn style_churn_buffer(offset: u8) -> RenderBuffer {
    let mut buf = RenderBuffer::new(WIDTH, HEIGHT);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let style = Style::default().fg(Color::Rgb(x as u8, y as u8, offset));
            buf.set_ref((x, y), &Cell::narrow(glyph(x, y, offset)).with_style(style));
        }
    }
    buf
}

/// Styled runs of eight cells, which is closer to how real text is
/// coloured than either a uniform frame or per-cell churn.
fn styled_runs_buffer(offset: u8) -> RenderBuffer {
    let mut buf = RenderBuffer::new(WIDTH, HEIGHT);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let style = Style::default().fg(Color::Indexed(((x / 8 + y) % 16) as u8));
            buf.set_ref((x, y), &Cell::narrow(glyph(x, y, offset)).with_style(style));
        }
    }
    buf
}

/// Wide CJK primaries. Each is a single scalar, so it encodes inline on
/// the emit path but costs two columns.
fn cjk_buffer(offset: u8) -> RenderBuffer {
    let mut buf = RenderBuffer::new(WIDTH, HEIGHT);
    for y in 0..HEIGHT {
        let mut x = 0;
        while x + 1 < WIDTH {
            let ch = char::from_u32(0x4E00 + ((x as u32 + y as u32 + offset as u32) % 512))
                .unwrap_or('中');
            buf.set_ref((x, y), &Cell::wide(ch));
            x += 2;
        }
    }
    buf
}

/// Multi-scalar grapheme clusters: the only content that reaches the
/// arena's cluster table on the emit path.
fn cluster_buffer(offset: u8) -> RenderBuffer {
    use crate::buffer::SurfaceMut;
    use crate::cell::Cell;

    let mut buf = RenderBuffer::new(WIDTH, HEIGHT);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            // A base letter plus a combining acute: two scalars, so the
            // arena has to intern it.
            let text = format!("{}\u{0301}", glyph(x, y, offset));
            SurfaceMut::set_cell(&mut buf, (x, y).into(), &Cell::from(text));
        }
    }
    buf
}

fn prime(renderer: &mut Renderer, buf: &mut RenderBuffer, out: &mut Vec<u8>) {
    renderer.render(out, buf).unwrap();
    out.clear();
}

/// Run a swap-and-render loop between two prepared frames, optionally
/// mutating the renderer before each render (e.g. to force a clear).
fn bench_swap_render(
    b: &mut Bencher,
    mut renderer: Renderer,
    mut current: RenderBuffer,
    mut next: RenderBuffer,
    mut before_render: impl FnMut(&mut Renderer),
) {
    let mut out = Vec::with_capacity(64 * 1024);
    prime(&mut renderer, &mut current, &mut out);

    b.iter(|| {
        std::mem::swap(black_box(&mut current), black_box(&mut next));
        out.clear();
        before_render(black_box(&mut renderer));
        black_box(renderer.render(black_box(&mut out), black_box(&mut current))).unwrap();
        black_box(&out);
    });
}

/// Two frames built by `make`, primed and swapped every iteration.
fn bench_pair(b: &mut Bencher, make: impl Fn(u8) -> RenderBuffer) {
    bench_swap_render(b, Renderer::new(), make(0), make(1), |_| {});
}

// ---------------------------------------------------------------------
// How much changed
// ---------------------------------------------------------------------

/// No row is dirty, so the renderer returns without inspecting a cell.
/// This is the per-frame floor an idle application pays.
#[bench]
fn untouched_frame_early_out(b: &mut Bencher) {
    let mut first = filled_buffer(0);
    let mut second = first.clone();
    first.clear_touched();
    second.clear_touched();

    bench_swap_render(b, Renderer::new(), first, second, |_| {});
}

/// Every row is dirty but no cell actually differs, so the renderer walks
/// the whole grid and emits nothing. This is what a naive redraw of an
/// unchanged screen costs.
#[bench]
fn touched_frame_no_diff(b: &mut Bencher) {
    let mut first = filled_buffer(0);
    let mut second = first.clone();
    // `render` reads these flags but does not consume them, so every
    // iteration walks the full grid.
    first.touch_all();
    second.touch_all();

    bench_swap_render(b, Renderer::new(), first, second, |_| {});
}

#[bench]
fn single_cell_change(b: &mut Bencher) {
    let mut first = filled_buffer(0);
    let mut second = first.clone();
    first.clear_touched();
    second.clear_touched();
    first.set_ref((WIDTH / 2, HEIGHT / 2), &Cell::narrow('0'));
    second.set_ref((WIDTH / 2, HEIGHT / 2), &Cell::narrow('1'));

    bench_swap_render(b, Renderer::new(), first, second, |_| {});
}

/// One full row repainted, the shape of a status line or a progress bar.
#[bench]
fn single_line_change(b: &mut Bencher) {
    let mut first = filled_buffer(0);
    let mut second = first.clone();
    first.clear_touched();
    second.clear_touched();
    for x in 0..WIDTH {
        first.set_ref((x, HEIGHT / 2), &Cell::narrow(glyph(x, 0, 0)));
        second.set_ref((x, HEIGHT / 2), &Cell::narrow(glyph(x, 0, 1)));
    }

    bench_swap_render(b, Renderer::new(), first, second, |_| {});
}

/// Roughly a tenth of the grid changed, scattered across every row: the
/// shape of a cursor move plus a few updated fields.
///
/// Compare against `contiguous_tenth_change`, which changes the same number
/// of cells in one run per row, and `wide_span_two_changes`, which touches
/// the same column span while changing almost nothing. Between them the
/// three separate the cost of scanning a row from the cost of hopping
/// along it.
#[bench]
fn scattered_tenth_change(b: &mut Bencher) {
    let mut first = filled_buffer(0);
    let mut second = first.clone();
    first.clear_touched();
    second.clear_touched();
    for y in 0..HEIGHT {
        let mut x = y % 10;
        while x < WIDTH {
            first.set_ref((x, y), &Cell::narrow('0'));
            second.set_ref((x, y), &Cell::narrow('1'));
            x += 10;
        }
    }

    bench_swap_render(b, Renderer::new(), first, second, |_| {});
}

/// The same number of changed cells as `scattered_tenth_change`, but in one
/// run per row, so the cursor streams instead of hopping.
#[bench]
fn contiguous_tenth_change(b: &mut Bencher) {
    let mut first = filled_buffer(0);
    let mut second = first.clone();
    first.clear_touched();
    second.clear_touched();
    for y in 0..HEIGHT {
        for x in 0..WIDTH / 10 {
            first.set_ref((x, y), &Cell::narrow('0'));
            second.set_ref((x, y), &Cell::narrow('1'));
        }
    }

    bench_swap_render(b, Renderer::new(), first, second, |_| {});
}

/// Only the first and last cell of each row differ, which leaves the row's
/// touched span covering the full width. Isolates the cost of scanning a
/// span from the cost of emitting within it.
#[bench]
fn wide_span_two_changes(b: &mut Bencher) {
    let mut first = filled_buffer(0);
    let mut second = first.clone();
    first.clear_touched();
    second.clear_touched();
    for y in 0..HEIGHT {
        for x in [0, WIDTH - 1] {
            first.set_ref((x, y), &Cell::narrow('0'));
            second.set_ref((x, y), &Cell::narrow('1'));
        }
    }

    bench_swap_render(b, Renderer::new(), first, second, |_| {});
}

#[bench]
fn full_frame_all_cells_changed(b: &mut Bencher) {
    bench_pair(b, filled_buffer);
}

#[bench]
fn scroll_shift_up_by_1(b: &mut Bencher) {
    bench_swap_render(
        b,
        Renderer::new(),
        filled_buffer(0),
        shifted_up_buffer(),
        |_| {},
    );
}

#[bench]
fn force_clear_frame(b: &mut Bencher) {
    bench_swap_render(
        b,
        Renderer::new(),
        filled_buffer(0),
        filled_buffer(0),
        |renderer| renderer.request_clear(),
    );
}

// ---------------------------------------------------------------------
// What the content is
// ---------------------------------------------------------------------

#[bench]
fn full_frame_cjk(b: &mut Bencher) {
    bench_pair(b, cjk_buffer);
}

/// Every cell carries the same OSC 8 hyperlink, which is what a frame of
/// linked text looks like. Interning a link is the most expensive thing the
/// arena does, so this is where reuse pays most.
fn hyperlink_buffer(offset: u8) -> RenderBuffer {
    use crate::buffer::SurfaceMut;
    use crate::cell::{Cell, Style as CellStyle};

    let mut buf = RenderBuffer::new(WIDTH, HEIGHT);
    let style = CellStyle::new().link("https://example.com/some/path", "id=1");
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let cell = Cell::new(glyph(x, y, offset), style.clone());
            SurfaceMut::set_cell(&mut buf, crate::layout::Position::new(x, y), &cell);
        }
    }
    buf
}

#[bench]
fn full_frame_hyperlinks(b: &mut Bencher) {
    bench_pair(b, hyperlink_buffer);
}

#[bench]
fn full_frame_clusters(b: &mut Bencher) {
    bench_pair(b, cluster_buffer);
}

// ---------------------------------------------------------------------
// How the style moves
// ---------------------------------------------------------------------

#[bench]
fn full_frame_styled_runs(b: &mut Bencher) {
    bench_pair(b, styled_runs_buffer);
}

#[bench]
fn full_frame_style_churn(b: &mut Bencher) {
    bench_pair(b, style_churn_buffer);
}

// ---------------------------------------------------------------------
// Scaling: the same shapes on a 200x50 grid (5.2x the cells)
// ---------------------------------------------------------------------

#[bench]
fn big_untouched_frame_early_out(b: &mut Bencher) {
    let mut first = filled(BIG_WIDTH, BIG_HEIGHT, 0);
    let mut second = first.clone();
    first.clear_touched();
    second.clear_touched();

    bench_swap_render(b, Renderer::new(), first, second, |_| {});
}

#[bench]
fn big_single_cell_change(b: &mut Bencher) {
    let mut first = filled(BIG_WIDTH, BIG_HEIGHT, 0);
    let mut second = first.clone();
    first.clear_touched();
    second.clear_touched();
    first.set_ref((BIG_WIDTH / 2, BIG_HEIGHT / 2), &Cell::narrow('0'));
    second.set_ref((BIG_WIDTH / 2, BIG_HEIGHT / 2), &Cell::narrow('1'));

    bench_swap_render(b, Renderer::new(), first, second, |_| {});
}

#[bench]
fn big_full_frame_all_cells_changed(b: &mut Bencher) {
    bench_swap_render(
        b,
        Renderer::new(),
        filled(BIG_WIDTH, BIG_HEIGHT, 0),
        filled(BIG_WIDTH, BIG_HEIGHT, 1),
        |_| {},
    );
}

// ---------------------------------------------------------------------
// The whole frame: draw, render, flush
//
// Everything above measures `Renderer::render` alone, which turns a
// prepared grid into bytes. An application also has to *draw* the grid and
// *flush* the bytes, and those two are where its own time goes. These
// benchmarks cover the round trip so a frame budget can be read directly:
// divide 1s by the per-frame figure for a ceiling in frames per second.
// ---------------------------------------------------------------------

use crate::cell::Cell;
use crate::screen::Screen;
use crate::text::TextSurface;

/// A writer that keeps nothing, so flushing measures the staging drain and
/// not an allocator or a syscall.
struct Sink;

impl std::io::Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(black_box(buf).len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn screen() -> Screen<Sink> {
    let mut s = Screen::new(Sink, (WIDTH, HEIGHT));
    s.set_color_profile(crate::color::Profile::TrueColor);
    s
}

/// Two frames' worth of row text, built once so the benchmarks below
/// measure painting rather than string formatting.
fn rows(offset: u8) -> Vec<String> {
    (0..HEIGHT)
        .map(|y| (0..WIDTH).map(|x| glyph(x, y, offset)).collect())
        .collect()
}

/// Paint every row with `set_str`, the way an application draws text.
///
/// This is the one path where a style is interned once per call and reused
/// for the whole run, so it is the check on whether the fat-cell boundary
/// costs anything on the draw side.
fn paint(s: &mut Screen<Sink>, rows: &[String], tint: u8) {
    for (y, text) in rows.iter().enumerate() {
        let style = Style::default().fg(Color::Indexed(((y as u16 + tint as u16) % 16) as u8));
        s.set_str((0u16, y as u16), text, style);
    }
}

/// Drawing only: `set_str` over the whole grid, no render, no flush.
#[bench]
fn draw_frame_set_str(b: &mut Bencher) {
    let mut s = screen();
    let (a, c) = (rows(0), rows(1));
    let mut flip = false;
    b.iter(|| {
        flip = !flip;
        paint(black_box(&mut s), if flip { &a } else { &c }, flip as u8);
    });
}

/// Drawing where every cell carries a distinct truecolor style, which is
/// what a gradient or a pixel-art renderer does (see the `space` examples).
///
/// Nothing repeats, so the style memo never hits and every cell reaches the
/// arena. This is the worst case for interned storage: an id is minted and
/// then never reused.
#[bench]
fn draw_frame_set_cell_churn(b: &mut Bencher) {
    let mut s = screen();
    let mut offset = 0u8;
    b.iter(|| {
        offset = offset.wrapping_add(1);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let style = Style::default()
                    .fg(Color::Rgb(x as u8, y as u8, offset))
                    .bg(Color::Rgb(offset, x as u8, y as u8));
                let cell = Cell::new(glyph(x, y, offset), style);
                s.set_cell(crate::layout::Position::new(x, y), black_box(&cell));
            }
        }
    });
}

/// Drawing linked cells one by one. Interning a hyperlink builds an owned
/// `Link` just to probe the table, so a miss costs two allocations on top
/// of the lock -- this is what the per-buffer link memo is for.
#[bench]
fn draw_frame_set_cell_linked(b: &mut Bencher) {
    use crate::cell::Style as CellStyle;

    let mut s = screen();
    let style = CellStyle::new().link("https://example.com/some/path", "id=1");
    let mut offset = 0u8;
    b.iter(|| {
        offset = offset.wrapping_add(1);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let cell = Cell::new(glyph(x, y, offset), style.clone());
                s.set_cell(crate::layout::Position::new(x, y), black_box(&cell));
            }
        }
    });
}

/// Drawing cell by cell instead of by run, which is what a widget that
/// positions each glyph itself does.
#[bench]
fn draw_frame_set_cell(b: &mut Bencher) {
    let mut s = screen();
    let mut offset = 0u8;
    b.iter(|| {
        offset = offset.wrapping_add(1);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let cell = Cell::new(glyph(x, y, offset), Style::default());
                s.set_cell(crate::layout::Position::new(x, y), black_box(&cell));
            }
        }
    });
}

/// Draw plus render, with no flush, so subtracting this from
/// `frame_loop_full_repaint` gives the cost of draining to the writer.
#[bench]
fn frame_repaint_without_flush(b: &mut Bencher) {
    let mut s = screen();
    let (a, c) = (rows(0), rows(1));
    let mut flip = false;
    paint(&mut s, &a, 0);
    s.render().unwrap();
    std::io::Write::flush(&mut s).unwrap();

    b.iter(|| {
        flip = !flip;
        paint(black_box(&mut s), if flip { &a } else { &c }, flip as u8);
        black_box(s.render()).unwrap();
    });
}

/// A whole frame with every cell changed: the worst case an application can
/// hand the renderer.
#[bench]
fn frame_loop_full_repaint(b: &mut Bencher) {
    let mut s = screen();
    let (a, c) = (rows(0), rows(1));
    let mut flip = false;
    paint(&mut s, &a, 0);
    s.render().unwrap();
    std::io::Write::flush(&mut s).unwrap();

    b.iter(|| {
        flip = !flip;
        paint(black_box(&mut s), if flip { &a } else { &c }, flip as u8);
        black_box(s.render()).unwrap();
        std::io::Write::flush(&mut s).unwrap();
    });
}

/// A whole frame with one line changed, which is closer to what an idle
/// application redrawing a status line actually costs.
#[bench]
fn frame_loop_one_line(b: &mut Bencher) {
    let mut s = screen();
    let a = rows(0);
    let line = rows(1).remove(0);
    let mut flip = false;
    paint(&mut s, &a, 0);
    s.render().unwrap();
    std::io::Write::flush(&mut s).unwrap();

    b.iter(|| {
        flip = !flip;
        let text = if flip { &line } else { &a[0] };
        s.set_str((0u16, HEIGHT / 2), text, Style::default());
        black_box(s.render()).unwrap();
        std::io::Write::flush(&mut s).unwrap();
    });
}

/// A whole frame with nothing changed: the floor an idle application pays
/// per tick.
#[bench]
fn frame_loop_idle(b: &mut Bencher) {
    let mut s = screen();
    paint(&mut s, &rows(0), 0);
    s.render().unwrap();
    std::io::Write::flush(&mut s).unwrap();

    b.iter(|| {
        black_box(s.render()).unwrap();
        std::io::Write::flush(&mut s).unwrap();
    });
}
