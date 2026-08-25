//! Reproduction harness for the background-colour render bug seen in `diffv`.
//!
//! Renders long lorem lines that each carry a background colour and irregular
//! runs of spaces — leading indent, gaps in the middle, and a tail that either
//! stops at the last word or runs to the right edge. Scrolling horizontally
//! shifts every visible row by the same amount, which is exactly the shape the
//! renderer answers with `DCH`/`ICH` runs (`\e[3P`, `\e[2P`, …) plus `EL`, the
//! output captured in the bug reports.
//!
//! The artefact is a cell that keeps the wrong background for good: once the
//! renderer's model of the program drifts, the next diff sees no work for that
//! cell and nothing ever repairs it.
//!
//! Keys: `h`/`l` or `←`/`→` scroll one column, `b`/`w` scroll eight,
//! `j`/`k` or `↓`/`↑` scroll a row, `m` toggles the animated overlay
//! (`diffv`'s mascot — it perturbs the diff every frame), `q` quits.
//!
//! Run it inside tmux, which is where the bug shows up:
//!
//! ```sh
//! tmux new-session 'cargo run --example bce_scroll'
//! ```

use std::io::Write;

use uncurses::buffer::{Bounded, Surface, SurfaceMut};
use uncurses::color::Color;
use uncurses::event::{Event, KeyCode};
use uncurses::program::{Program, ProgramOptions};
use uncurses::screen::Optimizations;
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

const WORDS: &[&str] = &[
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipiscing",
    "elit",
    "sed",
    "do",
    "eiusmod",
    "tempor",
    "incididunt",
    "ut",
    "labore",
    "et",
    "dolore",
    "magna",
    "aliqua",
    "enim",
    "ad",
    "minim",
    "veniam",
    "quis",
    "nostrud",
];

/// `diffv`'s palette: added, removed, context, and a plain unhighlighted row.
const BACKGROUNDS: &[Option<Color>] = &[
    Some(Color::Rgb(43, 58, 46)),
    Some(Color::Rgb(63, 45, 48)),
    Some(Color::Rgb(62, 68, 81)),
    None,
];

const LINES: usize = 200;

/// One rendered row: the text, its background, and whether the background runs
/// all the way to the right edge or stops after the last word.
struct Line {
    text: String,
    bg: Option<Color>,
    fill_to_edge: bool,
}

/// Deterministic xorshift so a reproduction can be described by its seed alone.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn main() -> std::io::Result<()> {
    let seed = std::env::var("BCE_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x2545F491_4F6CDD1D);

    let mut program = Program::stdio()?;
    program.init_with(ProgramOptions::default())?;
    // Tracing knob: `BCE_OFF=TABS,REP,...` drops optimizations by name so a
    // capture can be read unambiguously. `capture-pane` re-emits a tab-skipped
    // run as a single `\t` and cannot say which columns it covers, so `TABS`
    // has to go before any column-by-column comparison means anything.
    if let Ok(off) = std::env::var("BCE_OFF") {
        let mut opts = program.screen().optimizations();
        for name in off.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match Optimizations::from_name(name) {
                Some(flag) => opts.remove(flag),
                None => panic!("BCE_OFF: unknown optimization {name:?}"),
            }
        }
        program.screen_mut().set_optimizations(opts);
    }
    program.enter_alt_screen()?;
    program.hide_cursor()?;

    let result = run(&mut program, build_lines(seed));

    program.finish()?;
    result
}

/// Builds lines whose space runs vary every couple of rows: the indent, the
/// gaps between words, and the tail all change on a different cycle, so no two
/// neighbouring rows share a blank layout.
fn build_lines(seed: u64) -> Vec<Line> {
    let mut rng = Rng(seed | 1);

    (0..LINES)
        .map(|i| {
            // Indent and gap width change every other row; the tail every
            // third. Overlapping periods keep the pattern from repeating.
            let indent = 2 * ((i / 2) % 5);
            let gap = 1 + (i / 2) % 6;
            let mut text = " ".repeat(indent);

            for w in 0..12 + rng.below(10) {
                if w > 0 {
                    // Every few words the gap widens into a run of spaces
                    // wide enough for a DCH/ICH shift to land inside it.
                    text.push_str(&" ".repeat(if w % 4 == 0 { gap } else { 1 }));
                }
                text.push_str(WORDS[rng.below(WORDS.len())]);
            }

            // A trailing run of spaces that still carries the background.
            text.push_str(&" ".repeat(rng.below(12)));

            Line {
                text,
                bg: BACKGROUNDS[(i / 2) % BACKGROUNDS.len()],
                fill_to_edge: i % 3 != 0,
            }
        })
        .collect()
}

fn run(program: &mut Program<Stdin, Stdout>, lines: Vec<Line>) -> std::io::Result<()> {
    program.set_title("BCE scroll repro")?;

    let mut col: usize = 0;
    let mut row: usize = 0;
    let mut mascot = true;
    let mut tick: usize = 0;
    // `BCE_FRAMES=<n>` renders n frames per keystroke, advancing the overlay
    // each time. That is the churn a frame timer produces -- the renderer
    // diffing a moving overlay against the rows many times per scroll -- but
    // driven by input, so the program is quiescent between keys and a trace can
    // capture without racing an animation.
    let frames: usize = std::env::var("BCE_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);

    // `BCE_DUMP=<path>` appends the frame the app *intended* after every
    // render, encoded the same way `tmux capture-pane -e` encodes a pane. The
    // program has to match it; where it does not, the renderer's model of the
    // program has drifted and nothing will ever repair that cell. Unlike a
    // forced repaint this observes without touching the program, so drift
    // accumulates instead of being repaired by the act of looking.
    let mut dump = std::env::var("BCE_DUMP")
        .ok()
        .map(|p| std::fs::File::create(p).expect("BCE_DUMP"));

    loop {
        for _ in 0..frames.max(1) {
            draw(
                program.screen_mut(),
                &lines,
                col,
                row,
                mascot.then_some(tick),
            );
            program.screen_mut().render()?;
            if let Some(f) = dump.as_mut() {
                dump_intent(program.screen_mut(), f)?;
            }
            tick += 1;
        }

        if program.poll_event(None)? {
            let ev = program.read_event()?;
            program.observe_event(&ev)?;
            match ev {
                Event::KeyPress(key) => match key.code {
                    KeyCode::Char('q') | KeyCode::Escape => break,
                    KeyCode::Char('h') | KeyCode::Left => col = col.saturating_sub(1),
                    KeyCode::Char('l') | KeyCode::Right => col += 1,
                    KeyCode::Char('b') => col = col.saturating_sub(8),
                    KeyCode::Char('w') => col += 8,
                    KeyCode::Char('k') | KeyCode::Up => row = row.saturating_sub(1),
                    KeyCode::Char('j') | KeyCode::Down => row = (row + 1).min(lines.len() - 1),
                    KeyCode::Char('m') => mascot = !mascot,
                    // Advance the overlay without scrolling.
                    KeyCode::Char(' ') => {}
                    // Repaint the identical frame from scratch. Whatever the
                    // incremental path left on program has to match what a
                    // full redraw produces; if it does not, the renderer's
                    // model of the program has drifted and that cell will
                    // never be repaired. Deliberately does not advance
                    // `tick`, so the two frames really are identical.
                    KeyCode::Char('r') => {
                        // The overlay must not move, or the two frames are
                        // not the same frame.
                        tick = tick.saturating_sub(frames.max(1));
                        program.screen_mut().invalidate();
                        continue;
                    }
                    _ => {}
                },
                Event::Resize(ws) => program.screen_mut().resize((ws.col, ws.row)),
                _ => {}
            }
        }
    }

    Ok(())
}

/// Writes the front buffer as `capture-pane -e`-style text: SGR runs followed
/// by the cell characters, one line per row. Continuation cells of a wide
/// glyph carry no bytes of their own and are skipped.
fn dump_intent(screen: &mut Screen<Stdout>, out: &mut std::fs::File) -> std::io::Result<()> {
    writeln!(out, "--- frame ---")?;
    let (w, h) = (screen.width(), screen.height());
    for y in 0..h {
        let mut pen: Option<Style> = None;
        for x in 0..w {
            let Some(cell) = screen.cell((x, y).into()) else {
                continue;
            };
            if cell.is_continuation() {
                continue;
            }
            if pen != Some(cell.style.style) {
                write!(out, "{}", sgr(&cell.style.style))?;
                pen = Some(cell.style.style);
            }
            write!(out, "{cell}")?;
        }
        writeln!(out)?;
    }
    out.flush()
}

fn sgr(style: &Style) -> String {
    let part = |c: Option<Color>, base: u8, reset: u8| match c {
        Some(Color::Rgb(r, g, b)) => format!("\x1b[{base};2;{r};{g};{b}m"),
        // The corpus only paints with `Color::Rgb`.
        Some(other) => unreachable!("unexpected colour {other:?}"),
        None => format!("\x1b[{reset}m"),
    };
    format!("{}{}", part(style.fg, 38, 39), part(style.bg, 48, 49))
}

fn draw(
    screen: &mut Screen<Stdout>,
    lines: &[Line],
    col: usize,
    row: usize,
    mascot: Option<usize>,
) {
    screen.clear();

    let w = screen.width() as usize;
    let h = screen.height() as usize;
    if w == 0 || h == 0 {
        return;
    }

    for y in 0..h.saturating_sub(1) {
        let Some(line) = lines.get(row + y) else {
            break;
        };

        // The horizontal window into the line. Slicing by chars keeps the
        // offset in columns; the corpus is ASCII so they agree.
        let visible: String = line.text.chars().skip(col).take(w).collect();
        let text = if line.fill_to_edge {
            format!("{visible:w$}")
        } else {
            visible
        };

        let mut style = Style::default().fg(Color::Rgb(171, 178, 191));
        if let Some(bg) = line.bg {
            style = style.bg(bg);
        }
        screen.set_str((0, y as u16), &text, style);
    }

    // A small animated block standing in for `diffv`'s mascot: it moves every
    // frame, so the renderer diffs a moving overlay against the scrolled rows
    // instead of a clean shift.
    if let Some(tick) = mascot {
        let mx = (tick / 3) % w.saturating_sub(6).max(1);
        let my = h.saturating_sub(6) + (tick / 7) % 4;
        let style = Style::default()
            .fg(Color::Rgb(40, 44, 52))
            .bg(Color::Rgb(229, 192, 123));
        for (i, art) in [" ^_^ ", "(   )"].iter().enumerate() {
            if my + i < h {
                screen.set_str((mx as u16, (my + i) as u16), art, style);
            }
        }
    }

    let help = " h/l scroll  b/w x8  j/k rows  space tick  r repaint  m mascot  q quit ";
    screen.set_str(
        (0, h.saturating_sub(1) as u16),
        &format!("{help:w$}"),
        Style::default()
            .fg(Color::Rgb(40, 44, 52))
            .bg(Color::Rgb(97, 175, 239)),
    );
}
