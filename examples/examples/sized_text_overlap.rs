//! Compare OSC 66 overwrite rules directly in a compatible terminal.
//!
//! Run in Kitty with an 80-by-16 or larger viewport. Space switches between
//! the original blocks and three overlay operations. --dump emits both frames
//! for replay through a native terminal parser.

use std::io::{self, Write};

use uncurses::ansi::{
    cursor::write_cup,
    text_sizing::{TextSizing, write_sized_text},
};
use uncurses::event::Event;
use uncurses::program::{Program, ProgramOptions};

const BLOCK_ROW: u16 = 5;
const BLOCK_COLUMNS: [u16; 3] = [4, 30, 56];

fn draw<W: Write>(out: &mut W, after: bool) -> io::Result<()> {
    out.write_all(b"\x1b[0m\x1b[2J\x1b[H")?;
    writeln!(out, "OSC 66 overwrite rules (requires Kitty text sizing)")?;
    write_cup(out, 1, 0)?;
    out.write_all(if after {
        b"AFTER: each panel tried to write X at its marked column."
    } else {
        b"BEFORE: three identical 8-by-2 sized-text blocks."
    })?;
    for (index, column) in BLOCK_COLUMNS.into_iter().enumerate() {
        write_cup(out, 3, column - 2)?;
        out.write_all(
            [
                b"1. Write in lower row".as_slice(),
                b"2. Write in top row".as_slice(),
                b"3. Erase, then write".as_slice(),
            ][index],
        )?;
        write_cup(out, BLOCK_ROW, column)?;
        out.write_all(b"\x1b[1;38;2;255;255;255;48;2;0;0;0m")?;
        write_sized_text(
            out,
            "Demo",
            TextSizing {
                scale: 2,
                width: 4,
                ..TextSizing::default()
            },
        )?;
        out.write_all(b"\x1b[0m")?;
        write_cup(out, 8, column + 1)?;
        out.write_all(b"^")?;
        write_cup(out, 9, column - 2)?;
        write!(out, "Requested column {}", column + 2)?;
    }
    if after {
        write_cup(out, BLOCK_ROW + 1, BLOCK_COLUMNS[0] + 1)?;
        out.write_all(b"X")?;
        write_cup(out, BLOCK_ROW, BLOCK_COLUMNS[1] + 1)?;
        out.write_all(b"X")?;
        // Erasing the top-left cell removes the entire multicell block.
        write_cup(out, BLOCK_ROW, BLOCK_COLUMNS[2])?;
        out.write_all(b"\x1b[X")?;
        write_cup(out, BLOCK_ROW + 1, BLOCK_COLUMNS[2] + 1)?;
        out.write_all(b"X")?;
    }
    for (column, result) in [
        (2, "X skips past Demo."),
        (28, "X replaces all of Demo."),
        (54, "X stays at its target."),
    ] {
        write_cup(out, 11, column)?;
        out.write_all(if after { result.as_bytes() } else { b"" })?;
    }
    write_cup(out, 14, 0)?;
    out.write_all(b"Space: before/after    R: replay after    Q/Esc: quit")?;
    out.flush()
}

fn main() -> io::Result<()> {
    let mut dump = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--dump" => dump = true,
            "--help" | "-h" => {
                println!(
                    "sized_text_overlap [--dump]\nRun in Kitty with at least 80 columns and 16 rows."
                );
                return Ok(());
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown argument: {argument}"),
                ));
            }
        }
    }
    if dump {
        let mut output = io::stdout().lock();
        for (name, after) in [("before", false), ("after", true)] {
            let mut bytes = Vec::new();
            draw(&mut bytes, after)?;
            write!(output, "{name}\t")?;
            for byte in bytes {
                write!(output, "{byte:02x}")?;
            }
            writeln!(output)?;
        }
        return Ok(());
    }
    let mut program = Program::stdio()?;
    let result = (|| {
        program.init_with(ProgramOptions {
            bracketed_paste: false,
            ..ProgramOptions::default()
        })?;
        if program.screen().size().width < 80 || program.screen().size().height < 16 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the example needs at least 80 columns and 16 rows",
            ));
        }
        program.enter_alt_screen()?;
        program.hide_cursor()?;
        let mut after = true;
        draw(program.screen_mut().writer_mut(), after)?;
        loop {
            match program.read_event()? {
                Event::KeyPress(key) if key.matches_any(["q", "esc", "ctrl+c"]) => break,
                Event::KeyPress(key) if key.matches_any(["space", "r"]) => {
                    after = if key.matches_any(["r"]) { true } else { !after };
                    draw(program.screen_mut().writer_mut(), after)?;
                }
                _ => {}
            }
        }
        Ok(())
    })();
    result.and(program.finish())
}
