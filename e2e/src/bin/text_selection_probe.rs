//! A drag-to-select surface built for a test to drive, not for a person.
//!
//! The rows are chosen for the geometry the wide-cluster cursor defect needs
//! rather than to demonstrate anything: a row of wide clusters with a
//! seventeen-column row below it, so a frame that ends on the short row
//! leaves the cursor on an odd column and the next frame reaches the wide
//! row with a move that keeps it.
//!
//! `examples/text_selection.rs` is the version meant to be run by hand.

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
    // Seventeen columns, which is what parks the cursor on an odd one.
    "Grapheme clusters",
    "A family \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} and a flag \u{1f1ef}\u{1f1f5} are each one cluster.",
    "",
    "Plain ASCII",
    "The quick brown fox jumps over the lazy dog.",
    "",
    "drag to select, q quits",
];

struct Row {
    cells: Vec<Cell>,
}

impl Row {
    fn new(text: &str, screen: &Screen<Stdout>) -> Self {
        let mode = screen.width_mode();
        let eaw = screen.eaw_wide();
        let mut cells: Vec<Cell> = Vec::new();
        for (cluster, width) in grapheme_cells(text, mode, eaw) {
            match width {
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

type Point = (usize, usize);

#[derive(Default)]
struct Selection {
    anchor: Option<Point>,
    focus: Option<Point>,
    dragging: bool,
}

impl Selection {
    fn ordered(&self) -> Option<(Point, Point)> {
        let (a, b) = (self.anchor?, self.focus?);
        Some(if a <= b { (a, b) } else { (b, a) })
    }

    /// Closed over whole clusters, so the surface is only ever asked for
    /// ranges a terminal can draw.
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
            // A continuation is placed by the cell that owns it.
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
