//! A selection dragged backwards over wide text.
//!
//! Guards <https://github.com/aymanbagabas/uncurses/pull/56>, "keep the
//! cursor and wide clusters in agreement".
//!
//! # The shape this needs
//!
//! A reverse drag along a row of wide clusters alone does not reproduce the
//! defect. The cursor also has to be resting on an odd column when the frame
//! begins, which is what a shorter neighbouring row provides: `Grapheme
//! clusters` is seventeen columns, so a frame that ends there leaves the
//! cursor at column seventeen. The next frame reaches the wide row with a
//! move that keeps the column, and a cursor recorded one column too high
//! draws that row over the second half of the glyph before it.
//!
//! Without that neighbouring row the same drag is clean, which is why the
//! defect survived a long search: every headless attempt swept the wide row
//! on its own and saw nothing.
//!
//! # What went wrong
//!
//! The cursor planner can travel by re-emitting the cells it passes over
//! when that costs fewer bytes than a cursor sequence. Moving from a column
//! that continues a cluster, it drew nothing and still recorded the column
//! as crossed, so every later move inherited the error.

use tui_test::{
    Backend, MouseAction, MouseOptions, Operation, OperationResult, RunOptions, Session,
};

/// The row the defect shows on, and the text it must always read as.
const JAPANESE: &str = "あのイーハトーヴォのすきとおった風、やまねの中のプリオシンのひとり。";

/// Zero-based row of that line in the demo's output.
const JAPANESE_ROW: usize = 3;

/// Zero-based row of `Grapheme clusters`, which is seventeen columns wide.
/// A frame that ends there leaves the cursor on an odd column, which is what
/// the defect needs before it reaches the wide row.
const SHORT_ROW: usize = 5;

fn demo(name: &str, backend: Backend, cols: u16, rows: u16) -> Session {
    let session = Session::new(name.to_string());
    session
        .run(RunOptions {
            backend,
            program: env!("CARGO_BIN_EXE_text_selection_probe").to_string(),
            args: Vec::new(),
            profile: Default::default(),
            cols,
            rows,
            cwd: None,
            env: Vec::new(),
            wait_ready: None,
            restart: true,
            timeouts: Default::default(),
            recording: Default::default(),
        })
        .expect("the demo did not start");
    // The first frame is written after the terminal answers the capability
    // query, so the screen is empty until the program settles.
    settle(&session);
    session
}

/// Wait for the screen to stop changing.
///
/// Every read has to happen on a finished frame, because a frame caught
/// halfway through is a difference this test cannot tell from the one it
/// looks for.
fn settle(session: &Session) {
    session
        .execute(Operation::WaitIdle {
            timeout_ms: Some(5_000),
        })
        .expect("the demo never settled");
}

fn row(session: &Session, index: usize) -> String {
    match session
        .execute(Operation::Text { full: false })
        .expect("could not read the screen")
    {
        OperationResult::Text(text) => text
            .lines()
            .nth(index)
            .unwrap_or_default()
            .trim_end()
            .to_string(),
        other => panic!("expected screen text, got {other:?}"),
    }
}

fn mouse(session: &Session, action: MouseAction) {
    session
        .execute(Operation::Mouse { action })
        .expect("the demo did not take the mouse event");
    settle(session);
}

/// The terminals and sizes this is checked against.
///
/// The defect depends on where the cursor is resting when a frame begins,
/// which the row widths decide, so the surface width belongs in the matrix
/// alongside the terminal. A width that is not a multiple of the rows'
/// lengths puts the cursor somewhere different again.
fn matrix() -> Vec<Place> {
    vec![
        ("alacritty-100x20", Backend::Alacritty, 100, 20),
        ("alacritty-80x24", Backend::Alacritty, 80, 24),
        ("alacritty-79x20", Backend::Alacritty, 79, 20),
        ("alacritty-120x30", Backend::Alacritty, 120, 30),
        ("rio-100x20", Backend::Rio, 100, 20),
        ("rio-79x20", Backend::Rio, 79, 20),
        ("xtermjs-100x20", Backend::Xtermjs, 100, 20),
        ("xtermjs-79x20", Backend::Xtermjs, 79, 20),
    ]
}

/// A pointer path: what it is called, where the button goes down, and the
/// points it visits while held.
type Drag = (&'static str, (u16, u16), Vec<(u16, u16)>);

/// A terminal to run under: what it is called, which emulator, and the
/// surface size.
type Place = (&'static str, Backend, u16, u16);

/// The drags to sweep, each a sequence of points the pointer visits.
///
/// A step can land on a row other than the wide one, and that matters: the
/// defect needs the cursor resting on an odd column when a frame reaches the
/// wide row, which is what a frame ending on the seventeen-column row
/// leaves behind. A drag that stays on the wide row never sets that up, and
/// passes even on unfixed code.
fn drags() -> Vec<Drag> {
    let wide = JAPANESE_ROW as u16;
    let short = SHORT_ROW as u16;
    vec![
        (
            "reverse-coarse",
            (80, 9),
            (0..=70).rev().step_by(5).map(|x| (x, wide)).collect(),
        ),
        (
            "reverse-fine",
            (60, 9),
            (0..=59).rev().map(|x| (x, wide)).collect(),
        ),
        (
            "forward",
            (0, 9),
            (0..=70).step_by(5).map(|x| (x, wide)).collect(),
        ),
        (
            "across-the-anchor",
            (36, 9),
            [50u16, 40, 36, 30, 20, 30, 36, 44, 36, 24, 36, 60, 36, 10]
                .into_iter()
                .map(|x| (x, wide))
                .collect(),
        ),
        // Each step touches the short row before returning to the wide one,
        // so every frame that repaints the wide row begins with the cursor
        // wherever the short row left it.
        (
            "between-rows",
            (80, 9),
            (0..=70)
                .rev()
                .step_by(2)
                .flat_map(|x| [(x, short), (x, wide)])
                .collect(),
        ),
        (
            "between-rows-fine",
            (40, 9),
            (0..=39)
                .rev()
                .flat_map(|x| [(x, short), (x, wide)])
                .collect(),
        ),
    ]
}

/// Every frame of every drag leaves the wide row readable.
///
/// The row is read after every step, because the defect appears on one
/// frame and is corrected on the next, so a check only at the end sees
/// nothing.
#[test]
fn a_drag_leaves_the_wide_row_intact() {
    let mut failures = Vec::new();

    for (place, backend, cols, rows) in matrix() {
        for (pattern, (press_x, press_y), columns) in drags() {
            let name = format!("{place}-{pattern}");
            let session = demo(&name, backend, cols, rows);
            if row(&session, JAPANESE_ROW) != JAPANESE {
                session.close().ok();
                panic!("{name}: the probe did not start from the expected text");
            }

            mouse(
                &session,
                MouseAction::Down {
                    x: press_x,
                    y: press_y,
                    options: MouseOptions::new(Default::default()),
                },
            );

            for (x, y) in columns {
                mouse(&session, MouseAction::Move { x, y });
                let seen = row(&session, JAPANESE_ROW);
                if seen != JAPANESE {
                    failures.push(format!("{name} at ({x}, {y}): {seen:?}"));
                    break;
                }
            }
            session.close().ok();
        }
    }

    assert!(
        failures.is_empty(),
        "the wide row changed in {} of the {} runs:\n{}",
        failures.len(),
        matrix().len() * drags().len(),
        failures.join("\n")
    );
}
