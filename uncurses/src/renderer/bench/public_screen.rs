use super::{Bencher, HEIGHT, WIDTH, black_box};
use crate::{
    buffer::{Buffer, Surface, SurfaceMut},
    cell::Cell,
    color::Profile,
    screen::Screen,
    style::Style,
    text::TextSurface,
};

fn frame(shift: u16, wide: bool) -> Buffer {
    let mut buffer = Buffer::new(WIDTH, HEIGHT);
    for y in 0..HEIGHT {
        let mut x = 0;
        while x < WIDTH {
            let cell = if wide {
                Cell::new(
                    if shift == 0 { "\u{754c}" } else { "\u{8a9e}" },
                    2,
                    Style::EMPTY,
                )
            } else {
                let value = b'a' + ((x + (y + shift) % HEIGHT) % 26) as u8;
                Cell::new(char::from(value).to_string(), 1, Style::EMPTY)
            };
            buffer.set_cell((x, y).into(), &cell);
            x += u16::from(cell.width());
        }
    }
    buffer
}

fn exercise(b: &mut Bencher, workload: u8, graphemes: bool) {
    let mut screen = Screen::new(Vec::new(), (WIDTH, HEIGHT));
    screen.set_fullscreen(true);
    screen.set_synchronized_output(workload == 4);
    screen.set_grapheme_clusters(graphemes);
    screen.set_color_profile(Profile::TrueColor);
    let first = frame(0, workload == 5);
    let second = frame(1, workload == 5);
    first.draw(&mut screen, (0, 0).into());
    screen.render().unwrap();
    let mut tick = false;
    b.iter(|| {
        screen.writer_mut().clear();
        tick = !tick;
        match workload {
            0 => {}
            1 => {
                screen.set_str((40, 12), if tick { "x" } else { "y" }, Style::EMPTY);
            }
            2 => {
                screen.clear();
                first.draw(&mut screen, (0, 0).into());
            }
            3..=5 => {
                let frame = if tick { &second } else { &first };
                frame.draw(&mut screen, (0, 0).into());
            }
            _ => unreachable!(),
        }
        screen.render().unwrap();
        black_box(screen.writer());
    });
}

#[bench]
fn screen_idle(b: &mut Bencher) {
    exercise(b, 0, true);
}

#[bench]
fn screen_sparse_text(b: &mut Bencher) {
    exercise(b, 1, true);
}

#[bench]
fn screen_equal_rebuild(b: &mut Bencher) {
    exercise(b, 2, true);
}

#[bench]
fn screen_all_cells_changed(b: &mut Bencher) {
    exercise(b, 3, true);
}

#[bench]
fn screen_scroll(b: &mut Bencher) {
    exercise(b, 4, true);
}

#[bench]
fn screen_wide_cells_changed(b: &mut Bencher) {
    exercise(b, 5, true);
}

#[bench]
fn screen_sparse_text_wc(b: &mut Bencher) {
    exercise(b, 1, false);
}

#[bench]
fn screen_all_cells_changed_wc(b: &mut Bencher) {
    exercise(b, 3, false);
}
