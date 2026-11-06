use super::*;
use uncurses::{
    buffer::{Surface, SurfaceMut},
    cell::Cell,
    layout::Position,
    style::Style,
};

fn all() -> ImageCapabilities {
    ImageCapabilities {
        kitty_placeholders: true,
        kitty: true,
        iterm2: true,
        sixel: true,
        cell_pixels: Some(Size::new(2, 4)),
    }
}

fn source() -> Image {
    Image::from_rgb(1, 1, vec![255, 0, 0]).unwrap()
}

fn screen() -> Screen<Vec<u8>> {
    let mut screen = Screen::new(Vec::new(), (20, 8));
    screen.set_color_profile(Profile::TrueColor);
    screen
}

fn draw<W: Write>(
    renderer: &mut ImageRenderer,
    screen: &mut Screen<W>,
    source: &Image,
    x: u16,
) -> ImageProtocol {
    renderer
        .draw(
            screen,
            1,
            Rect::new(x, 1, 4, 2),
            source,
            &ImageOptions::default(),
        )
        .unwrap()
}

#[test]
fn selection_uses_each_priority_and_only_usable_protocols() {
    let mut caps = all();
    let mut screen = screen();
    let area = Rect::new(1, 1, 4, 2);
    for expected in [
        ImageProtocol::KittyPlaceholders,
        ImageProtocol::Kitty,
        ImageProtocol::Iterm2,
        ImageProtocol::Sixel,
        ImageProtocol::Symbols,
    ] {
        let mut renderer = ImageRenderer::new(caps);
        assert_eq!(draw(&mut renderer, &mut screen, &source(), 1), expected);
        screen.render().unwrap();
        renderer.remove(&mut screen, 1);
        screen.render().unwrap();
        match expected {
            ImageProtocol::KittyPlaceholders => caps.kitty_placeholders = false,
            ImageProtocol::Kitty => caps.kitty = false,
            ImageProtocol::Iterm2 => caps.iterm2 = false,
            ImageProtocol::Sixel => caps.sixel = false,
            ImageProtocol::Symbols => {}
        }
    }
    for geometry in [None, Some(Size::new(0, 4)), Some(Size::new(2, 0))] {
        let renderer = ImageRenderer::new(ImageCapabilities {
            cell_pixels: geometry,
            ..all()
        });
        assert_eq!(renderer.select(&screen, area), ImageProtocol::Symbols);
    }
    let renderer = ImageRenderer::new(all());
    for profile in [
        Profile::Ansi256,
        Profile::Ansi,
        Profile::Ascii,
        Profile::Disabled,
    ] {
        screen.set_color_profile(profile);
        assert_eq!(renderer.select(&screen, area), ImageProtocol::Kitty);
    }
    screen.set_color_profile(Profile::TrueColor);
    screen.resize((300, 300));
    assert_eq!(
        renderer.select(&screen, Rect::new(0, 0, 297, 297)),
        ImageProtocol::KittyPlaceholders
    );
    for area in [Rect::new(0, 0, 298, 1), Rect::new(0, 0, 1, 298)] {
        assert_eq!(renderer.select(&screen, area), ImageProtocol::Kitty);
    }
    let renderer = ImageRenderer::new(ImageCapabilities {
        kitty: false,
        kitty_placeholders: false,
        ..all()
    });
    assert_eq!(
        renderer.select(&screen, Rect::new(0, 298, 4, 2)),
        ImageProtocol::Symbols
    );
    assert_eq!(
        renderer.select(&screen, Rect::new(0, 297, 4, 2)),
        ImageProtocol::Iterm2
    );
}

#[test]
fn iterm2_detection_accepts_supported_terminal_identities() {
    for name in [
        "iTerm2 3.5.0",
        "iTerm2(3.5.0)",
        "Rio",
        "rio 0.2.32",
        "Rio(0.2.32)",
        "WezTerm 20240203-110809-5046fc22",
        "WezTerm(20240203-110809-5046fc22)",
        "mintty 3.7.8",
        "mintty(3.7.8)",
        "Konsole 25.08.2",
        "Konsole(25.08.2)",
    ] {
        assert!(iterm2(terminal_identity(name, None, None)), "{name}");
        let mut renderer = ImageRenderer::new(ImageCapabilities {
            iterm2: iterm2(terminal_identity(name, Some("unrelated-parent"), None)),
            cell_pixels: Some(Size::new(2, 4)),
            ..Default::default()
        });
        let mut screen = screen();
        assert_eq!(
            draw(&mut renderer, &mut screen, &source(), 1),
            ImageProtocol::Iterm2
        );
        screen.render().unwrap();
        assert!(
            String::from_utf8_lossy(screen.writer()).contains("\x1b]1337;File="),
            "{name}"
        );
    }
    for term_program in ["iTerm.app", "iTerm2", "rio", "WezTerm", "mintty", "konsole"] {
        assert!(
            iterm2(terminal_identity("", Some(term_program), None)),
            "{term_program}"
        );
    }
}

#[test]
fn iterm2_detection_rejects_optional_support_and_unrelated_names() {
    for name in [
        "",
        "vscode",
        "xterm.js",
        "Tabby",
        "Hyper",
        "Warp",
        "Alacritty",
        "xterm",
        "tmux 3.4",
        "screen",
        "RioSomething",
        "WezTerminal",
        "KonsoleLike",
        "mintty-compatible",
        "iTerm22",
    ] {
        assert!(!iterm2(terminal_identity(name, None, None)), "{name}");
        assert!(!iterm2(terminal_identity("", Some(name), None)), "{name}");
        if !name.is_empty() {
            assert!(
                !iterm2(terminal_identity(name, Some("WezTerm"), None)),
                "reported identity must override inherited environment: {name}"
            );
        }
    }
    assert!(!iterm2(terminal_identity(
        "",
        Some("WezTerm 20240203"),
        None
    )));
}

#[test]
fn term_infers_image_protocols_without_query_replies() {
    for (term, native, placeholders, inline, protocol) in [
        ("xterm-kitty", true, false, false, ImageProtocol::Kitty),
        (
            "xterm-ghostty",
            true,
            true,
            false,
            ImageProtocol::KittyPlaceholders,
        ),
        (
            "xterm-rio",
            true,
            true,
            true,
            ImageProtocol::KittyPlaceholders,
        ),
        ("rio", true, true, true, ImageProtocol::KittyPlaceholders),
        ("wezterm", false, false, true, ImageProtocol::Iterm2),
    ] {
        let identity = terminal_identity("", None, Some(term));
        assert_eq!(kitty(identity), native, "{term}");
        assert_eq!(kitty_placeholders(identity), placeholders, "{term}");
        assert_eq!(iterm2(identity), inline, "{term}");
        assert_eq!(terminal_identity("", Some(""), Some(term)), identity);
        let mut renderer = ImageRenderer::new(ImageCapabilities {
            kitty: kitty(identity),
            kitty_placeholders: kitty_placeholders(identity),
            iterm2: iterm2(identity),
            cell_pixels: Some(Size::new(2, 4)),
            ..Default::default()
        });
        let mut screen = screen();
        assert_eq!(draw(&mut renderer, &mut screen, &source(), 1), protocol);
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert_eq!(output.contains("\x1b_G"), native, "{term}");
        assert_eq!(output.contains('\u{10eeee}'), placeholders, "{term}");
        assert_eq!(
            output.contains("\x1b]1337;File="),
            !native && inline,
            "{term}"
        );
    }
}

#[test]
fn term_inference_preserves_identity_precedence_and_exact_matching() {
    assert_eq!(
        terminal_identity("Ghostty 1.2.0", Some("iTerm.app"), Some("wezterm")),
        "Ghostty"
    );
    assert_eq!(
        terminal_identity("", Some("iTerm.app"), Some("xterm-kitty")),
        "iTerm.app"
    );
    for term in [
        "",
        "xterm",
        "xterm-256color",
        "screen",
        "screen-256color",
        "tmux-256color",
        "dumb",
        "xterm-kitty-extra",
        "xterm-ghostty-extra",
        "rio+base",
        "RioSomething",
        "wezterm-extra",
    ] {
        let identity = terminal_identity("", None, Some(term));
        assert!(!kitty(identity), "{term}");
        assert!(!kitty_placeholders(identity), "{term}");
        assert!(!iterm2(identity), "{term}");
    }
    for term in [
        "xterm-kitty",
        "xterm-ghostty",
        "xterm-rio",
        "rio",
        "wezterm",
    ] {
        for (name, program) in [("vscode 1.110", None), ("", Some("vscode"))] {
            let identity = terminal_identity(name, program, Some(term));
            assert_eq!(identity, "vscode");
            assert!(!kitty(identity), "{term}");
            assert!(!kitty_placeholders(identity), "{term}");
            assert!(!iterm2(identity), "{term}");
        }
        let name = "kitty(0.27.0)";
        assert_eq!(
            terminal_identity(name, Some("Ghostty"), Some(term)),
            "kitty"
        );
        assert!(!kitty_placeholders(name), "{term}");
    }
}

#[test]
fn tmux_wraps_resources_but_keeps_cells_and_lifetime_on_the_screen() {
    let mut renderer = ImageRenderer::new(ImageCapabilities {
        cell_pixels: Some(Size::new(16, 32)),
        ..all()
    });
    renderer.tmux_passthrough = true;
    let source = source();
    let mut screen = screen();
    assert_eq!(
        draw(&mut renderer, &mut screen, &source, 1),
        ImageProtocol::KittyPlaceholders
    );
    let upload = renderer.cache[&1].placement.upload.clone();
    let delete = renderer.cache[&1].placement.delete.clone();
    let upload_text = String::from_utf8_lossy(&upload);
    let chunks = upload_text.matches("\x1bPtmux;").count();
    assert!(chunks > 2);
    assert!(!upload_text.contains('\u{10eeee}'));
    assert!(delete.starts_with(b"\x1bPtmux;\x1b\x1b_Ga=d,"));
    assert!(screen.writer().is_empty());
    screen.render().unwrap();
    let output = String::from_utf8_lossy(screen.writer());
    assert_eq!(output.matches("\x1bPtmux;").count(), chunks);
    assert!(output.contains('\u{10eeee}'));
    screen.writer_mut().clear();
    screen.render().unwrap();
    assert!(screen.writer().is_empty());
    screen.clear();
    draw(&mut renderer, &mut screen, &source, 8);
    screen.render().unwrap();
    assert!(!String::from_utf8_lossy(screen.writer()).contains("\x1bPtmux;"));
    screen.writer_mut().clear();
    drop(renderer);
    screen.remove_placement(1);
    screen.render().unwrap();
    assert!(screen.writer().ends_with(&delete));
}

#[test]
fn placeholders_are_preferred_for_rio_and_ghostty() {
    for name in [
        "Rio",
        "rio 0.5.27",
        "Rio(0.5.27)",
        "Ghostty",
        "ghostty 1.2.0",
        "Ghostty(1.2.0)",
    ] {
        assert!(kitty_placeholders(name), "{name}");
        let mut renderer = ImageRenderer::new(ImageCapabilities {
            kitty_placeholders: kitty_placeholders(name),
            kitty: true,
            iterm2: iterm2(terminal_identity(name, None, None)),
            cell_pixels: Some(Size::new(2, 4)),
            ..Default::default()
        });
        let mut screen = screen();
        assert_eq!(
            draw(&mut renderer, &mut screen, &source(), 1),
            ImageProtocol::KittyPlaceholders,
            "{name}"
        );
        screen.render().unwrap();
        let output = String::from_utf8_lossy(screen.writer());
        assert!(output.contains("U=1"), "{name}");
        assert!(output.contains('\u{10eeee}'), "{name}");
        assert!(!output.contains("\x1b]1337;File="), "{name}");
    }
}

#[test]
fn sixel_uses_only_capability_parameters() {
    for attributes in [
        vec![],
        vec![None],
        vec![Some(4)],
        vec![Some(4), Some(6)],
        vec![Some(4), None],
    ] {
        assert!(!sixel(Some(&attributes)));
    }
    assert!(!sixel(None));
    for class in [
        None,
        Some(0),
        Some(1),
        Some(4),
        Some(6),
        Some(61),
        Some(62),
        Some(63),
        Some(64),
        Some(65),
        Some(u32::MAX),
    ] {
        assert!(sixel(Some(&[class, Some(4)])), "{class:?}");
        assert!(sixel(Some(&[class, None, Some(6), Some(4)])), "{class:?}");
        assert!(!sixel(Some(&[class, Some(6)])), "{class:?}");
    }
}

#[test]
fn reports_distinguish_placeholder_version() {
    for name in ["kitty(0.28.0)", "kitty(0.40.1)", "kitty(1.0.0)"] {
        assert!(kitty_placeholders(name), "{name}");
    }
    for name in [
        "kitty",
        "kitty(0.27.0)",
        "kitty(unknown)",
        "WezTerm 20240203",
        "iTerm2 3.5.0",
        "",
        "RioSomething",
        "ghostty-compatible",
        "tmux 3.4",
        "screen",
    ] {
        assert!(!kitty_placeholders(name), "{name}");
    }
}

#[test]
fn screen_owns_image_lifetime_after_the_preparation_cache_is_dropped() {
    for placeholders in [false, true] {
        let mut screen = screen();
        let mut renderer = ImageRenderer::new(ImageCapabilities {
            kitty_placeholders: placeholders,
            ..all()
        });
        draw(&mut renderer, &mut screen, &source(), 1);
        let deletion = renderer.cache[&1].placement.delete.clone();
        drop(renderer);
        assert!(screen.writer().is_empty());
        screen.render().unwrap();
        assert_eq!(
            String::from_utf8_lossy(screen.writer())
                .matches("a=t,")
                .count(),
            1
        );
        screen.writer_mut().clear();
        screen.remove_placement(1);
        screen.render().unwrap();
        assert!(screen.writer().ends_with(&deletion));
    }
}

#[test]
fn cached_uploads_survive_movement_and_deletion_follows_removal() {
    for placeholders in [false, true] {
        let mut renderer = ImageRenderer::new(ImageCapabilities {
            kitty_placeholders: placeholders,
            ..all()
        });
        let mut screen = screen();
        let source = source();
        draw(&mut renderer, &mut screen, &source, 1);
        assert!(screen.writer().is_empty());
        screen.flush().unwrap();
        assert!(screen.writer().is_empty());
        screen.render().unwrap();
        let initial = String::from_utf8_lossy(screen.writer()).into_owned();
        assert_eq!(initial.matches("a=t,").count(), 1);
        let resource = renderer.cache[&1].placement.delete.as_ref().to_vec();
        screen.writer_mut().clear();
        draw(&mut renderer, &mut screen, &source.clone(), 1);
        screen.render().unwrap();
        assert!(screen.writer().is_empty(), "{:?}", screen.writer());
        draw(&mut renderer, &mut screen, &source, 8);
        screen.render().unwrap();
        let moved = String::from_utf8_lossy(screen.writer());
        assert!(!moved.contains("a=t,"));
        assert!(!moved.contains("d=I,"));
        assert_eq!(renderer.cache[&1].placement.delete.as_ref(), resource);
        screen.writer_mut().clear();
        renderer.remove(&mut screen, 1);
        assert!(screen.writer().is_empty());
        screen.render().unwrap();
        assert!(screen.writer().ends_with(&resource));
        assert!(screen.writer().len() > resource.len());
        screen.writer_mut().clear();
        renderer.remove(&mut screen, 1);
        screen.render().unwrap();
        assert!(screen.writer().is_empty());
    }
}

#[test]
fn replacements_and_failed_preparation_preserve_options_and_frame() {
    let mut renderer = ImageRenderer::new(all());
    let mut screen = screen();
    let source = source();
    draw(&mut renderer, &mut screen, &source, 1);
    screen.render().unwrap();
    screen.writer_mut().clear();
    let old = renderer.cache[&1].placement.delete.as_ref().to_vec();
    let cell = screen.cell((1, 1).into()).unwrap().clone();
    let options = ImageOptions {
        scale: f64::NAN,
        ..Default::default()
    };
    assert!(
        renderer
            .draw(&mut screen, 1, Rect::new(8, 1, 4, 2), &source, &options)
            .is_err()
    );
    assert_eq!(screen.cell((1, 1).into()), Some(&cell));
    assert_eq!(renderer.cache[&1].placement.delete.as_ref(), old);
    assert!(screen.writer().is_empty());
    for (id, area) in [
        (0, Rect::new(1, 1, 4, 2)),
        (1, Rect::new(19, 0, 4, 2)),
        (1, Rect::new(u16::MAX, 0, 4, 2)),
        (1, Rect::new(0, 0, 0, 1)),
    ] {
        assert!(
            renderer
                .draw(&mut screen, id, area, &source, &ImageOptions::default())
                .is_err()
        );
    }
    let options = ImageOptions {
        background: [1, 2, 3],
        ..Default::default()
    };
    let saved = options.clone();
    renderer
        .draw(&mut screen, 1, Rect::new(1, 1, 4, 2), &source, &options)
        .unwrap();
    screen.render().unwrap();
    assert_eq!(saved, options);
    assert!(screen.writer().ends_with(&old));
    assert_ne!(renderer.cache[&1].placement.delete.as_ref(), old);
}

#[test]
fn removal_preserves_changed_overlays_and_clears_matching_wide_cells() {
    for caps in [all(), ImageCapabilities::default()] {
        let mut screen = screen();
        let mut renderer = ImageRenderer::new(caps);
        screen.set_eaw_wide(true);
        renderer.symbols = "u2580".parse().unwrap();
        draw(&mut renderer, &mut screen, &source(), 1);
        screen.set_str((2, 1), "TOP", Style::new().bold());
        renderer.remove(&mut screen, 1);
        for (x, text) in [(2, "T"), (3, "O"), (4, "P")] {
            assert_eq!(screen.cell((x, 1).into()).unwrap().content(), text);
        }
        for x in 1..5 {
            assert!(screen.cell((x, 2).into()).unwrap().is_blank());
            assert_eq!(screen.cell((x, 2).into()).unwrap().style, Style::default());
        }
        screen.render().unwrap();
    }
}

#[test]
fn symbols_use_screen_policy_and_reported_sampling_geometry_without_mutation() {
    let source = source();
    let mut screen = screen();
    screen.set_eaw_wide(true);
    let caps = ImageCapabilities {
        cell_pixels: Some(Size::new(3, 5)),
        ..Default::default()
    };
    let mut renderer = ImageRenderer::new(caps);
    renderer.symbols = "u2580".parse().unwrap();
    let options = ImageOptions::default();
    let saved = options.clone();
    renderer
        .draw(&mut screen, 1, Rect::new(0, 0, 4, 2), &source, &options)
        .unwrap();
    let mut sampling = options.clone();
    sampling.cell_pixels = (3, 5);
    let expected = source
        .symbols((4, 2).into(), &sampling, "u2580".parse().unwrap(), &screen)
        .unwrap();
    for y in 0..2 {
        for x in 0..4 {
            assert_eq!(screen.cell((x, y).into()), expected.cell((x, y).into()));
        }
    }
    assert_eq!(screen.cell(Position::ORIGIN).unwrap().width(), 2);
    assert_eq!(options, saved);
    screen.resize((1, 1));
    renderer.remove(&mut screen, 1);
    assert!(screen.cell(Position::ORIGIN).unwrap().is_blank());
}

#[derive(Default)]
struct Output {
    bytes: Vec<u8>,
    fail_write: bool,
    flushes: usize,
    fail_flush: Option<usize>,
}

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fail_write {
            self.bytes.extend_from_slice(&bytes[..bytes.len().min(5)]);
            return Err(io::Error::other("partial output failure"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        if self.fail_flush == Some(self.flushes) {
            Err(io::Error::other("flush failure"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn failed_output_keeps_uploads_and_deletion_pending_until_recovery() {
    for (write_failure, placeholders) in
        [(false, false), (false, true), (true, false), (true, true)]
    {
        let mut screen = Screen::new(
            Output {
                fail_write: write_failure,
                fail_flush: Some(1),
                ..Default::default()
            },
            (20, 8),
        );
        screen.set_color_profile(Profile::TrueColor);
        let mut renderer = ImageRenderer::new(ImageCapabilities {
            kitty_placeholders: placeholders,
            ..all()
        });
        draw(&mut renderer, &mut screen, &source(), 1);
        assert!(screen.render().is_err());
        screen.writer_mut().fail_write = false;
        screen.writer_mut().fail_flush = None;
        screen.writer_mut().bytes.clear();
        screen.invalidate();
        screen.render().unwrap();
        assert!(String::from_utf8_lossy(&screen.writer().bytes).contains("a=t,"));
        renderer.remove(&mut screen, 1);
        screen.writer_mut().fail_flush = Some(screen.writer().flushes + 2);
        assert!(screen.render().is_err());
        screen.writer_mut().fail_flush = None;
        screen.writer_mut().bytes.clear();
        screen.invalidate();
        renderer.remove(&mut screen, 1);
        screen.render().unwrap();
        assert!(String::from_utf8_lossy(&screen.writer().bytes).contains("d=I,"));
    }
}

#[test]
fn configuration_changes_apply_on_the_next_draw() {
    let source = source();
    let mut screen = screen();
    let mut renderer = ImageRenderer::new(all());
    draw(&mut renderer, &mut screen, &source, 1);
    screen.render().unwrap();
    screen.writer_mut().clear();
    screen.set_color_profile(Profile::Ansi256);
    assert!(screen.writer().is_empty());
    assert_eq!(
        draw(&mut renderer, &mut screen, &source, 1),
        ImageProtocol::Kitty
    );
    screen.render().unwrap();
    screen.writer_mut().clear();
    renderer.capabilities = ImageCapabilities::default();
    screen.render().unwrap();
    assert!(screen.writer().is_empty());
    assert_eq!(
        draw(&mut renderer, &mut screen, &source, 1),
        ImageProtocol::Symbols
    );
    screen.render().unwrap();
    screen.resize((2, 1));
    screen.render().unwrap();
    renderer.remove(&mut screen, 1);
    screen.render().unwrap();
}

#[test]
fn multiple_images_keep_independent_resources_and_unrendered_replacements() {
    let mut screen = screen();
    let mut renderer = ImageRenderer::new(all());
    let source = source();
    for id in [1, 2] {
        renderer
            .draw(
                &mut screen,
                id,
                Rect::new(id as u16 * 5, 1, 4, 2),
                &source,
                &ImageOptions::default(),
            )
            .unwrap();
    }
    let first = renderer.cache[&1].placement.delete.as_ref().to_vec();
    let second = renderer.cache[&2].placement.delete.as_ref().to_vec();
    assert_ne!(first, second);
    screen.render().unwrap();
    screen.writer_mut().clear();
    renderer.remove(&mut screen, 1);
    screen.render().unwrap();
    assert!(screen.writer().ends_with(&first));
    assert!(
        !screen
            .writer()
            .windows(second.len())
            .any(|bytes| bytes == second)
    );
    screen.writer_mut().clear();
    renderer
        .draw(
            &mut screen,
            2,
            Rect::new(10, 1, 3, 2),
            &source,
            &ImageOptions::default(),
        )
        .unwrap();
    let intermediate = renderer.cache[&2].placement.delete.as_ref().to_vec();
    renderer
        .draw(
            &mut screen,
            2,
            Rect::new(10, 1, 2, 2),
            &source,
            &ImageOptions::default(),
        )
        .unwrap();
    screen.render().unwrap();
    let output = String::from_utf8_lossy(screen.writer());
    assert_eq!(output.matches("a=t,").count(), 1);
    assert_eq!(output.matches("d=I,").count(), 1);
    assert!(screen.writer().ends_with(&second));
    assert!(
        !screen
            .writer()
            .windows(intermediate.len())
            .any(|bytes| bytes == intermediate)
    );
    renderer.remove(&mut screen, 2);
    screen.render().unwrap();
    assert!(renderer.cache.is_empty());
}

#[test]
fn placeholder_compression_saves_exact_metadata_bytes_and_preserves_public_cells() {
    use uncurses::screen::Optimizations;

    let mut plain = screen();
    let mut compressed = screen();
    plain.set_optimizations(Optimizations::empty());
    compressed.set_optimizations(Optimizations::empty());
    let mut renderer = ImageRenderer::new(all());
    draw(&mut renderer, &mut compressed, &source(), 1);
    let cells = renderer.cache[&1]
        .placement
        .cells
        .as_ref()
        .unwrap()
        .as_ref()
        .clone();
    let upload_len = renderer.cache[&1].placement.upload.len();
    cells.draw(&mut plain, (1, 1).into());
    plain.render().unwrap();
    compressed.render().unwrap();
    let mut saved = 0;
    for y in 0..2 {
        let anchor = cells.cell((0, y).into()).unwrap().content();
        let run = format!("{anchor}{}", "\u{10eeee}".repeat(3));
        assert!(String::from_utf8_lossy(compressed.writer()).contains(&run));
        for x in 0..4 {
            let cell = cells.cell((x, y).into()).unwrap();
            assert_eq!(compressed.cell((1 + x, 1 + y).into()), Some(cell));
            if x > 0 {
                saved += cell.content().len() - "\u{10eeee}".len();
            }
        }
    }
    assert_eq!(plain.writer(), &compressed.writer()[upload_len..]);
    let diacritics = uncurses::ansi::graphics::KITTY_PLACEHOLDER_DIACRITICS;
    let emitted = String::from_utf8_lossy(compressed.writer())
        .chars()
        .filter(|c| diacritics.contains(c))
        .map(char::len_utf8)
        .sum::<usize>();
    let complete = (0..2)
        .flat_map(|y| cells.line(y).unwrap())
        .flat_map(|cell| cell.content().chars().skip(1))
        .map(char::len_utf8)
        .sum::<usize>();
    assert_eq!(complete - emitted, saved);
    assert!(saved >= 24);
    compressed.writer_mut().clear();
    compressed.render().unwrap();
    assert!(compressed.writer().is_empty());
}

#[test]
fn partial_overlays_and_source_changes_reanchor_the_next_visible_cell() {
    let mut screen = screen();
    let mut renderer = ImageRenderer::new(all());
    draw(&mut renderer, &mut screen, &source(), 1);
    let cells = renderer.cache[&1]
        .placement
        .cells
        .as_ref()
        .unwrap()
        .as_ref()
        .clone();
    screen.render().unwrap();
    for x in 0..3 {
        screen.writer_mut().clear();
        screen.set_str((1 + x, 2), "X", Style::EMPTY);
        screen.render().unwrap();
        let next = cells.cell((x + 1, 1).into()).unwrap();
        assert!(String::from_utf8_lossy(screen.writer()).contains(next.content()));
        assert_eq!(screen.cell((x + 2, 2).into()), Some(next));
        assert_eq!(screen.cell((x + 1, 2).into()).unwrap().content(), "X");
    }
    cells.draw(&mut screen, (1, 1).into());
    screen.render().unwrap();
    screen.writer_mut().clear();
    screen.set_cell((1, 2), cells.cell((2, 0).into()).unwrap());
    screen.render().unwrap();
    let next = cells.cell((1, 1).into()).unwrap();
    assert!(String::from_utf8_lossy(screen.writer()).contains(next.content()));
    assert_eq!(screen.cell((1, 2).into()), cells.cell((2, 0).into()));
    assert_eq!(screen.cell((2, 2).into()), Some(next));
    screen.writer_mut().clear();
    screen.render().unwrap();
    assert!(screen.writer().is_empty());
}

#[test]
fn failed_output_restores_full_placeholder_cells_before_recovery_or_removal() {
    let mut screen = Screen::new(
        Output {
            fail_write: true,
            ..Default::default()
        },
        (20, 8),
    );
    screen.set_color_profile(Profile::TrueColor);
    let mut renderer = ImageRenderer::new(all());
    draw(&mut renderer, &mut screen, &source(), 1);
    let cells = renderer.cache[&1]
        .placement
        .cells
        .as_ref()
        .unwrap()
        .as_ref()
        .clone();
    assert!(screen.render().is_err());
    for y in 0..2 {
        for x in 0..4 {
            assert_eq!(
                screen.cell((x + 1, y + 1).into()),
                cells.cell((x, y).into())
            );
        }
    }
    screen.writer_mut().fail_write = false;
    screen.invalidate();
    screen.render().unwrap();
    renderer.remove(&mut screen, 1);
    screen.render().unwrap();
    for y in 0..2 {
        for x in 0..4 {
            assert_eq!(screen.cell((x + 1, y + 1).into()), Some(&Cell::BLANK));
        }
    }
}
