//! Display an image inside a [`uncurses::screen::Screen`].
//!
//! ```text
//! cargo run -p uncurses-image --example image -- <PATH> [PROTOCOL]
//! ```
//!
//! `PROTOCOL` is one of `auto` (default), `halfblocks`, `kitty`,
//! `sixel`, or `iterm2`. `auto` picks the best match for the
//! detected terminal capabilities.
//!
//! Press `q`, `Esc`, or `Ctrl-C` to quit. The image is centered and
//! resized to fit half of the terminal area.
//!
//! ```text
//! cargo run -p uncurses-image --example image --features sixel -- ferris.png sixel
//! ```

use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use uncurses::Rect;
use uncurses::SurfaceMut;
use uncurses::ansi::mode::{MouseEncoding, MouseMode};
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, Source};
use uncurses::screen::{Capabilities, Screen};
use uncurses::terminal::{disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout};
use uncurses_image::{Image, ImageLayer, ImageProtocol, Resize};

fn parse_protocol(s: &str) -> Option<ImageProtocol> {
    match s.to_ascii_lowercase().as_str() {
        "auto" => Some(ImageProtocol::Auto),
        "halfblocks" | "half" | "blocks" => Some(ImageProtocol::HalfBlocks),
        "kitty" => Some(ImageProtocol::Kitty),
        "sixel" => Some(ImageProtocol::Sixel),
        "iterm2" | "iterm" => Some(ImageProtocol::Iterm2),
        _ => None,
    }
}

fn print_usage(program: &str) {
    eprintln!(
        "usage: {program} <IMAGE_PATH> [PROTOCOL]\n\
         protocols: auto (default), halfblocks, kitty, sixel, iterm2"
    );
}

fn main() -> ExitCode {
    let mut args = env::args();
    let program = args.next().unwrap_or_else(|| "image".into());
    let Some(path) = args.next() else {
        print_usage(&program);
        return ExitCode::from(2);
    };
    let protocol = match args.next() {
        None => ImageProtocol::Auto,
        Some(s) => match parse_protocol(&s) {
            Some(p) => p,
            None => {
                eprintln!("unknown protocol: {s}");
                print_usage(&program);
                return ExitCode::from(2);
            }
        },
    };

    if let Err(e) = run(PathBuf::from(path), protocol) {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run(path: PathBuf, protocol: ImageProtocol) -> Result<(), Box<dyn std::error::Error>> {
    let image = Image::from_path(&path)?;

    let state = enable_raw_mode(stdin(), stdout())?;
    let size = get_window_size(stdout()).unwrap_or_default();
    let mut screen = Screen::new(stdout()).with_size(size.col, size.row);
    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(false)?;
    screen.set_mouse_mode(MouseMode::Normal, MouseEncoding::Sgr)?;

    let caps = Capabilities::default();
    let mut layer = ImageLayer::new(&caps).with_protocol(protocol);

    let resolved = layer.protocol();
    let id = layer.add(image);
    place_centered(&mut layer, id, &screen);

    redraw(&mut screen, &mut layer, resolved, &path)?;

    let mut events = Source::new(stdin())?;
    loop {
        let ev = events.read()?;
        match ev {
            Event::KeyPress(Key {
                code: KeyCode::Char('q') | KeyCode::Escape,
                modifiers,
                ..
            }) if modifiers.is_empty() => break,
            Event::KeyPress(Key {
                code: KeyCode::Char('c'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CTRL) => break,
            Event::Resize(ws) => {
                screen.resize(ws.col, ws.row);
                layer.invalidate();
                place_centered(&mut layer, id, &screen);
                redraw(&mut screen, &mut layer, resolved, &path)?;
            }
            _ => {}
        }
    }

    layer.shutdown(&mut screen)?;
    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state)?;
    Ok(())
}

fn place_centered<W: std::io::Write>(
    layer: &mut ImageLayer<'_>,
    id: uncurses_image::ImageId,
    screen: &Screen<W>,
) {
    let w = screen.width();
    let h = screen.height();
    // Reserve a row at the top for the status line.
    let avail_h = h.saturating_sub(2);
    let cw = (w / 2).max(4).min(w);
    let ch = (avail_h / 2).max(2).min(avail_h);
    let x = w.saturating_sub(cw) / 2;
    let y = 1 + avail_h.saturating_sub(ch) / 2;
    layer.place(
        id,
        Rect {
            x,
            y,
            width: cw,
            height: ch,
        },
        Resize::default(),
    );
}

fn redraw<W: std::io::Write>(
    screen: &mut Screen<W>,
    layer: &mut ImageLayer<'_>,
    requested: ImageProtocol,
    path: &std::path::Path,
) -> std::io::Result<()> {
    screen.clear();
    let resolved = layer.protocol();
    let header = format!(
        " {} — requested: {:?}, resolved: {:?} — q/Esc to quit ",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("<image>"),
        requested,
        resolved,
    );
    screen.set_str((0, 0), &header, uncurses::text::WrapMode::Truncate);
    layer.render(screen)?;
    screen.flush()?;
    Ok(())
}
