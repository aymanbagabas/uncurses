//! Styling showcase: SGR attributes, colors, and OSC 8 hyperlinks.
//!
//! Writes a series of styled lines straight to stdout through the `style`
//! package, with no raw mode or alternate screen. Each [`Style`] is built
//! fluently and rendered with its [`styled`](Style::styled) `Display`
//! adapter, which emits the SGR sequence, the text, then a reset.
//!
//! Run with `cargo run --example styles`. Truecolor and fancy underlines
//! need a capable terminal; terminals that ignore a sequence simply render
//! the text unstyled.

use std::io::{self, Write};

use uncurses::ansi::hyperlink::{write_hyperlink_end, write_hyperlink_start};
use uncurses::color::{BasicColor, Color};
use uncurses::style::{Style, UnderlineStyle};

fn main() -> io::Result<()> {
    let mut out = io::stdout().lock();

    section(&mut out, "Attributes")?;
    writeln!(out, "  {}", Style::default().bold().styled("bold"))?;
    writeln!(out, "  {}", Style::default().italic().styled("italic"))?;
    writeln!(
        out,
        "  {}",
        Style::default().underline().styled("underline")
    )?;
    writeln!(
        out,
        "  {}",
        Style::default().strikethrough().styled("strikethrough")
    )?;
    writeln!(out, "  {}", Style::default().reverse().styled("reverse"))?;

    section(&mut out, "Underline styles")?;
    for (name, kind) in [
        ("single", UnderlineStyle::Single),
        ("double", UnderlineStyle::Double),
        ("curly", UnderlineStyle::Curly),
        ("dotted", UnderlineStyle::Dotted),
        ("dashed", UnderlineStyle::Dashed),
    ] {
        let style = Style::default()
            .underline()
            .underline_style(kind)
            .underline_color(BasicColor::BrightRed);
        writeln!(out, "  {}", style.styled(name))?;
    }

    section(&mut out, "Colors")?;
    writeln!(
        out,
        "  {}",
        Style::default()
            .fg(BasicColor::Green)
            .styled("basic green (16-color)")
    )?;
    writeln!(
        out,
        "  {}",
        Style::default()
            .fg(Color::Indexed(208))
            .styled("indexed 208 (256-color)")
    )?;
    writeln!(
        out,
        "  {}",
        Style::default()
            .fg(Color::Rgb(255, 105, 180))
            .styled("rgb(255,105,180) (truecolor)")
    )?;
    writeln!(
        out,
        "  {}",
        Style::default()
            .fg(BasicColor::White)
            .bg(BasicColor::Blue)
            .bold()
            .styled(" foreground on background ")
    )?;

    section(&mut out, "Hyperlink (OSC 8)")?;
    // The cell-based renderer emits OSC 8 automatically for any cell whose
    // Style carries a `link(...)`. Writing straight to stdout (no cells),
    // wrap the text in the hyperlink sequence by hand and style it with SGR.
    let url = "https://github.com/aymanbagabas/uncurses";
    let style = Style::default().underline().fg(BasicColor::BrightBlue);
    write!(out, "  ")?;
    write_hyperlink_start(&mut out, url, "")?;
    write!(out, "{}", style.styled("uncurses on GitHub"))?;
    write_hyperlink_end(&mut out)?;
    writeln!(out, " (Ctrl/Cmd-click in a supporting terminal)")?;

    out.flush()
}

fn section(out: &mut impl Write, title: &str) -> io::Result<()> {
    let heading = Style::default().bold().fg(BasicColor::BrightCyan);
    writeln!(out, "\n{}", heading.styled(title))
}
