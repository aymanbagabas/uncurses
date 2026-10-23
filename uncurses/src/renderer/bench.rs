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

extern crate test;

use test::{Bencher, black_box};

use crate::buffer::SurfaceMut;
use crate::cell::Cell;
use crate::renderer::{RenderBuffer, Renderer};

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn bench_placements(b: &mut Bencher, count: u64, workload: u8) {
    use crate::{
        layout::Rect,
        screen::{PlacementOptions, Screen},
        style::Style,
        text::TextSurface,
    };
    let mut screen = Screen::new(Vec::new(), (240, 80));
    screen.set_fullscreen(true);
    let options = PlacementOptions {
        draw: b"PLACEMENT".to_vec(),
        ..Default::default()
    };
    let paint = |screen: &mut Screen<Vec<u8>>, offset: u16| {
        for id in 1..=count {
            let x = ((id - 1) % 8) as u16 * 28 + 2 + if id == 1 { offset } else { 0 };
            let y = ((id - 1) / 8) as u16 * 8 + 3;
            screen
                .set_placement(id, Rect::new(x, y, 12, 3), &options)
                .unwrap();
        }
    };
    paint(&mut screen, 0);
    screen.render().unwrap();
    let mut tick = false;
    b.iter(|| {
        screen.writer_mut().clear();
        tick = !tick;
        match workload {
            0 => {}
            1 => {
                screen.set_str((2, 78), if tick { "a" } else { "b" }, Style::EMPTY);
            }
            2 => paint(&mut screen, 0),
            3 => {
                screen.clear();
                paint(&mut screen, u16::from(tick));
            }
            4 => {
                paint(&mut screen, 0);
                screen.set_str((5, 4), if tick { "TOP" } else { "top" }, Style::EMPTY);
            }
            _ => unreachable!(),
        }
        screen.render().unwrap();
        black_box(screen.writer());
    });
}

macro_rules! placement_benchmark {
    ($name:ident, $count:expr, $workload:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            bench_placements(b, $count, $workload);
        }
    };
}

placement_benchmark!(placements_unchanged_0, 0, 0);
placement_benchmark!(placements_unchanged_1, 1, 0);
placement_benchmark!(placements_unchanged_64, 64, 0);
placement_benchmark!(placements_sparse_0, 0, 1);
placement_benchmark!(placements_sparse_1, 1, 1);
placement_benchmark!(placements_sparse_64, 64, 1);
placement_benchmark!(placements_equal_0, 0, 2);
placement_benchmark!(placements_equal_1, 1, 2);
placement_benchmark!(placements_equal_64, 64, 2);
placement_benchmark!(placements_move_1, 1, 3);
placement_benchmark!(placements_move_64, 64, 3);
placement_benchmark!(placements_overlay_1, 1, 4);
placement_benchmark!(placements_overlay_64, 64, 4);

fn filled_buffer(offset: u8) -> RenderBuffer {
    let mut buf = RenderBuffer::new(WIDTH, HEIGHT);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let ch = char::from(b'a' + ((x as u8 + y as u8 + offset) % 26));
            buf.set_cell((x, y), &Cell::new(ch.to_string(), 1));
        }
    }
    buf
}

fn shifted_up_buffer() -> RenderBuffer {
    let mut buf = RenderBuffer::new(WIDTH, HEIGHT);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let source_y = (y + 1) % HEIGHT;
            let ch = char::from(b'a' + ((x as u8 + source_y as u8) % 26));
            buf.set_cell((x, y), &Cell::new(ch.to_string(), 1));
        }
    }
    buf
}

fn prime(renderer: &mut Renderer, buf: &mut RenderBuffer, out: &mut Vec<u8>) {
    renderer.render(out, buf).unwrap();
    out.clear();
}

/// Run a swap-and-render loop between two prepared frames, optionally mutating
/// the renderer before each render (e.g. to force a clear).
fn bench_swap_render(
    b: &mut Bencher,
    mut renderer: Renderer,
    mut current: RenderBuffer,
    mut next: RenderBuffer,
    mut before_render: impl FnMut(&mut Renderer),
) {
    let mut out = Vec::with_capacity(16 * 1024);
    prime(&mut renderer, &mut current, &mut out);

    b.iter(|| {
        std::mem::swap(black_box(&mut current), black_box(&mut next));
        out.clear();
        before_render(black_box(&mut renderer));
        black_box(renderer.render(black_box(&mut out), black_box(&mut current))).unwrap();
        black_box(&out);
    });
}

#[bench]
fn full_frame_no_changes(b: &mut Bencher) {
    let mut first = filled_buffer(0);
    let mut second = first.clone();
    first.clear_touched();
    second.clear_touched();

    let mut renderer = Renderer::new();
    let mut out = Vec::with_capacity(16 * 1024);
    let mut initial = filled_buffer(0);
    prime(&mut renderer, &mut initial, &mut out);

    b.iter(|| {
        std::mem::swap(black_box(&mut first), black_box(&mut second));
        out.clear();
        black_box(renderer.render(black_box(&mut out), black_box(&mut first))).unwrap();
        black_box(&out);
    });
}

#[bench]
fn full_frame_all_cells_changed(b: &mut Bencher) {
    bench_swap_render(
        b,
        Renderer::new(),
        filled_buffer(0),
        filled_buffer(1),
        |_| {},
    );
}

#[bench]
fn single_cell_change(b: &mut Bencher) {
    let mut first = filled_buffer(0);
    let mut second = first.clone();
    first.clear_touched();
    second.clear_touched();
    first.set_cell((WIDTH / 2, HEIGHT / 2), &Cell::new("0", 1));
    second.set_cell((WIDTH / 2, HEIGHT / 2), &Cell::new("1", 1));

    bench_swap_render(b, Renderer::new(), first, second, |_| {});
}

#[bench]
fn scroll_shift_up_by_1(b: &mut Bencher) {
    // Scroll detection is fullscreen-only and requires synchronized output,
    // so a bare `Renderer::new()` never reaches it and this measures a plain
    // redraw instead of the path it names.
    let mut r = Renderer::new();
    r.set_fullscreen(true);
    r.set_sync_output(true);
    bench_swap_render(b, r, filled_buffer(0), shifted_up_buffer(), |_| {});
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
