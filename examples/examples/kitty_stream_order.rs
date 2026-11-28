//! Compare text before and after Kitty image placement, then delete the image.
//!
//! Run in Kitty with an 80-by-24 or larger viewport. Space advances the
//! experiment. --dump emits the same three stages for native parser replay.

use std::io::{self, Write};

use uncurses::ansi::{cursor::write_cup, graphics::write_kitty_graphics};
use uncurses::event::Event;
use uncurses::program::{Program, ProgramOptions};

const PANELS: [(u16, u16, Option<i32>, &str); 4] = [
    (4, 5, None, "z omitted"),
    (42, 5, Some(0), "z = 0"),
    (4, 14, Some(-1), "z = -1"),
    (42, 14, Some(-1_073_741_825), "z = -1073741825"),
];
const STAGES: [&str; 3] = [
    "1. BEFORE IMAGE text, then image placement",
    "2. AFTER IMAGE text added; images unchanged",
    "3. Images deleted; text cells unchanged",
];

fn delete_image(out: &mut impl Write, image: u32) -> io::Result<()> {
    write_kitty_graphics(out, &["a=d", "d=I", &format!("i={image}"), "q=2"], &[])
}

fn text(out: &mut impl Write, x: u16, y: u16, label: &str) -> io::Result<()> {
    write_cup(out, y, x)?;
    write!(out, "\x1b[1;38;2;255;255;255;48;2;0;0;0m{label}\x1b[0m")
}

fn draw(out: &mut impl Write, image: u32, stage: usize) -> io::Result<()> {
    out.write_all(b"\x1b7")?;
    if stage == 0 {
        delete_image(out, image)?;
        out.write_all(b"\x1b[0m\x1b[2J\x1b[HKitty graphics: stream order and z-index")?;
        // Each /0AA group is one opaque RGB pixel: (255, 64, 0).
        write_kitty_graphics(
            out,
            &["a=t", "f=24", "s=30", "v=8", &format!("i={image}"), "q=2"],
            &b"/0AA".repeat(30 * 8),
        )?;
        for (index, (x, y, z, label)) in PANELS.into_iter().enumerate() {
            write_cup(out, y - 2, x)?;
            out.write_all(label.as_bytes())?;
            text(out, x + 7, y + 1, "BEFORE IMAGE")?;
            write_cup(out, y, x)?;
            let mut options = format!("a=p,i={image},p={},c=30,r=4,C=1,q=2", index + 1);
            if let Some(z) = z {
                options.push_str(&format!(",z={z}"));
            }
            write_kitty_graphics(out, &[&options], &[])?;
        }
        write_cup(out, 20, 0)?;
        out.write_all(b"White text has an explicit black background. Orange pixels are opaque.")?;
        write_cup(out, 22, 0)?;
        out.write_all(b"Space: next stage    R: restart    Q/Esc: quit")?;
    } else if stage == 1 {
        for (x, y, _, _) in PANELS {
            text(out, x + 7, y + 2, "AFTER IMAGE")?;
        }
    } else {
        delete_image(out, image)?;
    }
    write_cup(out, 1, 0)?;
    write!(out, "\x1b[0m\x1b[K{}", STAGES[stage])?;
    out.write_all(b"\x1b8")?;
    out.flush()
}

fn main() -> io::Result<()> {
    let mut dump = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--dump" => dump = true,
            "--help" | "-h" => {
                println!(
                    "kitty_stream_order [--dump]\nRun in Kitty with at least 80 columns and 24 rows."
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
    let image = 0x6000_0000 | std::process::id();
    if dump {
        let mut output = io::stdout().lock();
        for (stage, name) in ["image", "text", "deleted"].into_iter().enumerate() {
            let mut bytes = Vec::new();
            draw(&mut bytes, image, stage)?;
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
        let size = program.screen().size();
        if size.width < 80 || size.height < 24 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the example needs at least 80 columns and 24 rows",
            ));
        }
        program.enter_alt_screen()?;
        program.hide_cursor()?;
        let mut stage = 0;
        draw(program.screen_mut(), image, stage)?;
        loop {
            match program.read_event()? {
                Event::KeyPress(key) if key.matches_any(["q", "esc", "ctrl+c"]) => break,
                Event::KeyPress(key) if key.matches_any(["space", "r"]) => {
                    stage = if key.matches_any(["r"]) {
                        0
                    } else {
                        (stage + 1) % 3
                    };
                    draw(program.screen_mut(), image, stage)?;
                }
                _ => {}
            }
        }
        Ok(())
    })();
    let cleanup =
        delete_image(program.screen_mut(), image).and_then(|()| program.screen_mut().flush());
    result.and(cleanup).and(program.finish())
}
