//! Release complete lines while an unfinished line stays editable.
//! Run with `--dump` to inspect the bytes without a terminal.

use std::io::{self, Write};

use uncurses::{
    buffer::{Bounded, SurfaceMut},
    program::Program,
    screen::Screen,
    style::Style,
    text::TextSurface,
};

const CHUNKS: [&str; 4] = ["First", " line.\nSecond", " line.\n", "A fresh block."];

fn write_chunk<W: Write>(screen: &mut Screen<W>, pending: &str, chunk: &str) -> io::Result<String> {
    let text = format!("{pending}{chunk}");
    let lines: Vec<_> = text.split('\n').collect();
    let completed = lines.len() - 1;
    let tail = lines[completed];
    screen.resize_height((completed + usize::from(!tail.is_empty())) as u16);
    screen.clear();
    for (row, line) in lines.iter().enumerate() {
        screen.set_str((0, row as u16), line, Style::default());
    }
    if completed > 0 {
        screen.commit_rows(completed as u16)?;
    } else {
        screen.render()?;
    }
    Ok(tail.to_owned())
}

fn main() -> io::Result<()> {
    if std::env::args().any(|arg| arg == "--dump") {
        let mut screen = Screen::new(Vec::new(), (40, 0));
        let mut pending = String::new();
        for (index, chunk) in CHUNKS.into_iter().enumerate() {
            pending = write_chunk(&mut screen, &pending, chunk)?;
            println!(
                "Chunk {}: height={}, pending={pending:?}, bytes={:?}",
                index + 1,
                screen.height(),
                String::from_utf8_lossy(screen.writer()),
            );
            assert_eq!(screen.height(), u16::from(!pending.is_empty()));
            screen.writer_mut().clear();
            screen.render()?;
            assert!(screen.writer().is_empty());
        }
        assert_eq!(pending, "A fresh block.");
        screen.commit_rows(screen.height())?;
        assert_eq!(screen.height(), 0);
        println!(
            "Final release: {:?}",
            String::from_utf8_lossy(screen.writer())
        );
        return Ok(());
    }

    let mut program = Program::stdio()?;
    program.init()?;
    program.screen_mut().resize_height(0);
    let result = (|| {
        let mut pending = String::new();
        for chunk in CHUNKS {
            pending = write_chunk(program.screen_mut(), &pending, chunk)?;
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        let height = program.screen().height();
        program.screen_mut().commit_rows(height)
    })();
    let finish = program.finish();
    result.and(finish)
}
