//! Drag sized text over a dim Chinese character pattern.
//!
//! Run with `cargo run -p examples --example sized_text_drag` in a terminal
//! with Kitty text sizing support. Drag the heading with the left mouse
//! button. Press `l` to insert a log line above the screen.
//! Press `q`, `Esc`, or `Ctrl-C` to quit.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use uncurses::ansi;
use uncurses::buffer::SurfaceMut;
use uncurses::cell::Cell;
use uncurses::color::Color;
use uncurses::event::{Event, Key, MouseButton};
use uncurses::layout::{Position, Rect, Size};
use uncurses::program::{MouseTracking, Program, ProgramOptions};
use uncurses::screen::{Screen, TextSizing};
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

const PATTERN: &str = "山水云林风月星河";
const HEADING: &str = "拖动我";

struct Drag {
    rect: Rect,
    offset: Option<Position>,
}

impl Drag {
    fn fits(&self, size: Size) -> bool {
        self.rect.width <= size.width && self.rect.height < size.height
    }

    fn clamp(&mut self, size: Size) {
        self.rect.x = self.rect.x.min(size.width.saturating_sub(self.rect.width));
        self.rect.y = self
            .rect
            .y
            .min(size.height.saturating_sub(self.rect.height + 1));
    }

    fn mouse(&mut self, event: Event, size: Size) -> bool {
        match event {
            Event::MouseClick(m)
                if m.button == MouseButton::Left
                    && self.fits(size)
                    && self.rect.contains((m.x, m.y)) =>
            {
                self.offset = Some(Position::new(m.x - self.rect.x, m.y - self.rect.y));
            }
            Event::MouseMove(m) if m.button == MouseButton::Left && self.offset.is_some() => {
                self.move_to((m.x, m.y).into(), size);
            }
            Event::MouseRelease(m)
                if matches!(m.button, MouseButton::Left | MouseButton::None)
                    && self.offset.is_some() =>
            {
                self.move_to((m.x, m.y).into(), size);
                self.offset = None;
            }
            _ => return false,
        }
        true
    }

    fn move_to(&mut self, pointer: Position, size: Size) {
        if let Some(offset) = self.offset {
            self.rect.x = pointer.x.saturating_sub(offset.x);
            self.rect.y = pointer.y.saturating_sub(offset.y);
            self.clamp(size);
        }
    }
}

fn background(screen: &mut Screen<impl Write>) {
    screen.clear();
    let size = screen.size();
    let style = Style::default()
        .fg(Color::rgb(105, 125, 145))
        .bg(Color::rgb(14, 20, 28))
        .faint();
    let cells: Vec<_> = PATTERN
        .chars()
        .map(|ch| Cell::new(ch.to_string(), 2).style(style.clone()))
        .collect();
    let blank = Cell::BLANK.style(style);
    for y in 0..size.height {
        for x in (0..size.width.saturating_sub(1)).step_by(2) {
            let index = (usize::from(x / 2) + usize::from(y)) % cells.len();
            screen.set_cell((x, y), &cells[index]);
        }
        if !size.width.is_multiple_of(2) {
            screen.set_cell((size.width - 1, y), &blank);
        }
    }
}

fn heading_style() -> Style {
    Style::default()
        .fg(Color::rgb(255, 220, 110))
        .bg(Color::rgb(30, 48, 65))
        .bold()
}

fn render(screen: &mut Screen<impl Write>, drag: &Drag, sizing: TextSizing) -> io::Result<()> {
    let size = screen.size();
    let fits = drag.fits(size);
    if fits {
        screen.set_sized_text(
            "heading",
            drag.rect.position(),
            HEADING,
            heading_style(),
            sizing,
        )?;
    } else {
        screen.remove_region("heading");
    }
    if size.height > 0 {
        let style = Style::default().fg(Color::White).bg(Color::rgb(30, 48, 65));
        let footer = Rect::new(0, size.height - 1, size.width, 1);
        screen.fill_rect(footer, &Cell::BLANK.style(style.clone()));
        let message = if fits {
            format!(
                "{} | l logs | q quits | ({}, {})",
                if drag.offset.is_some() {
                    "Dragging"
                } else {
                    "Drag with left mouse"
                },
                drag.rect.x,
                drag.rect.y,
            )
        } else {
            format!("Resize to {}x{}", drag.rect.width, drag.rect.height + 1,)
        };
        screen.set_str(footer.position(), &message, style);
    }
    screen.render()
}

fn run(program: &mut Program<Stdin, Stdout>) -> io::Result<()> {
    program.enter_alt_screen()?;
    program.hide_cursor()?;
    check_sized_text(program)?;
    program.query_capabilities(&[])?;
    let sizing = TextSizing {
        scale: 3,
        width: 6,
        ..Default::default()
    };
    let block = sizing.size()?;
    let size = program.screen().size();
    let mut drag = Drag {
        rect: Rect::new(
            size.width.saturating_sub(block.width) / 2,
            size.height.saturating_sub(block.height + 1) / 2,
            block.width,
            block.height,
        ),
        offset: None,
    };
    let quit: [Key; 3] = ["q", "esc", "ctrl+c"].map(|key| key.parse().unwrap());
    let log: Key = "l".parse().unwrap();
    background(program.screen_mut());
    render(program.screen_mut(), &drag, sizing)?;
    loop {
        match program.read_event()? {
            Event::KeyPress(ref key) if quit.contains(key) => return Ok(()),
            Event::KeyPress(ref key) if *key == log => {
                program
                    .screen_mut()
                    .insert_above("Log line above the managed screen")?;
                continue;
            }
            Event::Resize(_) => {
                program.autoresize()?;
                drag.offset = None;
                drag.clamp(program.screen().size());
                background(program.screen_mut());
            }
            event => {
                if !drag.mouse(event, program.screen().size()) {
                    continue;
                }
            }
        }
        render(program.screen_mut(), &drag, sizing)?;
    }
}

fn write_sizing_probe(out: &mut impl Write) -> io::Result<()> {
    out.write_all(b"\r")?;
    ansi::cursor::write_request_cursor_position(out)?;
    ansi::text_sizing::write_sized_text(
        out,
        "",
        TextSizing {
            width: 2,
            ..Default::default()
        },
    )?;
    ansi::cursor::write_request_cursor_position(out)?;
    ansi::text_sizing::write_sized_text(
        out,
        "",
        TextSizing {
            scale: 2,
            width: 0,
            ..Default::default()
        },
    )?;
    ansi::cursor::write_request_cursor_position(out)
}

fn check_sized_text(program: &mut Program<Stdin, Stdout>) -> io::Result<()> {
    let screen = program.screen_mut();
    if screen.size().width < 6 || screen.size().height < 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "enlarge the terminal to at least 18 columns and 4 rows",
        ));
    }
    screen.move_cursor_to(Position::ORIGIN)?;
    write_sizing_probe(screen)?;
    screen.flush()?;
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut positions = Vec::new();
    let mut deferred = Vec::new();
    while positions.len() < 3 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || !program.poll_event(Some(remaining))? {
            break;
        }
        if let Some(event) = program.try_read_event()? {
            match event {
                Event::CursorPosition { position, .. } => positions.push(position),
                other => deferred.push(other),
            }
        }
    }
    for event in deferred.into_iter().rev() {
        program.unread_event(event);
    }
    program.screen_mut().invalidate_tracked_cursor();
    program.screen_mut().invalidate();
    if positions != [Position::ORIGIN, Position::new(2, 0), Position::new(4, 0)] {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the terminal output path did not confirm Kitty text width and scale support; run directly in a supporting terminal",
        ));
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let mut program = Program::stdio()?;
    program.init_with(ProgramOptions {
        mouse: Some(MouseTracking::empty()),
        ..Default::default()
    })?;
    let result = run(&mut program);
    let finish = program.finish();
    result?;
    finish
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use uncurses::buffer::Surface;
    use uncurses::event::{KeyModifiers, Mouse};

    fn record_frame(
        frames: &mut Vec<u8>,
        screen: &mut Screen<Vec<u8>>,
        reference: &mut Screen<Vec<u8>>,
        position: Option<Position>,
    ) -> io::Result<()> {
        let Size { width, height } = screen.size();
        let (x, y) = position.map_or((-1, -1), |pos| (i32::from(pos.x), i32::from(pos.y)));
        write!(frames, "{width}\t{height}\t{x}\t{y}\t")?;
        for target in [screen, reference] {
            for byte in target.writer_mut().drain(..) {
                write!(frames, "{byte:02x}")?;
            }
            frames.push(b'\t');
        }
        frames.push(b'\n');
        Ok(())
    }

    #[test]
    fn drag_keeps_the_grab_offset_and_stops_on_release() {
        let size = Size::new(80, 24);
        let mut drag = Drag {
            rect: Rect::new(10, 5, 18, 3),
            offset: None,
        };
        let mouse = |x, y, button| Mouse::new(x, y, button, KeyModifiers::empty());
        assert!(!drag.mouse(Event::MouseClick(mouse(0, 0, MouseButton::Left)), size));
        assert!(!drag.mouse(Event::MouseClick(mouse(12, 6, MouseButton::Right)), size));
        assert!(drag.mouse(Event::MouseClick(mouse(12, 6, MouseButton::Left)), size));
        drag.mouse(Event::MouseMove(mouse(23, 12, MouseButton::Left)), size);
        assert_eq!(drag.rect.position(), Position::new(21, 11));
        drag.mouse(Event::MouseMove(mouse(79, 23, MouseButton::Left)), size);
        assert_eq!(drag.rect.position(), Position::new(62, 20));
        drag.mouse(Event::MouseRelease(mouse(0, 0, MouseButton::None)), size);
        assert_eq!(drag.rect.position(), Position::ORIGIN);
        assert!(drag.offset.is_none());
        assert!(!drag.mouse(Event::MouseMove(mouse(50, 10, MouseButton::Left)), size));
    }

    #[test]
    fn moving_the_heading_preserves_the_wide_background() -> io::Result<()> {
        let sizing = TextSizing {
            scale: 3,
            width: 6,
            ..Default::default()
        };
        let mut screen = Screen::new(Vec::new(), (41, 10));
        screen.set_fullscreen(true);
        background(&mut screen);
        let original: Vec<_> = (0..9)
            .flat_map(|y| (0..41).map(move |x| (x, y)))
            .map(|pos| screen.cell(pos.into()).unwrap().clone())
            .collect();
        let mut drag = Drag {
            rect: Rect::new(3, 2, 18, 3),
            offset: None,
        };
        render(&mut screen, &drag, sizing)?;
        screen.writer_mut().clear();
        render(&mut screen, &drag, sizing)?;
        assert!(screen.writer().is_empty());
        drag.rect.x = 19;
        render(&mut screen, &drag, sizing)?;
        assert!(screen.writer().windows(4).any(|bytes| bytes == b"]66;"));
        let current: Vec<_> = (0..9)
            .flat_map(|y| (0..41).map(move |x| (x, y)))
            .map(|pos| screen.cell(pos.into()).unwrap().clone())
            .collect();
        assert_eq!(current, original);
        screen.resize((17, 3));
        drag.clamp(screen.size());
        assert!(!drag.fits(screen.size()));
        background(&mut screen);
        render(&mut screen, &drag, sizing)?;
        screen.resize((40, 10));
        background(&mut screen);
        render(&mut screen, &drag, sizing)?;
        Ok(())
    }

    #[test]
    #[ignore = "requires Kitty; compares incremental output with fresh native frames"]
    fn native_drag_matches_fresh_frames() -> io::Result<()> {
        let sizing = TextSizing {
            scale: 3,
            width: 6,
            ..Default::default()
        };
        let mut frames = Vec::new();
        let mut probe = Vec::new();
        write_sizing_probe(&mut probe)?;
        write!(frames, "probe\t")?;
        for byte in probe {
            write!(frames, "{byte:02x}")?;
        }
        frames.push(b'\n');
        for (width, height) in [(40, 10), (41, 10), (120, 42), (121, 42)] {
            for sync in [false, true] {
                let mut screen = Screen::new(Vec::new(), (width, height));
                screen.set_fullscreen(true);
                screen.set_synchronized_output(sync);
                background(&mut screen);
                let positions = (0..=width - 18)
                    .map(|x| (x, 2))
                    .chain((0..=height - 4).map(|y| ((width - 18) / 2, y)))
                    .chain((0..=width - 18).rev().map(|x| (x, height - 4)));
                for (x, y) in positions {
                    let drag = Drag {
                        rect: Rect::new(x, y, 18, 3),
                        offset: None,
                    };
                    render(&mut screen, &drag, sizing)?;
                    if x.is_multiple_of(3) {
                        screen.insert_above("log")?;
                    }
                    if x.is_multiple_of(7) {
                        screen.insert_above(&"line\n".repeat(usize::from(height) + 1))?;
                    }
                    let mut fresh = Screen::new(Vec::new(), (width, height));
                    fresh.set_fullscreen(true);
                    fresh.set_synchronized_output(sync);
                    background(&mut fresh);
                    render(&mut fresh, &drag, sizing)?;
                    record_frame(&mut frames, &mut screen, &mut fresh, Some((x, y).into()))?;
                }
            }
        }
        for sync in [false, true] {
            frames.extend_from_slice(b"reset\n");
            let mut screen = Screen::new(Vec::new(), (41, 10));
            let mut reference = Screen::new(Vec::new(), (41, 10));
            let drag = Drag {
                rect: Rect::new(3, 2, 18, 3),
                offset: None,
            };
            let displayed = Some(drag.rect.position());
            for target in [&mut screen, &mut reference] {
                target.set_fullscreen(true);
                target.set_synchronized_output(sync);
                background(target);
                render(target, &drag, sizing)?;
            }
            record_frame(&mut frames, &mut screen, &mut reference, displayed)?;
            for (width, height) in [(61, 14), (40, 8), (41, 10), (23, 6)] {
                // The parser resizes the same terminal before the replay bytes.
                writeln!(frames, "resize\t{width}\t{height}")?;
                screen.resize((width, height));
                reference.resize((width, height));
                screen.set_str((0, 0), "PENDING", Style::default());
                screen.set_sized_text("heading", (5, 2), HEADING, heading_style(), sizing)?;
                screen.insert_above("log")?;
                reference.render()?;
                record_frame(&mut frames, &mut screen, &mut reference, displayed)?;
            }
            reference.set_str((0, 0), "PENDING", Style::default());
            reference.set_sized_text("heading", (5, 2), HEADING, heading_style(), sizing)?;
            screen.render()?;
            reference.invalidate();
            reference.render()?;
            record_frame(
                &mut frames,
                &mut screen,
                &mut reference,
                Some((5, 2).into()),
            )?;
            screen.clear();
            screen.insert_above("pending clear")?;
            reference.invalidate();
            reference.render()?;
            record_frame(
                &mut frames,
                &mut screen,
                &mut reference,
                Some((5, 2).into()),
            )?;
            writeln!(frames, "resize\t20\t6")?;
            screen.resize((20, 6));
            reference.resize((20, 6));
            assert_eq!(
                screen.insert_above("does not fit").unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
            assert!(screen.writer().is_empty());
            reference.clear();
            screen.render()?;
            reference.render()?;
            record_frame(&mut frames, &mut screen, &mut reference, None)?;
        }
        let mut child = Command::new("kitty")
            .args([
                "+runpy",
                "import runpy; runpy.run_path('tests/sized_text_drag_native.py', run_name='__main__')",
            ])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let written = child.stdin.take().unwrap().write_all(&frames);
        let output = child.wait_with_output()?;
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        written?;
        print!("{}", String::from_utf8_lossy(&output.stdout));
        Ok(())
    }
}
