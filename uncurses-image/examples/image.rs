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
//! At startup the example writes a capability probe to the terminal,
//! drains the reply events into a [`Capabilities`] snapshot, and only
//! then constructs the image layer — so `auto` resolution sees the
//! true terminal answer (e.g. iTerm2 graphics confirmed via XTVERSION
//! rather than just env-detected). Late capability replies that
//! arrive after the first frame are folded back into the snapshot
//! and trigger a redraw.
//!
//! Press `q`, `Esc`, or `Ctrl-C` to quit. The image is centered and
//! resized to fit half of the terminal area.

use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use uncurses::Rect;
use uncurses::SurfaceMut;
use uncurses::ansi::mode::{MouseEncoding, MouseMode};
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, Source};
use uncurses::screen::{Capabilities, Feature, Screen};
use uncurses::terminal::{
    Stdin, Stdout, disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout,
};
use uncurses_image::{Image, ImageLayer, ImageProtocol, Resize};

const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const FRAME_POLL: Duration = Duration::from_millis(50);

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

    let mut events = Source::new(stdin())?;
    let mut caps = Capabilities::default();
    probe_capabilities(&mut caps, &mut screen, &mut events)?;

    let mut layer = ImageLayer::new().with_protocol(protocol);
    let id = layer.add(image);
    place_centered(&mut layer, id, &screen);

    redraw(&mut screen, &mut layer, &caps, protocol, &path)?;

    let result = event_loop(
        &mut screen,
        &mut layer,
        &mut events,
        &mut caps,
        id,
        protocol,
        &path,
    );

    layer.shutdown(&mut screen)?;
    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin(), stdout(), &state)?;
    result
}

/// Send a capability probe and drain the replies into `caps`. The
/// probe terminates either when DA1 (the natural probe terminator)
/// arrives or when [`PROBE_TIMEOUT`] elapses.
fn probe_capabilities(
    caps: &mut Capabilities,
    screen: &mut Screen<Stdout>,
    events: &mut Source<Stdin>,
) -> std::io::Result<()> {
    Feature::default().write_probe(screen.writer_mut())?;
    screen.writer_mut().flush()?;

    let deadline = std::time::Instant::now() + PROBE_TIMEOUT;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if !events.poll(Some(remaining))? {
            break;
        }
        while let Some(ev) = events.try_read() {
            caps.update(&ev);
            if matches!(ev, Event::PrimaryDeviceAttributes(_)) {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn event_loop(
    screen: &mut Screen<Stdout>,
    layer: &mut ImageLayer,
    events: &mut Source<Stdin>,
    caps: &mut Capabilities,
    id: uncurses_image::ImageId,
    requested: ImageProtocol,
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        if !events.poll(Some(FRAME_POLL))? {
            continue;
        }
        let mut dirty = false;
        let mut quit = false;
        while let Some(ev) = events.try_read() {
            // Late capability replies (e.g. an XTVERSION that arrived
            // after the probe deadline) update the snapshot and force
            // a redraw with the new resolved protocol. Invalidate the
            // screen too: raster image bursts emitted before the new
            // caps were known may have left pixels burned onto the
            // terminal canvas that a normal cell-diff repaint won't
            // wipe — a full clear-screen on the next render does.
            if caps.update(&ev) {
                layer.invalidate();
                screen.invalidate();
                dirty = true;
                continue;
            }
            match ev {
                Event::KeyPress(Key {
                    code: KeyCode::Char('q') | KeyCode::Escape,
                    modifiers,
                    ..
                }) if modifiers.is_empty() => quit = true,
                Event::KeyPress(Key {
                    code: KeyCode::Char('c'),
                    modifiers,
                    ..
                }) if modifiers.contains(KeyModifiers::CTRL) => quit = true,
                Event::Resize(ws) => {
                    screen.resize(ws.col, ws.row);
                    layer.invalidate();
                    place_centered(layer, id, screen);
                    dirty = true;
                }
                _ => {}
            }
        }
        if quit {
            return Ok(());
        }
        if dirty {
            redraw(screen, layer, caps, requested, path)?;
        }
    }
}

/// Place the image centered inside the screen. The placement area is
/// capped at a fixed maximum so the image stays clearly inside the
/// terminal regardless of the window size — useful for the example
/// where we want a deterministic visible result rather than the image
/// stretching across the full terminal.
fn place_centered<W: Write>(
    layer: &mut ImageLayer,
    id: uncurses_image::ImageId,
    screen: &Screen<W>,
) {
    const MAX_W: u16 = 40;
    const MAX_H: u16 = 20;

    let w = screen.width();
    let h = screen.height();
    // Reserve a row at the top for the status line.
    let avail_h = h.saturating_sub(2);
    let cw = (w / 2).clamp(4, MAX_W).min(w);
    let ch = (avail_h / 2).clamp(2, MAX_H).min(avail_h);
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

fn redraw<W: Write>(
    screen: &mut Screen<W>,
    layer: &mut ImageLayer,
    caps: &Capabilities,
    requested: ImageProtocol,
    path: &std::path::Path,
) -> std::io::Result<()> {
    screen.clear();
    let resolved = layer.protocol(caps);
    let header = format!(
        " {} — requested: {:?}, resolved: {:?} — q/Esc to quit ",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("<image>"),
        requested,
        resolved,
    );
    screen.set_str((0, 0), &header, uncurses::text::WrapMode::Truncate);
    layer.render(caps, screen)?;
    screen.flush()?;
    Ok(())
}
