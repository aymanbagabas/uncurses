//! Animated tour through several rendering features: a centered framed
//! viewport runs through six short scenes — random sprinkles, nested
//! coloured panels, line-by-line art, a styled banner with a hyperlink
//! and the five underline styles, a marquee, and bouncing balls. The
//! sequence loops; press `q`, `Q`, `Esc`, or `Ctrl-C` at a prompt to
//! exit. Any other key skips ahead.
//!
//! The banner scene shows: an OSC 8 hyperlink wrapped around a label,
//! a curly red underline on a misspelled-looking word, plus single,
//! double, dotted, and dashed underlines side-by-side.

use std::io::Write;
use std::time::{Duration, Instant};

use uncurses::SurfaceMut;
use uncurses::cell::Cell;
use uncurses::color::{BasicColor, Color};
use uncurses::event::{Event, Key, KeyCode, KeyModifiers, Source};
use uncurses::layout::Position;
use uncurses::screen::Screen;
use uncurses::style::{Style, UnderlineStyle};
use uncurses::terminal::{
    Stdin, disable_raw_mode, enable_raw_mode, get_window_size, stdin, stdout,
};
use uncurses::text::WrapMode;

const BOX_W: u16 = 56;
const BOX_H: u16 = 16;

const FRAME: Duration = Duration::from_millis(33);
const LINK_URL: &str = "https://github.com/aymanbagabas/uncurses";

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn range(&mut self, n: u32) -> u32 {
        (self.next_u64() as u32) % n.max(1)
    }
}

#[derive(Clone, Copy)]
struct Anchor {
    x: u16,
    y: u16,
}

fn anchor<W: Write>(screen: &Screen<W>) -> Anchor {
    let w = screen.width();
    let h = screen.height();
    Anchor {
        x: w.saturating_sub(BOX_W) / 2,
        y: h.saturating_sub(BOX_H) / 2,
    }
}

fn paint_blank<W: Write>(screen: &mut Screen<W>) {
    screen.clear();
}

fn draw_box<W: Write>(screen: &mut Screen<W>, a: Anchor, style: &Style) {
    let (x0, y0) = (a.x, a.y);
    let (x1, y1) = (a.x + BOX_W - 1, a.y + BOX_H - 1);

    let mut top = String::with_capacity(BOX_W as usize);
    top.push('┌');
    for _ in 0..BOX_W - 2 {
        top.push('─');
    }
    top.push('┐');
    let mut bot = String::with_capacity(BOX_W as usize);
    bot.push('└');
    for _ in 0..BOX_W - 2 {
        bot.push('─');
    }
    bot.push('┘');

    screen.set_str_with((x0, y0), &top, WrapMode::Truncate, style.clone());
    screen.set_str_with((x0, y1), &bot, WrapMode::Truncate, style.clone());
    for y in y0 + 1..y1 {
        screen.set_str_with((x0, y), "│", WrapMode::Truncate, style.clone());
        screen.set_str_with((x1, y), "│", WrapMode::Truncate, style.clone());
    }
}

fn fill_inside<W: Write>(screen: &mut Screen<W>, a: Anchor, style: &Style) {
    let cell = Cell::narrow(" ").style(style.clone());
    for y in a.y + 1..a.y + BOX_H - 1 {
        for x in a.x + 1..a.x + BOX_W - 1 {
            screen.set_cell(Position::new(x, y), &cell);
        }
    }
}

fn write<W: Write>(screen: &mut Screen<W>, x: u16, y: u16, s: &str, style: &Style) {
    screen.set_str_with((x, y), s, WrapMode::Truncate, style.clone());
}

fn write_link<W: Write>(screen: &mut Screen<W>, x: u16, y: u16, s: &str, style: &Style, url: &str) {
    screen.set_str_with((x, y), s, WrapMode::Truncate, style.clone().link(url, ""));
}

fn footer<W: Write>(screen: &mut Screen<W>, a: Anchor, hint: &str) {
    let dim = Style::default().fg(BasicColor::BrightBlack.into());
    let label_w = hint.chars().count() as i32;
    let center = a.x as i32 + BOX_W as i32 / 2;
    let lx = (center - label_w / 2).max(0) as u16;
    write(screen, lx, a.y + BOX_H, hint, &dim);
}

/// Drive a scene until the user presses a key or `dur` elapses (when
/// `dur` is `Some`). With `dur = None` the scene runs until a keypress.
/// Returns `Ok(true)` to keep going, `Ok(false)` when the user asked
/// to quit. Any non-quit key advances early.
fn run_scene<W: Write>(
    screen: &mut Screen<W>,
    events: &mut Source<Stdin>,
    dur: Option<Duration>,
    mut tick: impl FnMut(&mut Screen<W>, Duration) -> std::io::Result<()>,
) -> std::io::Result<bool> {
    while events.try_read().is_some() {}
    let start = Instant::now();
    let end = dur.map(|d| start + d);
    let mut next_frame = start + FRAME;
    tick(screen, Duration::ZERO)?;
    screen.render()?;
    screen.flush()?;

    loop {
        let now = Instant::now();
        if let Some(end) = end
            && now >= end
        {
            return Ok(true);
        }
        let frame_remaining = next_frame.saturating_duration_since(now);
        let timeout = match end {
            Some(end) => frame_remaining.min(end - now),
            None => frame_remaining,
        };
        if events.poll(Some(timeout))? {
            while let Some(ev) = events.try_read() {
                match ev {
                    Event::KeyPress(Key {
                        code: KeyCode::Char('q' | 'Q') | KeyCode::Escape,
                        modifiers,
                        ..
                    }) if modifiers.is_empty() => return Ok(false),
                    Event::KeyPress(Key {
                        code: KeyCode::Char('c'),
                        modifiers,
                        ..
                    }) if modifiers.contains(KeyModifiers::CTRL) => return Ok(false),
                    Event::KeyPress(_) => return Ok(true),
                    Event::Resize(ws) => {
                        screen.resize(ws.col, ws.row);
                    }
                    _ => {}
                }
            }
        }
        let now = Instant::now();
        if now >= next_frame {
            next_frame += FRAME;
            if next_frame < now {
                next_frame = now + FRAME;
            }
            tick(screen, now - start)?;
            screen.render()?;
            screen.flush()?;
        }
    }
}

fn scene_sprinkles<W: Write>(
    screen: &mut Screen<W>,
    events: &mut Source<Stdin>,
) -> std::io::Result<bool> {
    let mut rng = Rng::new(0x9E37_79B9_7F4A_7C15);
    let mut frame_no = 0u32;

    run_scene(
        screen,
        events,
        Some(Duration::from_secs(3)),
        move |screen, _| {
            let a = anchor(screen);
            if frame_no == 0 {
                paint_blank(screen);
                draw_box(
                    screen,
                    a,
                    &Style::default().fg(BasicColor::BrightWhite.into()),
                );
                footer(screen, a, "scene 1 / 6 — sprinkles");
            }
            let inner_w = (BOX_W - 2) as u32;
            let inner_h = (BOX_H - 2) as u32;
            let glyph = if frame_no < 30 { "·" } else { "✦" };
            let fg = if frame_no < 30 {
                Color::Basic(BasicColor::BrightCyan)
            } else {
                Color::Basic(BasicColor::BrightYellow)
            };
            let style = Style::default().fg(fg);
            let cell = Cell::narrow(glyph).style(style);
            for _ in 0..40 {
                let dx = rng.range(inner_w) as u16;
                let dy = rng.range(inner_h) as u16;
                let x = a.x + 1 + dx;
                let y = a.y + 1 + dy;
                screen.set_cell(Position::new(x, y), &cell);
            }
            frame_no += 1;
            Ok(())
        },
    )
}

fn scene_panels<W: Write>(
    screen: &mut Screen<W>,
    events: &mut Source<Stdin>,
) -> std::io::Result<bool> {
    let mut rng = Rng::new(0x243F_6A88_85A3_08D3);
    let mut last_tick = u64::MAX;

    type PanelAttr = fn(Style) -> Style;
    type Panel = (
        u16,
        u16,
        u16,
        u16,
        BasicColor,
        BasicColor,
        &'static str,
        PanelAttr,
    );
    let id_attr: PanelAttr = |s| s.italic();
    let bd_attr: PanelAttr = |s| s.bold();
    let mx_attr: PanelAttr = |s| s.bold().italic();
    let mut panels: Vec<Panel> = vec![
        (
            4,
            2,
            32,
            7,
            BasicColor::Blue,
            BasicColor::BrightWhite,
            "back panel — italic",
            id_attr,
        ),
        (
            10,
            4,
            32,
            7,
            BasicColor::Magenta,
            BasicColor::BrightWhite,
            "middle — bold",
            bd_attr,
        ),
        (
            16,
            6,
            32,
            7,
            BasicColor::Green,
            BasicColor::Black,
            "front — bold + italic",
            mx_attr,
        ),
    ];

    run_scene(
        screen,
        events,
        Some(Duration::from_secs(6)),
        move |screen, elapsed| {
            let tick = elapsed.as_millis() as u64 / 600;
            if tick == last_tick {
                return Ok(());
            }
            if last_tick != u64::MAX {
                let idx = rng.range(panels.len() as u32) as usize;
                let mut p = panels.remove(idx);
                let max_dx = BOX_W.saturating_sub(p.2 + 2);
                let max_dy = BOX_H.saturating_sub(p.3 + 1);
                p.0 = 2 + rng.range((max_dx - 1) as u32) as u16;
                p.1 = 1 + rng.range((max_dy - 1) as u32) as u16;
                panels.push(p);
            }
            last_tick = tick;

            let a = anchor(screen);
            paint_blank(screen);
            draw_box(
                screen,
                a,
                &Style::default().fg(BasicColor::BrightWhite.into()),
            );
            for &(dx, dy, w, h, bg, fg, label, attr) in &panels {
                let x = a.x + dx;
                let y = a.y + dy;
                let style = Style::default().bg(bg.into()).fg(fg.into());
                let cell = Cell::narrow(" ").style(style.clone());
                for yy in y..y + h {
                    for xx in x..x + w {
                        screen.set_cell(Position::new(xx, yy), &cell);
                    }
                }
                write(screen, x + 2, y + 1, label, &attr(style));
            }
            footer(screen, a, "scene 2 / 6 — nested panels");
            Ok(())
        },
    )
}

const ART: &[&str] = &[
    "             /\\             ",
    "            /  \\            ",
    "           /    \\           ",
    "          /      \\          ",
    "         /  /\\    \\         ",
    "        /  /  \\    \\        ",
    "       /  /    \\    \\       ",
    "      /  /  /\\  \\    \\      ",
    "     /  /  /  \\  \\    \\     ",
    "    /  /  /    \\  \\    \\    ",
    "   /  /  /  /\\  \\  \\    \\   ",
    "  /__/__/__/__\\__\\__\\____\\  ",
    "  ~~~~~~~~~~~~~~~~~~~~~~~~  ",
];

fn scene_art<W: Write>(
    screen: &mut Screen<W>,
    events: &mut Source<Stdin>,
) -> std::io::Result<bool> {
    let draw_ms: u64 = 2200;
    let flash_ms: u64 = 2200;
    let total_ms = draw_ms + flash_ms;
    let mut last_row = usize::MAX;
    let mut last_phase: Option<bool> = None;
    run_scene(
        screen,
        events,
        Some(Duration::from_millis(total_ms)),
        move |screen, elapsed| {
            let a = anchor(screen);
            let label_x = a.x + (BOX_W - ART[0].chars().count() as u16) / 2;
            let label_y = a.y + ((BOX_H - 1).saturating_sub(ART.len() as u16)) / 2 + 1;
            let ms = elapsed.as_millis() as u64;
            if ms < draw_ms {
                let step = (ms * ART.len() as u64 / draw_ms) as usize;
                let row = step.min(ART.len());
                if row == last_row {
                    return Ok(());
                }
                if last_row == usize::MAX {
                    paint_blank(screen);
                    fill_inside(screen, a, &Style::default().bg(BasicColor::Black.into()));
                    draw_box(screen, a, &Style::default().fg(BasicColor::Red.into()));
                    footer(screen, a, "scene 3 / 6 — line-by-line art");
                }
                last_row = row;
                let style = Style::default()
                    .fg(BasicColor::BrightWhite.into())
                    .bg(BasicColor::Black.into());
                for (i, line) in ART.iter().take(row).enumerate() {
                    write(screen, label_x, label_y + i as u16, line, &style);
                }
            } else {
                let phase = ((ms - draw_ms) / 250).is_multiple_of(2);
                if last_phase == Some(phase) {
                    return Ok(());
                }
                last_phase = Some(phase);
                let mut style = Style::default()
                    .fg(BasicColor::BrightWhite.into())
                    .bg(BasicColor::Black.into());
                if phase {
                    style = style.faint();
                }
                for (i, line) in ART.iter().enumerate() {
                    write(screen, label_x, label_y + i as u16, line, &style);
                }
            }
            Ok(())
        },
    )
}

fn scene_banner<W: Write>(
    screen: &mut Screen<W>,
    events: &mut Source<Stdin>,
) -> std::io::Result<bool> {
    let mut drawn = false;
    run_scene(
        screen,
        events,
        Some(Duration::from_millis(6000)),
        move |screen, _| {
            if drawn {
                return Ok(());
            }
            drawn = true;
            let a = anchor(screen);
            paint_blank(screen);
            draw_box(
                screen,
                a,
                &Style::default().fg(BasicColor::BrightCyan.into()),
            );

            let title = Style::default().fg(BasicColor::BrightWhite.into()).bold();
            write(screen, a.x + 4, a.y + 1, "Style sampler", &title);

            // Underline styles row.
            let row = a.y + 3;
            let col = a.x + 4;

            let single = Style::default()
                .fg(BasicColor::BrightWhite.into())
                .underline_style(UnderlineStyle::Single);
            write(screen, col, row, "single", &single);

            let double = Style::default()
                .fg(BasicColor::BrightWhite.into())
                .underline_style(UnderlineStyle::Double)
                .underline_color(BasicColor::Cyan.into());
            write(screen, col + 10, row, "double", &double);

            let curly = Style::default()
                .fg(BasicColor::BrightWhite.into())
                .underline_style(UnderlineStyle::Curly)
                .underline_color(BasicColor::BrightRed.into());
            write(screen, col + 20, row, "curly", &curly);

            let dotted = Style::default()
                .fg(BasicColor::BrightWhite.into())
                .underline_style(UnderlineStyle::Dotted)
                .underline_color(BasicColor::BrightYellow.into());
            write(screen, col + 30, row, "dotted", &dotted);

            let dashed = Style::default()
                .fg(BasicColor::BrightWhite.into())
                .underline_style(UnderlineStyle::Dashed)
                .underline_color(BasicColor::BrightGreen.into());
            write(screen, col + 40, row, "dashed", &dashed);

            // Text attributes row.
            let attr_row = a.y + 5;
            let white = || Style::default().fg(BasicColor::BrightWhite.into());
            write(screen, col, attr_row, "bold", &white().bold());
            write(screen, col + 6, attr_row, "italic", &white().italic());
            write(screen, col + 14, attr_row, "faint", &white().faint());
            write(screen, col + 21, attr_row, " reverse ", &white().reverse());
            write(
                screen,
                col + 32,
                attr_row,
                "strike",
                &white().strikethrough(),
            );
            write(screen, col + 40, attr_row, "blink", &white().blink());

            // Spell-check look (curly red underline on a typo).
            let plain = white();
            write(screen, col, a.y + 7, "spell-check look:", &plain);
            let typo = white()
                .underline_style(UnderlineStyle::Curly)
                .underline_color(BasicColor::BrightRed.into());
            write(screen, col + 19, a.y + 7, "teh", &typo);
            write(screen, col + 23, a.y + 7, "quick brown fox", &plain);

            // Mixed-style demo line: bold + italic + colored fg.
            let mixed = Style::default()
                .fg(BasicColor::BrightMagenta.into())
                .bold()
                .italic();
            write(screen, col, a.y + 9, "bold + italic + magenta", &mixed);

            // Hyperlink line.
            let link_label = Style::default()
                .fg(BasicColor::BrightBlue.into())
                .underline_style(UnderlineStyle::Single)
                .bold();
            write(screen, col, a.y + 11, "open the project page →", &plain);
            write_link(
                screen,
                col + 24,
                a.y + 11,
                "click here",
                &link_label,
                LINK_URL,
            );

            footer(screen, a, "scene 4 / 6 — underlines + hyperlink");
            Ok(())
        },
    )
}

const MARQUEE: &[(&str, UnderlineStyle, BasicColor)] = &[
    (
        "welcome to the tour",
        UnderlineStyle::Single,
        BasicColor::BrightCyan,
    ),
    (
        "six small scenes",
        UnderlineStyle::Double,
        BasicColor::BrightMagenta,
    ),
    (
        "hyperlinks travel with the cell",
        UnderlineStyle::Curly,
        BasicColor::BrightYellow,
    ),
    (
        "each underline has its own color",
        UnderlineStyle::Dotted,
        BasicColor::BrightGreen,
    ),
    (
        "press any key to advance",
        UnderlineStyle::Dashed,
        BasicColor::BrightRed,
    ),
];

fn scene_marquee<W: Write>(
    screen: &mut Screen<W>,
    events: &mut Source<Stdin>,
) -> std::io::Result<bool> {
    let mut drawn_box = false;
    let mut last_offset = i32::MIN;
    let joined: String = {
        let mut s = String::new();
        for (i, (msg, _, _)) in MARQUEE.iter().enumerate() {
            if i > 0 {
                s.push_str(" • ");
            }
            s.push_str(msg);
        }
        s.push_str(" • ");
        s
    };

    let mut spans: Vec<(usize, usize, UnderlineStyle, BasicColor)> = Vec::new();
    {
        let mut cursor = 0usize;
        for (i, (msg, us, c)) in MARQUEE.iter().enumerate() {
            if i > 0 {
                cursor += " • ".chars().count();
            }
            let len = msg.chars().count();
            spans.push((cursor, cursor + len, *us, *c));
            cursor += len;
        }
    }

    let total = joined.chars().count();
    let inner_w = (BOX_W - 4) as usize;

    run_scene(screen, events, None, move |screen, elapsed| {
        let a = anchor(screen);
        if !drawn_box {
            drawn_box = true;
            paint_blank(screen);
            draw_box(
                screen,
                a,
                &Style::default().fg(BasicColor::BrightWhite.into()),
            );
            footer(
                screen,
                a,
                "scene 5 / 6 — marquee — any key to continue, Q to quit",
            );
        }
        let speed_chars_per_sec = 12.0f32;
        let offset = (elapsed.as_secs_f32() * speed_chars_per_sec) as i32;
        if offset == last_offset {
            return Ok(());
        }
        last_offset = offset;

        let row = a.y + BOX_H / 2;
        let blank = Style::default();
        let spaces: String = " ".repeat(inner_w);
        write(screen, a.x + 2, row, &spaces, &blank);

        let chars: Vec<char> = joined.chars().collect();
        for col in 0..inner_w {
            let idx_signed = offset + col as i32;
            let idx = idx_signed.rem_euclid(total as i32) as usize;
            let ch = chars[idx];
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            let mut style = Style::default().fg(BasicColor::BrightWhite.into());
            for (lo, hi, us, c) in &spans {
                if idx >= *lo && idx < *hi {
                    style = style.underline_style(*us).underline_color((*c).into());
                    break;
                }
            }
            write(screen, a.x + 2 + col as u16, row, s, &style);
        }
        Ok(())
    })
}

struct Ball {
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
    glyph: &'static str,
    color: BasicColor,
}

fn scene_balls<W: Write>(
    screen: &mut Screen<W>,
    events: &mut Source<Stdin>,
) -> std::io::Result<bool> {
    let mut balls = vec![
        Ball {
            x: 4.0,
            y: 2.0,
            dx: 0.7,
            dy: 0.4,
            glyph: "O",
            color: BasicColor::BrightRed,
        },
        Ball {
            x: 18.0,
            y: 6.0,
            dx: -0.5,
            dy: 0.6,
            glyph: "*",
            color: BasicColor::BrightYellow,
        },
        Ball {
            x: 32.0,
            y: 4.0,
            dx: 0.6,
            dy: -0.5,
            glyph: "@",
            color: BasicColor::BrightCyan,
        },
    ];
    let mut box_drawn = false;

    run_scene(screen, events, None, move |screen, _elapsed| {
        let a = anchor(screen);
        // Interior playfield: cols 1..BOX_W-1 (BOX_W-2 wide) and rows
        // 1..BOX_H-1 (BOX_H-2 tall) — full interior, no reserved
        // footer row so balls can hit the bottom border.
        let inner_w = (BOX_W - 2) as f32;
        let inner_h = (BOX_H - 2) as f32;
        if !box_drawn {
            box_drawn = true;
            paint_blank(screen);
            draw_box(
                screen,
                a,
                &Style::default().fg(BasicColor::BrightWhite.into()),
            );
            // Hint sits below the box; full interior is free for bouncing.
            footer(
                screen,
                a,
                "scene 6 / 6 — bouncing balls — any key to continue, Q to quit",
            );
        }
        let blank = Cell::narrow(" ").style(Style::default());
        for y in a.y + 1..a.y + BOX_H - 1 {
            for x in a.x + 1..a.x + BOX_W - 1 {
                screen.set_cell(Position::new(x, y), &blank);
            }
        }
        for ball in balls.iter_mut() {
            ball.x += ball.dx;
            ball.y += ball.dy;
            if ball.x < 0.0 {
                ball.x = 0.0;
                ball.dx = -ball.dx;
            }
            if ball.x > inner_w - 1.0 {
                ball.x = inner_w - 1.0;
                ball.dx = -ball.dx;
            }
            if ball.y < 0.0 {
                ball.y = 0.0;
                ball.dy = -ball.dy;
            }
            if ball.y > inner_h - 1.0 {
                ball.y = inner_h - 1.0;
                ball.dy = -ball.dy;
            }
        }
        for ball in &balls {
            let cx = a.x + 1 + ball.x as u16;
            let cy = a.y + 1 + ball.y as u16;
            let style = Style::default().fg(ball.color.into()).bold();
            let cell = Cell::narrow(ball.glyph).style(style);
            screen.set_cell(Position::new(cx, cy), &cell);
        }
        Ok(())
    })
}

fn main() -> std::io::Result<()> {
    let stdin = stdin();
    let stdout = stdout();
    let state = enable_raw_mode(stdin, stdout)?;
    let size = get_window_size(stdout).unwrap_or_default();
    let mut screen = Screen::new(stdout, (size.col, size.row));
    screen.set_alt_screen(true)?;
    screen.set_cursor_visible(false)?;

    let mut events = Source::new(stdin)?;

    type Scene =
        fn(&mut Screen<uncurses::terminal::Stdout>, &mut Source<Stdin>) -> std::io::Result<bool>;
    let scenes: [Scene; 6] = [
        scene_sprinkles,
        scene_panels,
        scene_art,
        scene_banner,
        scene_marquee,
        scene_balls,
    ];

    'outer: loop {
        for scene in scenes {
            if !scene(&mut screen, &mut events)? {
                break 'outer;
            }
        }
    }

    screen.reset()?;
    screen.flush()?;
    disable_raw_mode(stdin, stdout, &state)?;
    Ok(())
}
