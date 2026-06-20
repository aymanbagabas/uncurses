//! Centered click-counter button.
//!
//! Press `enter` or `space`, or click the button with the mouse, to increment.
//! `q`, `Esc`, or `Ctrl-C` exits.

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::color::BasicColor;
use uncurses::event::{Event, Key, MouseButton};
use uncurses::screen::{MousePreference, Screen, ScreenOptions};
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

/// Click-counter app: owns a self-managing [`Screen`] plus its own UI
/// state. `start` enters the alternate screen and enables mouse tracking,
/// `run` drives the event loop, and teardown is handled by
/// [`Screen::finish`].
struct App {
    screen: Screen<Stdin, Stdout>,
    count: u32,
    button_rect: Option<(u16, u16, u16)>,
    quit_keys: [Key; 3],
    click_keys: [Key; 2],
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut screen = Screen::stdio()?;
        // Begin a session and enable plain (click) mouse tracking; the
        // screen picks the best mode and encoding the terminal supports.
        screen.init_with(ScreenOptions {
            mouse: Some(MousePreference::default()),
            ..ScreenOptions::default()
        })?;
        screen.enter_alt_screen()?;
        screen.hide_cursor()?;

        // Parse key bindings once. `Key: FromStr`, and `==` compares on
        // the canonical chord identity — so plain equality is the right
        // operator for keyboard-shortcut matching.
        Ok(Self {
            screen,
            count: 0,
            button_rect: None,
            quit_keys: ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap()),
            click_keys: ["enter", "space"].map(|s| s.parse().unwrap()),
        })
    }

    fn render(&mut self) -> std::io::Result<()> {
        redraw(&mut self.screen, self.count);
        self.button_rect = button_bounds(&self.screen, self.count);
        self.screen.present()
    }

    fn run(&mut self) -> std::io::Result<()> {
        self.render()?;

        loop {
            let ev = self.screen.read_event()?;
            let mut dirty = false;
            match ev {
                Event::KeyPress(ref key) if self.quit_keys.contains(key) => break,
                Event::KeyPress(ref key) if self.click_keys.contains(key) => {
                    self.count = self.count.saturating_add(1);
                    dirty = true;
                }
                Event::MouseClick(m)
                    if m.button == MouseButton::Left && hit(self.button_rect, m.x, m.y) =>
                {
                    self.count = self.count.saturating_add(1);
                    dirty = true;
                }
                Event::Resize(ws) => {
                    self.screen.resize((ws.col, ws.row));
                    dirty = true;
                }
                _ => {}
            }
            if dirty {
                self.render()?;
            }
        }
        Ok(())
    }

    fn stop(self) -> std::io::Result<()> {
        self.screen.finish()
    }
}

fn main() -> std::io::Result<()> {
    let mut app = App::start()?;
    let result = app.run();
    app.stop()?;
    result
}

fn button_label(count: u32) -> String {
    let label = if count == 0 {
        "Click me!".to_string()
    } else {
        format!("Clicks: {count}")
    };
    format!("[ {label} ]")
}

fn button_bounds(screen: &Screen<Stdin, Stdout>, count: u32) -> Option<(u16, u16, u16)> {
    let w = screen.width();
    let h = screen.height();
    if w < 20 || h < 5 {
        return None;
    }
    let inner = button_label(count);
    let inner_w = inner.chars().count() as u16;
    let x = w.saturating_sub(inner_w) / 2;
    let y = h / 2;
    Some((x, y, inner_w))
}

fn hit(rect: Option<(u16, u16, u16)>, mx: u16, my: u16) -> bool {
    let Some((x, y, w)) = rect else {
        return false;
    };
    my == y && mx >= x && mx < x + w
}

fn redraw(screen: &mut Screen<Stdin, Stdout>, count: u32) {
    screen.clear();
    let w = screen.width();
    let h = screen.height();
    if w < 20 || h < 5 {
        return;
    }

    let inner = button_label(count);
    let inner_w = inner.chars().count() as u16;
    let x = w.saturating_sub(inner_w) / 2;
    let y = h / 2;

    let button = Style::default()
        .fg(BasicColor::BrightWhite.into())
        .bg(BasicColor::Blue.into())
        .bold();
    screen.set_str((x, y), &inner, button);

    let help = Style::default().fg(BasicColor::BrightBlack.into());
    let hint = "click / enter / space: increment • q: quit";
    let hint_w = hint.chars().count() as u16;
    let hx = w.saturating_sub(hint_w) / 2;
    screen.set_str((hx, h.saturating_sub(2)), hint, help);
}
