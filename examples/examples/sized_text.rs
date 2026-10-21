//! Demonstrate stable regions, ordered text writes, and underlying text.
//!
//! Run in a terminal with Kitty text sizing support for the visual example.
//! Use `--preview` for escaped bytes or `--dump` for tab-separated frame names
//! and hexadecimal bytes for replay through a terminal parser.
//! Add `--inline` to use eight rows in the normal terminal buffer.

use std::io::{self, Write};
use std::time::Duration;

use uncurses::ansi::mode::Mode;
use uncurses::buffer::{SurfaceMut, View};
use uncurses::cell::Cell;
use uncurses::layout::{Rect, Size};
use uncurses::screen::{PreparedRegion, Region, RegionContext, RegionOp, Screen, TextSizing};
use uncurses::style::Style;
use uncurses::text::TextSurface;

#[derive(PartialEq)]
struct CaptionedText {
    title: &'static str,
    caption: &'static str,
}

impl Region for CaptionedText {
    fn same(&self, other: &dyn Region) -> bool {
        other.downcast_ref::<Self>() == Some(self)
    }

    fn prepare(&self, size: Size, _: RegionContext) -> io::Result<PreparedRegion> {
        if size != Size::new(10, 3)
            || self.caption.len() > 10
            || !self
                .caption
                .bytes()
                .all(|b| b == b' ' || b.is_ascii_graphic())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a 10x3 block and a printable ASCII caption",
            ));
        }
        let mut title = Vec::new();
        uncurses::ansi::text_sizing::write_sized_text(
            &mut title,
            self.title,
            TextSizing {
                scale: 2,
                width: 5,
                ..Default::default()
            },
        )?;
        Ok(PreparedRegion {
            draw: vec![
                RegionOp::SetStyle(Style::default().bold()),
                RegionOp::Write(title),
                RegionOp::SetStyle(Style::default().faint()),
                RegionOp::MoveTo((0, 2).into()),
                RegionOp::Write(format!("{:<10}", self.caption).into_bytes()),
            ],
            erase: vec![RegionOp::Erase(Rect::new(0, 0, 10, 3))],
        })
    }
}

fn layout(screen: &mut Screen<Vec<u8>>) -> io::Result<()> {
    screen.clear();
    screen.set_str((2, 2), "underlying cells", Style::default());
    heading(screen, (2, 2))?;
    screen.set_str((0, 7), "Regions and ordinary cells", Style::default());
    screen.set_cursor_position((0, 7));
    Ok(())
}

fn heading(screen: &mut Screen<Vec<u8>>, position: (u16, u16)) -> io::Result<()> {
    screen.set_sized_text(
        "heading",
        position,
        "Build",
        Style::default().bold(),
        TextSizing {
            scale: 2,
            width: 5,
            ..Default::default()
        },
    )
}

fn demonstrate(
    size: Size,
    inline: bool,
    mut show: impl FnMut(&str, &[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let mut screen = Screen::new(Vec::new(), size);
    screen.set_fullscreen(!inline);
    screen.set_synchronized_output(true);
    let present = |name: &str,
                   screen: &mut Screen<Vec<u8>>,
                   show: &mut dyn FnMut(&str, &[u8]) -> io::Result<()>| {
        let label = if name == "initial" || name == "identical rebuild" {
            "initial / identical rebuild"
        } else {
            name
        };
        screen.clear_rect(Rect::new(0, 0, size.width, 1));
        screen.set_str((0, 0), label, Style::default().bold());
        screen.set_str((0, 1), "Each stage lasts 1.5 seconds.", Style::default());
        screen.render()?;
        let bytes = std::mem::take(screen.writer_mut());
        show(name, &bytes)?;
        Ok::<_, io::Error>(bytes.len())
    };

    layout(&mut screen)?;
    present("initial", &mut screen, &mut show)?;
    layout(&mut screen)?;
    assert_eq!(present("identical rebuild", &mut screen, &mut show)?, 0);

    screen.set_str((2, 2), "X", Style::default());
    present("origin overwrite", &mut screen, &mut show)?;
    screen.set_str((2, 2), "Y", Style::default());
    present("incremental edit", &mut screen, &mut show)?;
    heading(&mut screen, (2, 2))?;
    present("explicit replacement", &mut screen, &mut show)?;

    screen.set_str((3, 3), "abc", Style::default());
    present("lower row overwrite", &mut screen, &mut show)?;
    heading(&mut screen, (2, 2))?;
    present("before view overwrite", &mut screen, &mut show)?;
    View::new(&mut screen, (3, 3, 1, 1)).set_cell((3, 3).into(), &Cell::new("X", 1));
    present("view overwrite", &mut screen, &mut show)?;
    heading(&mut screen, (2, 2))?;
    present("before move", &mut screen, &mut show)?;
    heading(&mut screen, (20, 2))?;
    present("move and restore base", &mut screen, &mut show)?;
    screen.remove_region("heading");
    present("remove", &mut screen, &mut show)?;

    screen.set_region(
        "captioned",
        Rect::new(2, 2, 10, 3),
        CaptionedText {
            title: "Ready",
            caption: "prepared",
        },
    )?;
    present("prepared custom region", &mut screen, &mut show)?;
    screen.set_region(
        "captioned",
        Rect::new(20, 2, 10, 3),
        CaptionedText {
            title: "Ready",
            caption: "prepared",
        },
    )?;
    present("move prepared region", &mut screen, &mut show)?;
    screen.remove_region("captioned");
    present("remove prepared region", &mut screen, &mut show)?;

    screen.set_region(
        "captioned",
        Rect::new(2, 2, 10, 3),
        CaptionedText {
            title: "Ready",
            caption: "prepared",
        },
    )?;
    present("overlay base", &mut screen, &mut show)?;
    screen.set_str((4, 4), "X", Style::default());
    screen.set_str((6, 4), "Y", Style::default());
    present("disjoint caption edits", &mut screen, &mut show)?;
    screen.set_str((4, 4), "LIVE", Style::default().bold());
    present("text over caption", &mut screen, &mut show)?;
    screen.clear_rect(Rect::new(6, 4, 2, 1));
    present("clear part of caption", &mut screen, &mut show)?;
    screen.fill_rect(Rect::new(4, 4, 2, 1), &Cell::new("+", 1));
    present("fill part of caption", &mut screen, &mut show)?;
    screen.set_str((4, 4), "??", Style::default());
    screen.insert_above("overlay log")?;
    show(
        "replay overlaid region",
        &std::mem::take(screen.writer_mut()),
    )?;
    present("pending overlay edit", &mut screen, &mut show)?;
    screen.remove_region("captioned");
    present("remove overlaid region", &mut screen, &mut show)?;

    screen.clear();
    screen.set_sized_text(
        "fraction",
        (2, 2),
        "ab",
        Style::default(),
        TextSizing {
            numerator: 1,
            denominator: 2,
            ..Default::default()
        },
    )?;
    present("width and fractional scale", &mut screen, &mut show)?;
    if inline {
        screen.set_str((0, 5), "pending", Style::default());
        for (height, name) in [
            (10, "inline grow insertion"),
            (6, "inline shrink insertion"),
        ] {
            screen.resize((size.width, height));
            screen.insert_above(name)?;
            show(name, &std::mem::take(screen.writer_mut()))?;
        }
        screen.render()?;
        show("inline resized frame", &std::mem::take(screen.writer_mut()))?;
    }
    screen.release_regions()?;
    if inline {
        screen.move_cursor_to((0, screen.size().height - 1))?;
    }
    show("cleanup", screen.writer())
}

fn main() -> io::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let dump = args.iter().any(|arg| arg == "--dump");
    let preview = args.iter().any(|arg| arg == "--preview");
    let inline = args.iter().any(|arg| arg == "--inline");
    let live = !dump && !preview;
    if args.iter().filter(|arg| arg.as_str() != "--inline").count() > 1
        || args
            .iter()
            .any(|arg| !["--live", "--preview", "--dump", "--inline"].contains(&arg.as_str()))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "use --live, --preview, or --dump; add --inline for the normal buffer",
        ));
    }
    let mut out = io::stdout().lock();
    let mut terminal = uncurses::terminal::Terminal::stdio();
    let size = if live {
        let size = terminal.get_window_size()?;
        let min_height = if inline { 10 } else { 8 };
        if size.col < 40 || size.row < min_height {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("the example needs at least 40 columns and {min_height} rows"),
            ));
        }
        terminal.make_raw()?;
        Size::new(size.col, if inline { 8 } else { size.row })
    } else {
        Size::new(40, 8)
    };
    let result = (|| {
        if live && !inline {
            Mode::ALT_SCREEN_SAVE_CURSOR.set(&mut out)?;
            out.flush()?;
        }
        demonstrate(size, inline, |name, bytes| {
            if live {
                out.write_all(bytes)?;
                out.flush()?;
                std::thread::sleep(Duration::from_millis(1500));
            } else if dump {
                write!(out, "{name}\t")?;
                for byte in bytes {
                    write!(out, "{byte:02x}")?;
                }
                writeln!(out)?;
            } else {
                writeln!(
                    out,
                    "{name}: {} bytes, {}",
                    bytes.len(),
                    bytes.escape_ascii()
                )?;
            }
            Ok(())
        })
    })();
    let restore = if live {
        if inline {
            out.write_all(b"\r\n").and_then(|()| out.flush())
        } else {
            Mode::ALT_SCREEN_SAVE_CURSOR
                .reset(&mut out)
                .and_then(|()| out.flush())
        }
    } else {
        Ok(())
    };
    let raw_restore = if live { terminal.restore() } else { Ok(()) };
    result?;
    restore?;
    raw_restore
}
