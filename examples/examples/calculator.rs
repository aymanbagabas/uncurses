//! A mouse-driven calculator laid out like the macOS Calculator, rendered
//! *inline* (no alternate screen) right where the cursor sits.
//!
//! The interesting part is coordinate mapping. Terminals report mouse clicks
//! in whole-screen cell coordinates, but an inline surface starts partway
//! down the screen. [`Program::request_origin`] asks the terminal where the
//! surface's top-left cell physically is, and the answer shows up as
//! [`Program::origin`]; this asks once at startup and again on every resize,
//! since a resize can move the surface. Each click is
//! mapped into surface-local coordinates with [`Program::mouse_to_origin`]
//! before hit-testing the buttons, the mouse-origin analogue of
//! [`Program::mouse_pixels_to_cells`].
//!
//! Run with `cargo run --example calculator`. Click the buttons; press `q`,
//! `Esc`, or `Ctrl-C` to quit.

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::cell::Cell;
use uncurses::color::Color;
use uncurses::event::{Event, Key, MouseButton};
use uncurses::layout::Rect;
use uncurses::program::{MouseTracking, Program, ProgramOptions};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

const BTN_W: u16 = 8;
const BTN_H: u16 = 3;
const GAP: u16 = 1;
const DISPLAY_H: u16 = 3;
const COLS: u16 = 4;
const ROWS: u16 = 5;

const BOARD_W: u16 = COLS * BTN_W + (COLS - 1) * GAP;
const BOARD_H: u16 = DISPLAY_H + GAP + ROWS * BTN_H + (ROWS - 1) * GAP;

/// A key's visual family, which drives its colors.
#[derive(Clone, Copy)]
enum Kind {
    Fun, // AC, +/-, %  (light gray)
    Opr, // ÷ × − + =   (orange)
    Num, // 0-9, .      (dark gray)
}

/// One key: label, what it does, and where it sits in the grid.
struct Button {
    label: &'static str,
    action: Action,
    kind: Kind,
    col: u16,
    row: u16,
    span: u16,
}

#[derive(Clone, Copy)]
enum Action {
    Digit(char),
    Dot,
    Clear,
    Back,
    Sign,
    Percent,
    Op(char),
    Equals,
}

/// The macOS-style keypad, row by row.
fn buttons() -> Vec<Button> {
    use Action::*;
    use Kind::*;
    let mut b = Vec::new();
    let mut push = |label, action, kind, col, row, span| {
        b.push(Button {
            label,
            action,
            kind,
            col,
            row,
            span,
        });
    };
    push("⌫", Back, Fun, 0, 0, 1);
    push("AC", Clear, Fun, 1, 0, 1);
    push("%", Percent, Fun, 2, 0, 1);
    push("÷", Op('/'), Opr, 3, 0, 1);
    for (i, d) in ["7", "8", "9"].iter().enumerate() {
        push(d, Digit(d.chars().next().unwrap()), Num, i as u16, 1, 1);
    }
    push("×", Op('*'), Opr, 3, 1, 1);
    for (i, d) in ["4", "5", "6"].iter().enumerate() {
        push(d, Digit(d.chars().next().unwrap()), Num, i as u16, 2, 1);
    }
    push("−", Op('-'), Opr, 3, 2, 1);
    for (i, d) in ["1", "2", "3"].iter().enumerate() {
        push(d, Digit(d.chars().next().unwrap()), Num, i as u16, 3, 1);
    }
    push("+", Op('+'), Opr, 3, 3, 1);
    push("+/-", Sign, Num, 0, 4, 1);
    push("0", Digit('0'), Num, 1, 4, 1);
    push(".", Dot, Num, 2, 4, 1);
    push("=", Equals, Opr, 3, 4, 1);
    b
}

impl Button {
    /// The button's rectangle within the surface, shifted right by `dx` (the
    /// margin that centers the keypad in the full-width surface).
    fn rect(&self, dx: u16) -> Rect {
        let x = dx + self.col * (BTN_W + GAP);
        let y = DISPLAY_H + GAP + self.row * (BTN_H + GAP);
        let w = self.span * BTN_W + (self.span - 1) * GAP;
        Rect::new(x, y, w, BTN_H)
    }

    fn contains(&self, x: u16, y: u16, dx: u16) -> bool {
        let r = self.rect(dx);
        x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
    }
}

/// Calculator state: standard "entry / accumulator / pending op" machine.
struct Calc {
    display: String,
    acc: f64,
    pending: Option<char>,
    reset_on_next: bool,
}

impl Default for Calc {
    fn default() -> Self {
        Self {
            display: "0".into(),
            acc: 0.0,
            pending: None,
            reset_on_next: true,
        }
    }
}

impl Calc {
    fn entry(&self) -> f64 {
        self.display.parse().unwrap_or(0.0)
    }

    fn set_display(&mut self, value: f64) {
        // Trim a trailing ".0" so integers read cleanly.
        if value.fract() == 0.0 && value.abs() < 1e15 {
            self.display = format!("{}", value as i64);
        } else {
            self.display = format!("{value}");
        }
    }

    fn input_digit(&mut self, d: char) {
        if self.reset_on_next || self.display == "0" {
            self.display.clear();
            self.reset_on_next = false;
        }
        if self.display.len() < 12 {
            self.display.push(d);
        }
    }

    fn input_dot(&mut self) {
        if self.reset_on_next {
            self.display = "0".into();
            self.reset_on_next = false;
        }
        if !self.display.contains('.') {
            self.display.push('.');
        }
    }

    fn input_back(&mut self) {
        // Editing the shown value (even a computed result) turns it back into a
        // live entry, so subsequent digits append instead of replacing it.
        self.reset_on_next = false;
        self.display.pop();
        if self.display.is_empty() || self.display == "-" {
            self.display = "0".into();
            self.reset_on_next = true;
        }
    }

    /// True when the display holds nothing meaningful; drives the AC/C label.
    fn is_clear(&self) -> bool {
        self.display == "0"
    }

    /// macOS behavior: `C` clears just the current entry; pressing again (now
    /// showing `AC`) wipes the pending operation and accumulator too.
    fn clear(&mut self) {
        if self.is_clear() {
            *self = Calc::default();
        } else {
            self.display = "0".into();
            self.reset_on_next = true;
        }
    }

    /// The display with thousands separators inserted into the integer part,
    /// like the macOS Calculator (e.g. `4050` -> `4,050`).
    fn formatted(&self) -> String {
        let (sign, body) = match self.display.strip_prefix('-') {
            Some(rest) => ("-", rest),
            None => ("", self.display.as_str()),
        };
        let (int, frac) = match body.split_once('.') {
            Some((i, f)) => (i, Some(f)),
            None => (body, None),
        };
        let mut out = String::from(sign);
        for (i, ch) in int.chars().enumerate() {
            if i > 0 && (int.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(ch);
        }
        if let Some(f) = frac {
            out.push('.');
            out.push_str(f);
        }
        out
    }

    fn apply_pending(&mut self) {
        let rhs = self.entry();
        let result = match self.pending {
            Some('+') => self.acc + rhs,
            Some('-') => self.acc - rhs,
            Some('*') => self.acc * rhs,
            Some('/') if rhs != 0.0 => self.acc / rhs,
            Some('/') => f64::NAN,
            _ => rhs,
        };
        self.acc = result;
        self.set_display(result);
    }

    fn press(&mut self, action: Action) {
        match action {
            Action::Digit(d) => self.input_digit(d),
            Action::Dot => self.input_dot(),
            Action::Clear => self.clear(),
            Action::Back => self.input_back(),
            Action::Sign => {
                let v = -self.entry();
                self.set_display(v);
            }
            Action::Percent => {
                let v = self.entry() / 100.0;
                self.set_display(v);
                self.reset_on_next = true;
            }
            Action::Op(op) => {
                if self.pending.is_some() && !self.reset_on_next {
                    self.apply_pending();
                } else {
                    self.acc = self.entry();
                }
                self.pending = Some(op);
                self.reset_on_next = true;
            }
            Action::Equals => {
                if self.pending.is_some() {
                    self.apply_pending();
                    self.pending = None;
                }
                self.reset_on_next = true;
            }
        }
    }
}

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    // Inline (no alt screen), so clicks arrive in whole-screen coordinates and
    // have to be mapped into the surface.
    program.init_with(ProgramOptions {
        mouse: Some(MouseTracking::empty()),
        ..ProgramOptions::default()
    })?;
    let (cols, rows) = (program.screen().width(), program.screen().height());
    fit(program.screen_mut(), cols, rows);
    // Ask where the surface sits, so mouse_to_origin has something to subtract.
    program.request_origin()?;
    program.hide_cursor()?;

    let result = run(&mut program);
    // Shrink the surface and sign off, one blank row above and a couple of
    // columns to the left of the message.
    program.screen_mut().resize((cols.max(1), 2));
    program.screen_mut().clear();
    program
        .screen_mut()
        .set_str((2, 1), "Bye!", Style::default());
    program.screen_mut().render()?;
    program.finish()?;
    result
}

/// Size the inline surface: full terminal width (so the keypad can be
/// centered and trailing line-erases fall on the default background rather
/// than bleeding a button color across the row), and just tall enough for the
/// keypad.
fn fit(screen: &mut Screen<Stdout>, cols: u16, rows: u16) {
    let h = BOARD_H.min(rows.max(1));
    let size = (cols.max(1), h);
    // Resizing re-establishes the surface, so only do it when the clamped
    // size actually moved. Every caller routes through here.
    if screen.size() != size.into() {
        screen.resize(size);
    }
}

/// The left margin that centers the keypad in the surface.
fn margin(screen: &Screen<Stdout>) -> u16 {
    screen.width().saturating_sub(BOARD_W) / 2
}

fn run(program: &mut Program<Stdin, Stdout>) -> std::io::Result<()> {
    let quit: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
    let keys = buttons();
    let mut calc = Calc::default();
    let mut pressed: Option<usize> = None;
    render(program, &keys, &calc, pressed);

    loop {
        let event = program.read_event()?;
        match event {
            Event::KeyPress(ref k) if quit.contains(k) => break,
            Event::MouseClick(m) if m.button == MouseButton::Left => {
                // Map the whole-screen click into surface-local coordinates,
                // then hit-test the keypad.
                let local = program.mouse_to_origin(m);
                let dx = margin(program.screen_mut());
                if let Some(i) = keys.iter().position(|b| b.contains(local.x, local.y, dx)) {
                    calc.press(keys[i].action);
                    pressed = Some(i); // light the key while the button is held
                    render(program, &keys, &calc, pressed);
                }
            }
            Event::MouseRelease(_) if pressed.take().is_some() => {
                render(program, &keys, &calc, pressed);
            }
            Event::Resize(ws) => {
                fit(program.screen_mut(), ws.col, ws.row);
                // A resize can move the surface, so the origin is stale now.
                program.request_origin()?;
                render(program, &keys, &calc, pressed);
            }
            _ => {}
        }
    }
    Ok(())
}

fn render(
    program: &mut Program<Stdin, Stdout>,
    keys: &[Button],
    calc: &Calc,
    pressed: Option<usize>,
) {
    let dx = margin(program.screen_mut());
    program.screen_mut().clear();
    paint_display(program.screen_mut(), &calc.formatted(), dx);
    for (i, btn) in keys.iter().enumerate() {
        // The clear key shows AC when the display is empty, C otherwise.
        let label = match btn.action {
            Action::Clear if !calc.is_clear() => "C",
            _ => btn.label,
        };
        paint_button(program.screen_mut(), btn, label, dx, pressed == Some(i));
    }
    let _ = program.screen_mut().render();
}

fn paint_display(screen: &mut Screen<Stdout>, text: &str, dx: u16) {
    let style = Style::default().fg(Color::BrightWhite).bg(Color::Black);
    let rect = Rect::new(dx, 0, BOARD_W, DISPLAY_H);
    screen.fill_rect(rect, &Cell::narrow(" ").style(style.clone()));
    // Right-align the number, one cell of padding from the edge.
    let width = BOARD_W as usize;
    let shown: String = text
        .chars()
        .rev()
        .take(width - 1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let x = dx + BOARD_W.saturating_sub(shown.chars().count() as u16 + 1);
    screen.set_str((x, DISPLAY_H / 2), &shown, style.bold());
}

fn paint_button(screen: &mut Screen<Stdout>, btn: &Button, label: &str, dx: u16, pressed: bool) {
    // Each family has a resting color and a lighter pressed color.
    let (bg, fg) = match (btn.kind, pressed) {
        (Kind::Fun, false) => (Color::Rgb(0xa5, 0xa5, 0xa5), Color::Black),
        (Kind::Fun, true) => (Color::Rgb(0xd4, 0xd4, 0xd4), Color::Black),
        (Kind::Opr, false) => (Color::Rgb(0xff, 0x9f, 0x0a), Color::BrightWhite),
        (Kind::Opr, true) => (Color::Rgb(0xff, 0xc4, 0x6b), Color::BrightWhite),
        (Kind::Num, false) => (Color::Rgb(0x33, 0x33, 0x33), Color::BrightWhite),
        (Kind::Num, true) => (Color::Rgb(0x73, 0x73, 0x73), Color::BrightWhite),
    };
    let style = Style::default().fg(fg).bg(bg);
    let r = btn.rect(dx);
    screen.fill_rect(r, &Cell::narrow(" ").style(style.clone()));
    // Center the label.
    let label_w = label.chars().count() as u16;
    let lx = r.x + (r.width.saturating_sub(label_w)) / 2;
    let ly = r.y + r.height / 2;
    screen.set_str((lx, ly), label, style.bold());
}

#[cfg(test)]
mod tests {
    use super::Calc;

    fn fmt(s: &str) -> String {
        let mut c = Calc::default();
        c.display = s.into();
        c.formatted()
    }

    #[test]
    fn thousands_grouping() {
        assert_eq!(fmt("0"), "0");
        assert_eq!(fmt("999"), "999");
        assert_eq!(fmt("4050"), "4,050");
        assert_eq!(fmt("1234567"), "1,234,567");
        assert_eq!(fmt("-12345.678"), "-12,345.678");
        assert_eq!(fmt("12.5"), "12.5");
    }

    #[test]
    fn backspace_falls_back_to_zero() {
        let mut c = Calc::default();
        c.press(super::Action::Digit('7'));
        c.press(super::Action::Digit('5'));
        assert_eq!(c.display, "75");
        c.input_back();
        assert_eq!(c.display, "7");
        c.input_back();
        assert_eq!(c.display, "0");
    }

    #[test]
    fn backspace_edits_computed_value() {
        let mut c = Calc::default();
        // 12 + 33 = 45, then delete a digit off the result.
        for a in [
            super::Action::Digit('1'),
            super::Action::Digit('2'),
            super::Action::Op('+'),
            super::Action::Digit('3'),
            super::Action::Digit('3'),
            super::Action::Equals,
        ] {
            c.press(a);
        }
        assert_eq!(c.display, "45");
        c.press(super::Action::Back);
        assert_eq!(c.display, "4");
        c.press(super::Action::Digit('2'));
        assert_eq!(c.display, "42");
    }
}
