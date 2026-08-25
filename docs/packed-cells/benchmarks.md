# Benchmarks

All figures are `cargo +nightly bench` on one machine, 80x24 grid unless the
name says `big` (200x50), TrueColor profile, output to a discarding sink.
Fat and packed were measured in the same session with cooldowns between runs.

Run them with:

```sh
RUSTFLAGS="--cfg uncurses_bench" cargo +nightly bench -p uncurses renderer::bench
```

## Methodology, and two ways to get it wrong

**Isolate the target directory.** This repository inherits a global
`CARGO_TARGET_DIR`, so two checkouts of the same package share build
artifacts. An A/B across a worktree silently reported the working tree at
30,975 ns for a frame it actually renders in 12,919. Always:

```sh
CARGO_TARGET_DIR=/tmp/t-mine cargo +nightly bench ...
```

**Time the thing you mean to time.** Two benchmarks in this suite measured
nothing useful before being corrected. One built a `String` per row inside
`b.iter`, so it mostly measured formatting. Another built its buffers in
setup, so the interning it was meant to measure happened outside the timed
loop and an A/B of the link memo showed a flat 13,042 against 13,075. The
same comparison against a benchmark that times the writes showed 78,105
against 13,396.

## What the groups measure

Frame cost has three roughly independent inputs, so the suite varies one at a
time: how much changed, what the content is, and how the style moves. A
second grid size runs the headline cases so the numbers can be read for
scaling.

Note the difference between `untouched_frame_early_out` and
`touched_frame_no_diff`. The first measures the "nothing was drawn" check and
never looks at a cell. The second dirties every row and makes the renderer
prove cell by cell that there is nothing to emit, which is the honest cost of
diffing an unchanged screen.

## Render path

| bench | fat | packed | |
|---|---:|---:|---|
| `touched_frame_no_diff` | 6,368 | 1,071 | 5.95x |
| `full_frame_all_cells_changed` | 29,428 | 12,994 | 2.26x |
| `force_clear_frame` | 36,091 | 13,260 | 2.72x |
| `scroll_shift_up_by_1` | 31,099 | 13,133 | 2.37x |
| `full_frame_cjk` | 20,155 | 7,979 | 2.53x |
| `full_frame_clusters` | 30,514 | 19,515 | 1.56x |
| `full_frame_styled_runs` | 35,037 | 16,684 | 2.10x |
| `full_frame_style_churn` | 46,135 | 12,750 | 3.62x |
| `single_cell_change` | 455 | 94 | 4.86x |
| `single_line_change` | 1,264 | 553 | 2.28x |
| `scattered_tenth_change` | 22,569 | 10,452 | 2.16x |
| `contiguous_tenth_change` | 14,438 | 2,604 | 5.55x |
| `wide_span_two_changes` | 15,352 | 3,735 | 4.11x |
| `untouched_frame_early_out` | 7.89 | 7.34 | 1.07x |
| `big_full_frame_all_cells_changed` | 163,267 | 66,381 | 2.46x |
| `big_single_cell_change` | 1,082 | 188 | 5.76x |

## Draw and whole frame

This is where the packed form is paid for, and where the answer changes.

| bench | fat | packed | |
|---|---:|---:|---|
| `draw_frame_set_cell` | 14,084 | 13,111 | 1.07x |
| `draw_frame_set_str` | 30,308 | 29,318 | 1.03x |
| `draw_frame_set_cell_linked` | n/a | 13,459 | see below |
| **`draw_frame_set_cell_churn`** | **10,542** | **99,742** | **0.11x** |
| `frame_loop_full_repaint` | 70,581 | 48,268 | 1.46x |
| `frame_loop_one_line` | 3,435 | 2,083 | 1.65x |
| `frame_loop_idle` | 16.0 | 16.2 | 0.99x |

`draw_frame_set_cell_churn` gives every cell a distinct truecolor style, which
is what a gradient or a star field does. Nothing repeats, the style memo never
hits, and every cell mints an id that is never reused. This is the worst case
for interning and it is 9.5 times slower than storing the style inline.

## Cost model

Four benchmarks solve for a consistent model of the emit path:

- about 0.66 ns per cell scanned
- about 6 ns per cell emitted
- about 44 ns per cursor move

Check: `scattered_tenth_change` is 1,140 scan + (192 x 6) + (181 x 44), which
is 10,260 against a measured 10,257.

**A cursor move costs about seven emitted cells.** That is why
`scattered_tenth_change` costs 81 percent of a full repaint while changing ten
percent of the cells, on both trees. Scattered changes force the renderer to
hop, and hopping is the expensive part. A full repaint degenerates into one
long stream with a single cursor move: 1,971 bytes out with one escape
sequence, against 981 bytes with 181 of them.

## Frame budget

For a full repaint at 80x24:

```
draw     29,318 ns   61 percent
render   12,994 ns   27 percent
flush        24 ns    0 percent
```

Flushing is free. It is a `write_all` of one contiguous staging buffer, so the
library side cost is a memcpy; against a real terminal it becomes a syscall.

Within `render`, emission is about 92 percent and diffing about 8 percent.
Drawing is the larger half of the whole frame, which is worth remembering
before optimising the renderer further.
