use criterion::{Criterion, black_box, criterion_group, criterion_main};
use uncurses::bench_support::{RenderBuffer, Renderer};
use uncurses::cell::Cell;

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

fn bench_swap_render(
    c: &mut Criterion,
    name: &str,
    mut renderer: Renderer,
    mut current: RenderBuffer,
    mut next: RenderBuffer,
    mut before_render: impl FnMut(&mut Renderer),
) {
    let mut out = Vec::with_capacity(16 * 1024);
    prime(&mut renderer, &mut current, &mut out);

    c.bench_function(name, |b| {
        b.iter(|| {
            std::mem::swap(black_box(&mut current), black_box(&mut next));
            out.clear();
            before_render(black_box(&mut renderer));
            black_box(renderer.render(black_box(&mut out), black_box(&mut current))).unwrap();
            black_box(&out);
        });
    });
}

fn full_frame_no_changes(c: &mut Criterion) {
    let mut first = filled_buffer(0);
    let mut second = first.clone();
    first.clear_touched();
    second.clear_touched();

    let mut renderer = Renderer::new();
    let mut out = Vec::with_capacity(16 * 1024);
    let mut initial = filled_buffer(0);
    prime(&mut renderer, &mut initial, &mut out);

    c.bench_function("full_frame_no_changes", |b| {
        b.iter(|| {
            std::mem::swap(black_box(&mut first), black_box(&mut second));
            out.clear();
            black_box(renderer.render(black_box(&mut out), black_box(&mut first))).unwrap();
            black_box(&out);
        });
    });
}

fn full_frame_all_cells_changed(c: &mut Criterion) {
    bench_swap_render(
        c,
        "full_frame_all_cells_changed",
        Renderer::new(),
        filled_buffer(0),
        filled_buffer(1),
        |_| {},
    );
}

fn single_cell_change(c: &mut Criterion) {
    let mut first = filled_buffer(0);
    let mut second = first.clone();
    first.clear_touched();
    second.clear_touched();
    first.set_cell((WIDTH / 2, HEIGHT / 2), &Cell::narrow("0"));
    second.set_cell((WIDTH / 2, HEIGHT / 2), &Cell::narrow("1"));

    bench_swap_render(
        c,
        "single_cell_change",
        Renderer::new(),
        first,
        second,
        |_| {},
    );
}

fn scroll_shift_up_by_1(c: &mut Criterion) {
    bench_swap_render(
        c,
        "scroll_shift_up_by_1",
        Renderer::new(),
        filled_buffer(0),
        shifted_up_buffer(),
        |_| {},
    );
}

fn force_clear_frame(c: &mut Criterion) {
    bench_swap_render(
        c,
        "force_clear_frame",
        Renderer::new(),
        filled_buffer(0),
        filled_buffer(0),
        |renderer| renderer.request_clear(),
    );
}

criterion_group!(
    benches,
    full_frame_no_changes,
    full_frame_all_cells_changed,
    single_cell_change,
    scroll_shift_up_by_1,
    force_clear_frame
);
criterion_main!(benches);
