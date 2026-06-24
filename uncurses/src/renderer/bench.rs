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

use crate::cell::Cell;
use crate::renderer::{RenderBuffer, Renderer};

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn filled_buffer(offset: u8) -> RenderBuffer {
    let mut buf = RenderBuffer::new(WIDTH, HEIGHT);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let ch = char::from(b'a' + ((x as u8 + y as u8 + offset) % 26));
            buf.set_cell((x, y), &Cell::narrow(ch.to_string()));
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
            buf.set_cell((x, y), &Cell::narrow(ch.to_string()));
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
    first.set_cell((WIDTH / 2, HEIGHT / 2), &Cell::narrow("0"));
    second.set_cell((WIDTH / 2, HEIGHT / 2), &Cell::narrow("1"));

    bench_swap_render(b, Renderer::new(), first, second, |_| {});
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
