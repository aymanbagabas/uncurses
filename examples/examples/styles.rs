//! Styling showcase: SGR attributes, colors, and OSC 8 hyperlinks.
//!
//! Writes styled lines straight to stdout with no raw mode or alternate
//! screen. Each line follows the opener/closer pattern: a [`Style`] renders
//! through its [`Display`] to the SGR (and optional OSC 8) opener, and
//! [`Style::reset`] renders the matching closer. Both drop into an ordinary
//! [`writeln!`] like any other value: `writeln!(out, "{open}text{close}")`.
//!
//! Run with `cargo run --example styles`. Truecolor and fancy underlines
//! need a capable terminal; terminals that ignore a sequence simply render
//! the text unstyled.

use std::io::{self, Write};

use uncurses::color::{BasicColor, Color};
use uncurses::style::{Style, UnderlineStyle};

fn main() -> io::Result<()> {
    let mut out = io::stdout().lock();

    section(&mut out, "Attributes")?;
    show(&mut out, "bold", Style::new().bold())?;
    show(&mut out, "italic", Style::new().italic())?;
    show(&mut out, "underline", Style::new().underline())?;
    show(&mut out, "strikethrough", Style::new().strikethrough())?;
    show(&mut out, "reverse", Style::new().reverse())?;

    section(&mut out, "Underline styles")?;
    for (name, kind) in [
        ("single", UnderlineStyle::Single),
        ("double", UnderlineStyle::Double),
        ("curly", UnderlineStyle::Curly),
        ("dotted", UnderlineStyle::Dotted),
        ("dashed", UnderlineStyle::Dashed),
    ] {
        let style = Style::new()
            .underline()
            .underline_style(kind)
            .underline_color(BasicColor::BrightRed);
        show(&mut out, name, style)?;
    }

    section(&mut out, "Colors")?;
    show(
        &mut out,
        "basic green (16-color)",
        Style::new().fg(BasicColor::Green),
    )?;
    show(
        &mut out,
        "indexed 208 (256-color)",
        Style::new().fg(Color::Indexed(208)),
    )?;
    show(
        &mut out,
        "rgb(255,105,180) (truecolor)",
        Style::new().fg(Color::Rgb(255, 105, 180)),
    )?;
    show(
        &mut out,
        " foreground on background ",
        Style::new()
            .fg(BasicColor::White)
            .bg(BasicColor::Blue)
            .bold(),
    )?;

    section(&mut out, "Hyperlink (OSC 8)")?;
    // A `link(...)` makes the opener emit the OSC 8 start and the closer emit
    // its terminator, so the same opener/closer pattern that styles text also
    // makes it clickable.
    let url = "https://github.com/aymanbagabas/uncurses";
    let link = Style::new()
        .underline()
        .fg(BasicColor::BrightBlue)
        .link(url, "");
    writeln!(
        out,
        "  {link}uncurses on GitHub{} (Ctrl/Cmd-click in a supporting terminal)",
        link.reset()
    )?;

    out.flush()
}

/// Write `label` wrapped in `style`'s opener and closer.
///
/// `style` renders the opener through its `Display`; `style.reset()` renders
/// the matching closer through the `Display` of the returned value.
fn show(out: &mut impl Write, label: &str, style: Style) -> io::Result<()> {
    writeln!(out, "  {style}{label}{}", style.reset())
}

fn section(out: &mut impl Write, title: &str) -> io::Result<()> {
    let heading = Style::new().bold().fg(BasicColor::BrightCyan);
    writeln!(out, "\n{heading}{title}{}", heading.reset())
}
