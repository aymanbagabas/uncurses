//! Decoder micro-benchmarks (nightly only).
//!
//! The decoder runs on every byte a terminal sends, so a keystroke, a mouse
//! move and a paste all land here. These time the shapes that actually
//! arrive rather than a synthetic worst case. Build and run them with:
//!
//! ```sh
//! RUSTFLAGS="--cfg uncurses_bench" cargo +nightly bench
//! ```
//!
//! Nothing compiles this module unless `--cfg uncurses_bench` is set, so it
//! has no effect on normal builds, tests, or downstream consumers.

extern crate test;

use test::{Bencher, black_box};

use super::{Decoder, DecoderFlags};

fn decoder() -> Decoder {
    Decoder::new(DecoderFlags::empty())
}

/// Drive `input` through the decoder to exhaustion, once per iteration.
///
/// This walks [`Decoder::parse_one`], the entry point an `EventSource`
/// calls, rather than collecting into a `Vec`: the allocation would be the
/// larger cost on a stream of short sequences and would bury what the parser
/// itself does.
fn bench_parse(b: &mut Bencher, input: &[u8]) {
    let mut d = decoder();
    b.iter(|| {
        let mut rest = black_box(input);
        let mut events = 0usize;
        while !rest.is_empty() {
            let (consumed, event) = d.parse_one(rest);
            if consumed == 0 {
                break;
            }
            black_box(&event);
            rest = &rest[consumed..];
            events += 1;
        }
        events
    });
}

#[bench]
fn csi_cursor_up(b: &mut Bencher) {
    // The smallest CSI worth parsing: no parameters, no intermediates.
    bench_parse(b, b"\x1b[A");
}

#[bench]
fn csi_sgr_truecolor(b: &mut Bencher) {
    // Parameter-heavy, with the colon subparameters SGR allows.
    bench_parse(b, b"\x1b[38:2::255:100:50m");
}

#[bench]
fn csi_sgr_semicolons(b: &mut Bencher) {
    bench_parse(b, b"\x1b[0;1;38;5;214;48;5;236m");
}

#[bench]
fn csi_private_mode(b: &mut Bencher) {
    // A private prefix, which the parameter scan has to step over.
    bench_parse(b, b"\x1b[?2048h");
}

#[bench]
fn csi_intermediate(b: &mut Bencher) {
    // An intermediate byte, the region a control sequence may or may not
    // have: DECSCUSR.
    bench_parse(b, b"\x1b[2 q");
}

#[bench]
fn csi_sgr_mouse(b: &mut Bencher) {
    // Emitted per mouse move, so the hottest sequence in a drag.
    bench_parse(b, b"\x1b[<0;10;20M");
}

#[bench]
fn csi_unrecognized(b: &mut Bencher) {
    // Falls through every recogniser, so it pays the whole chain.
    bench_parse(b, b"\x1b[99;99;99~");
}

#[bench]
fn dcs_decrpss_cursor_style(b: &mut Bencher) {
    // A DCS reads a control sequence twice: the command section, then the
    // setting it reports.
    bench_parse(b, b"\x1bP1$r2 q\x1b\\");
}

#[bench]
fn dcs_xtgettcap(b: &mut Bencher) {
    bench_parse(b, b"\x1bP1+r6B663133=1B5B313B3250\x1b\\");
}

#[bench]
fn mixed_stream(b: &mut Bencher) {
    // What an application actually reads in a busy moment: text with
    // styling and a mouse report threaded through it.
    let mut input = Vec::new();
    for i in 0..64 {
        input.extend_from_slice(b"\x1b[0;1;32mhello \x1b[m");
        input.extend_from_slice(b"\x1b[<35;12;7M");
        input.extend_from_slice(format!("row {i}\r\n").as_bytes());
    }
    bench_parse(b, &input);
}

#[bench]
fn plain_text(b: &mut Bencher) {
    // The baseline: no escape sequences at all, so anything the parser
    // costs shows up as the difference from this.
    bench_parse(b, b"the quick brown fox jumps over the lazy dog");
}
