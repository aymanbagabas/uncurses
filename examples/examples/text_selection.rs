//! Drag-to-select over mixed-width text, painted one cell at a time.
//!
//! Text is segmented once into cells with [`grapheme_cells`], and every frame
//! writes each cell with [`SurfaceMut::set_cell`], merging a highlight style
//! into the cells a selection covers. This is the shape a document renderer
//! takes when it owns a grid of cells and repaints it each frame.
//!
//! Two rules keep a wide cluster intact. A continuation belongs to the cell
//! on its left and is placed by that cell's own write, so the painter writes
//! leads only. And a selection is closed over whole clusters, because a
//! terminal draws a glyph or does not, and cannot draw half of one.
//!
//! The content mixes CJK, a joined family, a flag, and ASCII, so a drag
//! crosses clusters of one column and of two.
//!
//! Run with `cargo run --example text_selection`. Drag with the left button
//! to select; press `q`, `esc`, or `Ctrl-C` to quit.

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::cell::Cell;
use uncurses::color::Color;
use uncurses::event::{Event, Key};
use uncurses::program::{MouseTracking, Program, ProgramOptions};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::{TextSurface, grapheme_cells};

const LINES: &[&str] = &[
    "Text and selection",
    "",
    "Japanese with wide punctuation",
    "あのイーハトーヴォのすきとおった風、やまねの中のプリオシンのひとり。",
    "",
    "Grapheme clusters",
    "A family \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} and a flag \u{1f1ef}\u{1f1f5} are each one cluster.",
    "",
    "Plain ASCII",
    "The quick brown fox jumps over the lazy dog.",
    "",
    "drag to select, q quits",
];

/// One row of the document, segmented into cells exactly once.
///
/// `cells.len()` is the row's column count, because a cluster wider than one
/// column is stored as a lead cell plus continuations. That is what lets a
/// cell index be used interchangeably with a column.
struct Row {
    cells: Vec<Cell>,
}

impl Row {
    fn new(text: &str, screen: &Screen<Stdout>) -> Self {
        // Segmentation and width both come from the surface, so the cells
        // agree with what the renderer will draw.
        let mode = screen.width_mode();
        let eaw = screen.eaw_wide();
        let mut cells: Vec<Cell> = Vec::new();
        for (cluster, width) in grapheme_cells(text, mode, eaw) {
            match width {
                // A zero-width cluster shares a column with the cluster it
                // follows, so it joins that cell rather than claiming one of
                // its own. Giving it a column of its own would put the rest
                // of the row one column to the right of where it belongs.
                0 => match cells.last_mut() {
                    Some(last) => {
                        let mut joined = last.content().to_string();
                        joined.push_str(cluster);
                        *last = if last.is_wide() {
                            Cell::wide(joined)
                        } else {
                            Cell::narrow(joined)
                        };
                    }
                    // Opening the row, it has nothing to share a column
                    // with, so it takes one of its own. A terminal draws a
                    // mark with no base on its own too, and dropping it
                    // would lose text the row is meant to hold.
                    None => cells.push(Cell::narrow(cluster)),
                },
                1 => cells.push(Cell::narrow(cluster)),
                w => {
                    cells.push(Cell::wide(cluster));
                    for _ in 1..w {
                        cells.push(Cell::continuation());
                    }
                }
            }
        }
        Self { cells }
    }
}

/// A point in the document: which row, and which column within it.
type Point = (usize, usize);

#[derive(Default)]
struct Selection {
    anchor: Option<Point>,
    focus: Option<Point>,
    dragging: bool,
}

impl Selection {
    /// The selection in document order, whichever way the drag went.
    fn ordered(&self) -> Option<(Point, Point)> {
        let (a, b) = (self.anchor?, self.focus?);
        Some(if a <= b { (a, b) } else { (b, a) })
    }

    /// The selected column range on `row`, as a half-open range closed over
    /// whole clusters.
    ///
    /// A pointer lands on a column, and a column can be the second half of a
    /// wide cluster. A terminal draws a glyph or does not, so a range that
    /// began or ended inside one would ask for two styles in a single glyph.
    /// The start moves back to the column its cluster owns, and the end moves
    /// past the columns that cluster occupies.
    fn range_on(&self, row: usize, cells: &[Cell]) -> Option<(usize, usize)> {
        let columns = cells.len();
        let ((ar, ac), (br, bc)) = self.ordered()?;
        if row < ar || row > br {
            return None;
        }
        let mut start = if row == ar { ac } else { 0 };
        let mut end = if row == br { bc.min(columns) } else { columns };
        while start > 0 && cells.get(start).is_some_and(Cell::is_continuation) {
            start -= 1;
        }
        while end < columns && cells[end].is_continuation() {
            end += 1;
        }
        (start < end).then_some((start, end))
    }
}

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    program.init_with(ProgramOptions {
        mouse: Some(MouseTracking::MOTION),
        ..ProgramOptions::default()
    })?;
    program.query_capabilities(&[])?;
    program.enter_alt_screen()?;
    program.hide_cursor()?;

    let result = run(&mut program);
    program.finish()?;
    result
}

fn run(program: &mut Program<Stdin, Stdout>) -> std::io::Result<()> {
    let quit: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
    let mut rows: Vec<Row> = LINES
        .iter()
        .map(|line| Row::new(line, program.screen()))
        .collect();
    let mut selection = Selection::default();

    render(program.screen_mut(), &rows, &selection)?;

    loop {
        match program.read_event()? {
            Event::KeyPress(ref k) if quit.contains(k) => break,
            Event::MouseClick(m) => {
                let point = locate(&rows, m.x, m.y);
                selection.anchor = Some(point);
                selection.focus = Some(point);
                selection.dragging = true;
            }
            Event::MouseMove(m) if selection.dragging => {
                selection.focus = Some(locate(&rows, m.x, m.y));
            }
            Event::MouseRelease(m) => {
                selection.focus = Some(locate(&rows, m.x, m.y));
                selection.dragging = false;
            }
            Event::Resize(_) => {
                program.autoresize()?;
                rows = LINES
                    .iter()
                    .map(|line| Row::new(line, program.screen()))
                    .collect();
            }
            _ => continue,
        }
        render(program.screen_mut(), &rows, &selection)?;
    }
    Ok(())
}

/// Turn a screen position into a document point, clamped to the content.
fn locate(rows: &[Row], x: u16, y: u16) -> Point {
    let row = (y as usize).min(rows.len().saturating_sub(1));
    let column = (x as usize).min(rows[row].cells.len());
    (row, column)
}

fn render(screen: &mut Screen<Stdout>, rows: &[Row], selection: &Selection) -> std::io::Result<()> {
    screen.clear();
    let highlight = Style::default().fg(Color::Black).bg(Color::White);

    for (y, row) in rows.iter().enumerate() {
        if y >= screen.height() as usize {
            break;
        }
        let selected = selection.range_on(y, &row.cells);
        for (x, cell) in row.cells.iter().enumerate() {
            if x >= screen.width() as usize {
                break;
            }
            // A continuation is placed by the cell that owns it, so the
            // painter writes leads and lets each one claim its own columns.
            if cell.is_continuation() {
                continue;
            }
            let position = (x as u16, y as u16);
            if selected.is_some_and(|(a, b)| x >= a && x < b) {
                screen.set_cell(position, &cell.clone().style(highlight.clone()));
            } else {
                screen.set_cell(position, cell);
            }
        }
    }
    screen.render()
}
