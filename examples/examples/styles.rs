//! Styling showcase: SGR attributes, colors, and OSC 8 hyperlinks.
//!
//! Writes styled lines straight to stdout with no raw mode or alternate
//! screen. Each [`Style`] renders through its [`Display`] to a bare SGR opener
//! (no trailing reset), and an empty [`Style`] renders the SGR reset. So a
//! style acts as an opening sequence and `Style::default()` as the matching
//! closing one, dropped into ordinary [`writeln!`] calls like any other value:
//! `writeln!(out, "{open}text{reset}")`.
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
    // An empty style renders the SGR reset: the universal closing sequence.
    let reset = Style::default();

    section(&mut out, "Attributes")?;
    writeln!(out, "  {}bold{reset}", Style::default().bold())?;
    writeln!(out, "  {}italic{reset}", Style::default().italic())?;
    writeln!(out, "  {}underline{reset}", Style::default().underline())?;
    writeln!(
        out,
        "  {}strikethrough{reset}",
        Style::default().strikethrough()
    )?;
    writeln!(out, "  {}reverse{reset}", Style::default().reverse())?;

    section(&mut out, "Underline styles")?;
    for (name, kind) in [
        ("single", UnderlineStyle::Single),
        ("double", UnderlineStyle::Double),
        ("curly", UnderlineStyle::Curly),
        ("dotted", UnderlineStyle::Dotted),
        ("dashed", UnderlineStyle::Dashed),
    ] {
        let open = Style::default()
            .underline()
            .underline_style(kind)
            .underline_color(BasicColor::BrightRed);
        writeln!(out, "  {open}{name}{reset}")?;
    }

    section(&mut out, "Colors")?;
    let green = Style::default().fg(BasicColor::Green);
    writeln!(out, "  {green}basic green (16-color){reset}")?;
    let indexed = Style::default().fg(Color::Indexed(208));
    writeln!(out, "  {indexed}indexed 208 (256-color){reset}")?;
    let truecolor = Style::default().fg(Color::Rgb(255, 105, 180));
    writeln!(out, "  {truecolor}rgb(255,105,180) (truecolor){reset}")?;
    let on_blue = Style::default()
        .fg(BasicColor::White)
        .bg(BasicColor::Blue)
        .bold();
    writeln!(out, "  {on_blue} foreground on background {reset}")?;

    section(&mut out, "Hyperlink (OSC 8)")?;
    // The cell renderer emits OSC 8 automatically for any cell whose Style
    // carries a `link(...)`. Writing straight to stdout (no cells), wrap the
    // text in the hyperlink sequence by hand and open/close the SGR around it.
    let url = "https://github.com/aymanbagabas/uncurses";
    let link = Style::default().underline().fg(BasicColor::BrightBlue);
    write!(out, "  ")?;
    write_hyperlink_start(&mut out, url, "")?;
    write!(out, "{link}uncurses on GitHub{reset}")?;
    write_hyperlink_end(&mut out)?;
    writeln!(out, " (Ctrl/Cmd-click in a supporting terminal)")?;

    out.flush()
}

fn section(out: &mut impl Write, title: &str) -> io::Result<()> {
    let heading = Style::default().bold().fg(BasicColor::BrightCyan);
    let reset = Style::default();
    writeln!(out, "\n{heading}{title}{reset}")
}
