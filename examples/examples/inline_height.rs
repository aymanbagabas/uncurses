//! Change an inline region's height while preserving unchanged rows.

use std::io;

use uncurses::{screen::Screen, style::Style, text::TextSurface};

fn main() -> io::Result<()> {
    let mut screen = Screen::new(Vec::new(), (40, 1));
    screen.set_str((0, 0), "First row", Style::default());
    screen.render()?;
    screen.writer_mut().clear();

    screen.resize_height(2);
    screen.set_str((0, 1), "Second row", Style::default());
    screen.render()?;
    let added = String::from_utf8_lossy(screen.writer());
    assert!(added.contains("Second row"));
    assert!(!added.contains("First row"));
    println!("Grow: {added:?}");
    screen.writer_mut().clear();

    screen.resize_height(1);
    screen.set_str((9, 0), "!", Style::default());
    screen.insert_above("Log before shrink")?;
    let inserted = String::from_utf8_lossy(screen.writer());
    assert_eq!(inserted, "\r\n\x1b[2A\x1b[LLog before shrink\x1b[K\r\n");
    assert!(!inserted.contains('!'));
    println!("Insert before shrink: {inserted:?}");
    screen.writer_mut().clear();

    screen.render()?;
    let removed = String::from_utf8_lossy(screen.writer());
    assert!(removed.contains("\x1b[J"));
    assert!(!removed.contains("First row"));
    assert!(removed.contains('!'));
    println!("Shrink: {removed:?}");
    screen.writer_mut().clear();

    screen.resize_height(1);
    screen.render()?;
    assert!(screen.writer().is_empty());
    println!("Same height: zero bytes");
    Ok(())
}
