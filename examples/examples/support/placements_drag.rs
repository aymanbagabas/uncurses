use std::io::{self, Write};

use uncurses::buffer::{Buffer, Surface, SurfaceMut};
use uncurses::color::{Color, Profile};
use uncurses::event::{Event, Key, MouseButton};
use uncurses::layout::{Position, Rect, Size};
use uncurses::program::{MouseTracking, Program, ProgramOptions};
use uncurses::screen::{Screen, TextSizing};
use uncurses::style::Style;
use uncurses::terminal::Winsize;
use uncurses::text::{TextSurface, WidthMode};
use uncurses_images::{Backend, CellImage, PreparedImage};

const WAITING_PIXELS: &str = "Waiting for cell pixels. Use --cell-pixels WxH; q quits.";

#[derive(Default)]
pub struct Options {
    pub inline: bool,
    pub dump: bool,
    pub cell_pixels: Option<Size>,
    pub replay_keys: Vec<Key>,
    pub dialog: bool,
    pub dump_dialog: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PixelGeometry {
    Unused,
    Optional,
    Required,
}

#[allow(dead_code)]
pub struct Preparation {
    pub width_mode: WidthMode,
    pub eaw_wide: bool,
    pub profile: Profile,
    pub cell_pixels: Option<Size>,
}

impl Preparation {
    fn new<W: Write>(screen: &Screen<W>, cell_pixels: Option<Size>) -> Self {
        Self {
            width_mode: screen.width_mode(),
            eaw_wide: screen.eaw_wide(),
            profile: screen.color_profile(),
            cell_pixels,
        }
    }
}

#[allow(dead_code)]
pub enum Visual {
    Native(PreparedImage),
    Cells(Buffer),
    Placeholders(CellImage),
    SizedText {
        size: Size,
        text: String,
        sizing: TextSizing,
        style: Style,
    },
}

impl Visual {
    fn size(&self) -> Size {
        match self {
            Self::Native(block) => block.size(),
            Self::Cells(cells) => Size::new(cells.width(), cells.height()),
            Self::Placeholders(image) => Size::new(image.cells().width(), image.cells().height()),
            Self::SizedText { size, .. } => *size,
        }
    }
}

pub trait Content {
    fn title(&self) -> String;
    fn geometry(&self) -> PixelGeometry;
    fn prepare(&mut self, context: Preparation) -> io::Result<Visual>;
    fn controls(&self) -> &str {
        "Drag or use arrows. q quits."
    }
    fn on_key(&mut self, _key: &Key) -> bool {
        false
    }
}

fn prepare_visual(
    content: &mut impl Content,
    context: Preparation,
) -> io::Result<(Option<Visual>, String)> {
    if content.geometry() == PixelGeometry::Required && context.cell_pixels.is_none() {
        return Ok((None, WAITING_PIXELS.to_owned()));
    }
    match content.prepare(context) {
        Ok(visual) => Ok((Some(visual), String::new())),
        Err(error) if error.kind() == io::ErrorKind::Unsupported => {
            Ok((None, format!("{error}. Select another protocol.")))
        }
        Err(error) => Err(error),
    }
}

struct Drag {
    position: Position,
    grab: Option<Position>,
    dialog: Option<Position>,
}

impl Default for Drag {
    fn default() -> Self {
        Self {
            position: Position::new(2, 3),
            grab: None,
            dialog: None,
        }
    }
}

impl Drag {
    fn dialog_key(&mut self, key: &Key) -> bool {
        if key.matches("space") {
            self.dialog = if self.dialog.is_some() {
                None
            } else {
                Some(Position::new(5, 4))
            };
            return true;
        }
        if let Some(position) = &mut self.dialog {
            if key.matches("shift+left") {
                position.x = position.x.saturating_sub(1);
            } else if key.matches("shift+right") {
                position.x = position.x.saturating_add(1);
            } else if key.matches("shift+up") {
                position.y = position.y.saturating_sub(1);
            } else if key.matches("shift+down") {
                position.y = position.y.saturating_add(1);
            } else {
                return false;
            }
            return true;
        }
        false
    }

    fn fit(&mut self, size: Size, viewport: Size) -> bool {
        let Some(max_x) = viewport.width.checked_sub(size.width) else {
            return false;
        };
        let Some(max_y) = viewport.height.checked_sub(size.height.saturating_add(1)) else {
            return false;
        };
        if max_y < 2 {
            return false;
        }
        self.position.x = self.position.x.min(max_x);
        self.position.y = self.position.y.clamp(2, max_y);
        true
    }

    fn press(&mut self, point: Position, size: Size) {
        self.grab = point
            .x
            .checked_sub(self.position.x)
            .zip(point.y.checked_sub(self.position.y))
            .filter(|&(x, y)| x < size.width && y < size.height)
            .map(|(x, y)| Position::new(x, y));
    }

    fn move_to(&mut self, point: Position) {
        if let Some(grab) = self.grab {
            self.position = Position::new(
                point.x.saturating_sub(grab.x),
                point.y.saturating_sub(grab.y),
            );
        }
    }
}

fn local_press(point: Position, origin: Position, viewport: Size) -> Option<Position> {
    let x = point.x.checked_sub(origin.x)?;
    let y = point.y.checked_sub(origin.y)?;
    (x < viewport.width && y < viewport.height).then_some(Position::new(x, y))
}

fn resize_cell_pixels(size: Winsize) -> Option<Size> {
    let width = size.xpixel.checked_div(size.col)?;
    let height = size.ypixel.checked_div(size.row)?;
    (width > 0 && height > 0).then_some(Size::new(width, height))
}

pub fn parse_cell_pixels(value: Option<String>) -> io::Result<Size> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "--cell-pixels needs positive dimensions, such as 8x16",
        )
    };
    let value = value.ok_or_else(invalid)?;
    let (width, height) = value.split_once('x').ok_or_else(invalid)?;
    let width = width.parse::<u16>().map_err(|_| invalid())?;
    let height = height.parse::<u16>().map_err(|_| invalid())?;
    if width == 0 || height == 0 {
        return Err(invalid());
    }
    Ok(Size::new(width, height))
}

#[allow(dead_code)]
pub fn run(
    title: &str,
    options: Options,
    geometry: PixelGeometry,
    prepare: impl FnMut(Preparation) -> io::Result<Visual>,
) -> io::Result<()> {
    struct Fixed<'a, F> {
        title: &'a str,
        geometry: PixelGeometry,
        prepare: F,
    }
    impl<F: FnMut(Preparation) -> io::Result<Visual>> Content for Fixed<'_, F> {
        fn title(&self) -> String {
            self.title.to_owned()
        }
        fn geometry(&self) -> PixelGeometry {
            self.geometry
        }
        fn prepare(&mut self, context: Preparation) -> io::Result<Visual> {
            (self.prepare)(context)
        }
    }
    run_interactive(
        options,
        Fixed {
            title,
            geometry,
            prepare,
        },
    )
}

pub fn run_interactive(options: Options, mut content: impl Content) -> io::Result<()> {
    if options.dump {
        return dump(&options, &mut content);
    }
    let mut program = Program::stdio()?;
    let result = (|| {
        program.init_with(ProgramOptions {
            mouse: Some(MouseTracking::empty()),
            bracketed_paste: false,
            ..ProgramOptions::default()
        })?;
        if options.inline {
            let size = program.screen().size();
            program
                .screen_mut()
                .resize((size.width, size.height.min(12)));
        } else {
            program.enter_alt_screen()?;
        }
        program.hide_cursor()?;

        let mut pixels = options.cell_pixels.or_else(|| program.cell_pixels());
        if content.geometry() != PixelGeometry::Unused && options.cell_pixels.is_none() {
            program.request_cell_pixel_size()?;
        }
        let (mut visual, mut notice) =
            prepare_visual(&mut content, Preparation::new(program.screen(), pixels))?;
        let mut drag = Drag::default();
        if options.dialog {
            drag.dialog = Some(Position::new(5, 4));
        }
        paint(
            program.screen_mut(),
            &mut drag,
            visual.as_ref(),
            &content.title(),
            &notice,
            content.controls(),
        )?;
        let mut origins_pending = 0usize;
        if options.inline {
            program.request_origin()?;
            origins_pending += 1;
        }

        let result = (|| {
            loop {
                let event = program.read_event()?;
                let mut reprepare = false;
                let mut query_origin = false;
                match event {
                    Event::KeyPress(key) if key.matches_any(["q", "esc", "ctrl+c"]) => break,
                    Event::KeyPress(key) if drag.dialog_key(&key) => {}
                    Event::KeyPress(key) if key.matches_any(["left", "right", "up", "down"]) => {
                        drag.grab = None;
                        if key.matches("left") {
                            drag.position.x = drag.position.x.saturating_sub(1);
                        } else if key.matches("right") {
                            drag.position.x = drag.position.x.saturating_add(1);
                        } else if key.matches("up") {
                            drag.position.y = drag.position.y.saturating_sub(1);
                        } else {
                            drag.position.y = drag.position.y.saturating_add(1);
                        }
                    }
                    Event::KeyPress(key) if content.on_key(&key) => reprepare = true,
                    Event::Resize(size) => {
                        drag.grab = None;
                        let height = if options.inline {
                            size.row.min(12)
                        } else {
                            size.row
                        };
                        program.screen_mut().resize((size.col, height));
                        query_origin = options.inline;
                        if content.geometry() != PixelGeometry::Unused
                            && options.cell_pixels.is_none()
                        {
                            let reported = resize_cell_pixels(size);
                            reprepare = pixels != reported;
                            pixels = reported;
                            program.request_cell_pixel_size()?;
                        }
                    }
                    Event::CellPixelSize { width, height }
                        if content.geometry() != PixelGeometry::Unused
                            && options.cell_pixels.is_none() =>
                    {
                        if width == 0 || height == 0 {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "the terminal reported zero cell pixel dimensions",
                            ));
                        }
                        let reported = Size::new(width, height);
                        reprepare = pixels != Some(reported);
                        pixels = Some(reported);
                    }
                    Event::CursorPosition { .. } if origins_pending > 0 => {
                        origins_pending -= 1;
                    }
                    Event::MouseClick(mouse)
                        if mouse.button == MouseButton::Left && origins_pending == 0 =>
                    {
                        let viewport = program.screen().size();
                        if let Some(visual) = &visual
                            && drag.fit(visual.size(), viewport)
                            && let Some(point) = local_press(
                                Position::new(mouse.x, mouse.y),
                                program.origin(),
                                viewport,
                            )
                        {
                            drag.press(point, visual.size());
                        }
                    }
                    Event::MouseMove(mouse)
                        if mouse.button == MouseButton::Left && origins_pending == 0 =>
                    {
                        let origin = program.origin();
                        drag.move_to(Position::new(
                            mouse.x.saturating_sub(origin.x),
                            mouse.y.saturating_sub(origin.y),
                        ));
                    }
                    Event::MouseRelease(_) => drag.grab = None,
                    _ => continue,
                }
                if reprepare {
                    drag.grab = None;
                    (visual, notice) =
                        prepare_visual(&mut content, Preparation::new(program.screen(), pixels))?;
                }
                paint(
                    program.screen_mut(),
                    &mut drag,
                    visual.as_ref(),
                    &content.title(),
                    &notice,
                    content.controls(),
                )?;
                if query_origin {
                    program.request_origin()?;
                    origins_pending += 1;
                }
            }
            Ok(())
        })();
        let cleanup = paint(
            program.screen_mut(),
            &mut drag,
            None,
            &content.title(),
            "Resources cleared.",
            content.controls(),
        );
        result.and(cleanup)
    })();
    let finish = program.finish();
    result.and(finish)
}

fn paint<W: Write>(
    screen: &mut Screen<W>,
    drag: &mut Drag,
    visual: Option<&Visual>,
    title: &str,
    empty_message: &str,
    controls: &str,
) -> io::Result<()> {
    screen.clear();
    screen.set_str((0, 0), title, Style::default().bold());
    screen.set_str((0, 1), controls, Style::EMPTY);
    let size = screen.size();
    for y in 2..size.height.saturating_sub(1) {
        let row = format!("row {y:02} {}", ".".repeat(usize::from(size.width)));
        screen.set_str((0, y), &row, Style::default().fg(Color::BrightBlack));
    }
    let message = match visual {
        Some(visual) if drag.fit(visual.size(), size) => {
            match visual {
                Visual::Native(image) => {
                    let area = Rect::new(
                        drag.position.x,
                        drag.position.y,
                        image.size().width,
                        image.size().height,
                    );
                    let visible = if image.backend() == Backend::Kitty {
                        visible_rectangles(area, drag.dialog.map(|p| Rect::new(p.x, p.y, 10, 3)))
                    } else {
                        vec![area]
                    };
                    for (index, rect) in visible.into_iter().enumerate() {
                        let id = index as u32 + 1;
                        let source =
                            Rect::new(rect.x - area.x, rect.y - area.y, rect.width, rect.height);
                        let options = image.crop(source)?.placement(id)?;
                        screen.set_placement(u64::from(id), rect, &options)?;
                    }
                }
                Visual::Cells(cells) => {
                    cells.draw(screen, drag.position);
                }
                Visual::Placeholders(image) => {
                    let size = visual.size();
                    screen.set_placement(
                        1,
                        Rect::new(drag.position.x, drag.position.y, size.width, size.height),
                        &image.placement(screen)?,
                    )?;
                }
                Visual::SizedText {
                    text,
                    sizing,
                    style,
                    ..
                } => {
                    screen.set_sized_text(1, drag.position, text, *sizing, style)?;
                }
            }
            format!("position {},{}", drag.position.x, drag.position.y)
        }
        Some(_) => {
            drag.grab = None;
            "Resize the terminal to show the block.".to_owned()
        }
        None => empty_message.to_owned(),
    };
    if visual.is_some()
        && let Some(position) = drag.dialog
    {
        let width = 10u16.min(size.width.saturating_sub(position.x));
        let height = 3u16.min(size.height.saturating_sub(position.y).saturating_sub(1));
        if width > 0 && height > 0 {
            let style = Style::default()
                .fg(Color::Rgb(255, 255, 255))
                .bg(Color::Rgb(0, 0, 0))
                .bold();
            screen.fill_rect(
                Rect::new(position.x, position.y, width, height),
                &uncurses::cell::Cell::BLANK.style(style.clone()),
            );
            screen.set_str(position, &"Dialog"[..usize::from(width.min(6))], style);
        }
    }
    if let Some(y) = size.height.checked_sub(1) {
        screen.set_str(
            (0, y),
            &format!("{message} | Space: dialog, Shift+arrows: move dialog"),
            Style::EMPTY,
        );
        screen.set_cursor_position((0, y));
    }
    screen.render()
}

fn visible_rectangles(area: Rect, overlay: Option<Rect>) -> Vec<Rect> {
    let Some(overlay) = overlay else {
        return vec![area];
    };
    let cut = area.intersection(overlay);
    if cut.width == 0 || cut.height == 0 {
        return vec![area];
    }
    [
        Rect::new(area.x, area.y, area.width, cut.y - area.y),
        Rect::new(
            area.x,
            cut.bottom(),
            area.width,
            area.bottom() - cut.bottom(),
        ),
        Rect::new(area.x, cut.y, cut.x - area.x, cut.height),
        Rect::new(cut.right(), cut.y, area.right() - cut.right(), cut.height),
    ]
    .into_iter()
    .filter(|r| r.width > 0 && r.height > 0)
    .collect()
}

fn dump(options: &Options, content: &mut impl Content) -> io::Result<()> {
    let mut screen = Screen::new(Vec::new(), (40, 12));
    screen.set_fullscreen(!options.inline);
    let pixels = Some(options.cell_pixels.unwrap_or(Size::new(8, 16)));
    let mut visual = content.prepare(Preparation::new(&screen, pixels))?;
    let mut drag = Drag::default();
    if options.dump_dialog {
        for (name, position) in [
            ("initial", None),
            ("dialog", Some(Position::new(5, 4))),
            ("dialog unchanged", Some(Position::new(5, 4))),
            ("dialog move", Some(Position::new(8, 5))),
            ("dialog origin", Some(Position::new(2, 3))),
            ("dialog close", None),
            ("remove", None),
        ] {
            drag.dialog = position;
            paint(
                &mut screen,
                &mut drag,
                (name != "remove").then_some(&visual),
                &content.title(),
                "Resources cleared.",
                content.controls(),
            )?;
            dump_frame(name, &mut screen)?;
        }
        return Ok(());
    }
    if !options.replay_keys.is_empty() {
        paint(
            &mut screen,
            &mut drag,
            Some(&visual),
            &content.title(),
            "",
            content.controls(),
        )?;
        dump_frame("initial", &mut screen)?;
        for (index, key) in options.replay_keys.iter().enumerate() {
            let dialog_changed = drag.dialog_key(key);
            if !dialog_changed && !content.on_key(key) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unhandled replay key: {key}"),
                ));
            }
            if !dialog_changed {
                visual = content.prepare(Preparation::new(&screen, pixels))?;
            }
            paint(
                &mut screen,
                &mut drag,
                Some(&visual),
                &content.title(),
                "",
                content.controls(),
            )?;
            dump_frame(&format!("key-{index}-{key}"), &mut screen)?;
            paint(
                &mut screen,
                &mut drag,
                Some(&visual),
                &content.title(),
                "",
                content.controls(),
            )?;
            dump_frame("unchanged", &mut screen)?;
        }
        paint(
            &mut screen,
            &mut drag,
            None,
            &content.title(),
            "Resources cleared.",
            content.controls(),
        )?;
        return dump_frame("remove", &mut screen);
    }
    for name in [
        "initial",
        "unchanged",
        "move",
        "resize-small",
        "resize-large",
        "remove",
    ] {
        match name {
            "move" => {
                drag.press(Position::new(3, 4), visual.size());
                drag.move_to(Position::new(21, 6));
                drag.grab = None;
            }
            "resize-small" => screen.resize((40, 3)),
            "resize-large" => screen.resize((40, 12)),
            _ => {}
        }
        paint(
            &mut screen,
            &mut drag,
            (name != "remove").then_some(&visual),
            &content.title(),
            "Resources cleared.",
            content.controls(),
        )?;
        dump_frame(name, &mut screen)?;
    }
    Ok(())
}

fn dump_frame(name: &str, screen: &mut Screen<Vec<u8>>) -> io::Result<()> {
    let bytes = std::mem::take(screen.writer_mut());
    let mut output = io::stdout().lock();
    write!(output, "{name}\t")?;
    for byte in bytes {
        write!(output, "{byte:02x}")?;
    }
    writeln!(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_outside_the_inline_viewport_do_not_start_a_drag() {
        let origin = Position::new(5, 10);
        let size = Size::new(40, 12);
        for point in [(4, 10), (5, 9), (45, 10), (5, 22)] {
            assert_eq!(local_press(point.into(), origin, size), None);
        }
        assert_eq!(
            local_press((8, 14).into(), origin, size),
            Some(Position::new(3, 4))
        );
    }

    #[test]
    fn movement_preserves_the_grab_offset_and_clamps_at_reserved_rows() {
        let mut drag = Drag::default();
        let size = Size::new(12, 3);
        drag.press((5, 4).into(), size);
        drag.move_to((20, 8).into());
        assert_eq!(drag.position, Position::new(17, 7));
        drag.move_to(Position::ORIGIN);
        assert!(drag.fit(size, Size::new(40, 12)));
        assert_eq!(drag.position, Position::new(0, 2));
        drag.move_to((u16::MAX, u16::MAX).into());
        assert!(drag.fit(size, Size::new(40, 12)));
        assert_eq!(drag.position, Position::new(28, 8));
        assert!(!drag.fit(size, Size::new(6, 3)));
    }

    #[test]
    fn a_click_beyond_the_block_cancels_the_grab() {
        let mut drag = Drag::default();
        drag.press((2, 3).into(), Size::new(12, 3));
        assert!(drag.grab.is_some());
        drag.press((14, 3).into(), Size::new(12, 3));
        assert!(drag.grab.is_none());
    }

    #[test]
    fn pixel_geometry_requires_two_positive_dimensions() {
        for value in [None, Some("8"), Some("0x16"), Some("8x0"), Some("-1x16")] {
            assert!(parse_cell_pixels(value.map(str::to_owned)).is_err());
        }
        assert_eq!(
            parse_cell_pixels(Some("8x16".into())).unwrap(),
            Size::new(8, 16)
        );
    }

    #[test]
    fn resize_geometry_uses_the_new_report_without_a_cell_size_reply() {
        assert_eq!(
            resize_cell_pixels(Winsize {
                row: 25,
                col: 60,
                xpixel: 480,
                ypixel: 400
            }),
            Some(Size::new(8, 16))
        );
        assert_eq!(
            resize_cell_pixels(Winsize {
                row: 25,
                col: 60,
                xpixel: 960,
                ypixel: 400
            }),
            Some(Size::new(16, 16))
        );
        for size in [
            Winsize {
                row: 0,
                col: 60,
                xpixel: 480,
                ypixel: 400,
            },
            Winsize {
                row: 25,
                col: 0,
                xpixel: 480,
                ypixel: 400,
            },
            Winsize {
                row: 25,
                col: 60,
                xpixel: 0,
                ypixel: 400,
            },
            Winsize {
                row: 25,
                col: 60,
                xpixel: 480,
                ypixel: 0,
            },
        ] {
            assert_eq!(resize_cell_pixels(size), None);
        }
    }
}
