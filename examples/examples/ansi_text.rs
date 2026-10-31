//! Parse UTF-8 C1 escapes in text, then render ordinary terminal controls.

use std::io;

use uncurses::{
    ansi::{
        strip::strip,
        text::{Token, TokenKind, string_width, tokenize},
        wrap::hardwrap,
    },
    buffer::Surface,
    screen::Screen,
    style::{AttrFlags, Style},
    text::{Painter, TextSurface, WidthMode},
};

fn main() -> io::Result<()> {
    for input in [b"\x85".as_slice(), b"\xc2\x85"] {
        let tokens: Vec<_> = tokenize(input, WidthMode::Wc, false).collect();
        assert_eq!(
            tokens,
            [Token {
                kind: TokenKind::Control,
                bytes: input,
                width: 0,
                control: 0x85,
            }]
        );
        for token in tokens {
            println!(
                "{:?}: bytes={:02x?}, width={}, control={:02x}",
                token.kind, token.bytes, token.width, token.control,
            );
        }
    }

    for (name, csi) in [("7-bit", "\x1b["), ("UTF-8 C1", "\u{9b}")] {
        let input = format!("{csi}1mhello{csi}m");
        let width = string_width(input.as_bytes(), WidthMode::Wc, false);
        assert_eq!(width, 5);
        assert_eq!(strip(&input), "hello");
        let wrapped = hardwrap(&input, 3, true);
        assert_eq!(strip(&wrapped), "hel\nlo");

        let mut screen = Screen::new(Vec::new(), (10, 1));
        Painter::new(&mut screen).set_str((0, 0), &input, Style::default());
        assert!(
            screen
                .cell((0, 0).into())
                .unwrap()
                .style
                .attrs
                .contains(AttrFlags::BOLD)
        );
        screen.render()?;
        assert!(screen.writer().contains(&0x1b));
        assert!(!screen.writer().contains(&0xc2));
        println!(
            "{name}: width={width}, plain={:?}, wrapped={wrapped:?}",
            strip(&input)
        );
        println!(
            "Rendered bytes: {:?}",
            String::from_utf8_lossy(screen.writer())
        );

        let inserted = format!("{csi}1mxxxxxxxxxx{csi}m");
        let mut screen = Screen::new(Vec::new(), (10, 1));
        screen.insert_above(&inserted)?;
        assert_eq!(
            screen.writer(),
            b"\r\n\x1b[A\x1b[L\x1b[1mxxxxxxxxxx\x1b[m\r\n"
        );
        println!(
            "Inserted bytes (one full row): {:?}",
            String::from_utf8_lossy(screen.writer())
        );
    }
    Ok(())
}
