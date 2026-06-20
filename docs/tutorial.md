# A short uncurses tutorial

Let's build a small interactive program from scratch: a centered button
you click (or press a key) to bump a counter. By the end you will have
touched raw mode, the alternate screen, drawing, the event loop, and a
clean teardown. The full source lives in `examples/counter.rs`.

## One type to start with

The high-level [`Screen`] is a self-managing facade. It owns three things
you would otherwise wire up yourself:

- a `Terminal` (the input and output halves, plus the raw-mode lifecycle),
- a `Canvas` (a cell grid with a diffing renderer that you draw into), and
- an `EventSource` (the decoder that turns input bytes into typed `Event`
  values).

You can use those three pieces directly when you want the control (see the
[uncurses README](../uncurses/README.md)), but `Screen` is the fastest way
to a working app. We will wrap it in an `App` struct with three methods:
`start` sets everything up, `run` is the loop, and `stop` puts the terminal
back.

## Setting up

`start` builds a `Screen` over the process stdio, begins a session, and
switches on the alternate screen, a hidden cursor, and mouse reporting.

```rust,ignore
use uncurses::event::{Event, Key, MouseButton};
use uncurses::screen::{MousePreference, Screen, ScreenOptions};
use uncurses::terminal::{Stdin, Stdout};

struct App {
    screen: Screen<Stdin, Stdout>,
    count: u32,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut screen = Screen::stdio()?;

        // init enters raw mode and queries the terminal's capabilities. The
        // options describe the defaults we want: here, enable mouse
        // tracking so clicks arrive as events. The screen picks the best
        // mouse mode and encoding the terminal actually supports.
        screen.init_with(ScreenOptions {
            mouse: Some(MousePreference::default()),
            ..ScreenOptions::default()
        })?;
        screen.enter_alt_screen()?;
        screen.hide_cursor()?;

        Ok(Self { screen, count: 0 })
    }
}
```

`init` (and `init_with`) does the busywork: raw mode, a batch of capability
queries, and the always-on defaults like bracketed paste. The mode setters
(`enter_alt_screen`, `hide_cursor`, `enable_mouse`, and the rest) write
their escape sequences immediately and flush, so each one returns an
`io::Result`.

## Drawing a frame

Drawing writes cells into the screen, then commits a frame. `render`
stages the diff and `flush` sends it; `present` does both in one go. A
frame only emits the cells that actually changed since the last one.

The drawing methods come from two traits. `clear`, `width`, and `height`
come from the surface traits in [`buffer`]; the string painters
(`set_str` and friends) come from [`TextSurface`]. Bring them into scope.

```rust,ignore
use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::color::BasicColor;
use uncurses::style::Style;
use uncurses::text::TextSurface;

impl App {
    fn render(&mut self) -> std::io::Result<()> {
        self.screen.clear();
        let label = format!("[ Clicks: {} ]", self.count);
        let w = self.screen.width();
        let h = self.screen.height();
        let x = w.saturating_sub(label.len() as u16) / 2;

        let style = Style::default()
            .fg(BasicColor::BrightWhite)
            .bg(BasicColor::Blue)
            .bold();
        self.screen.set_str((x, h / 2), &label, style);

        self.screen.present()
    }
}
```

## The event loop

`run` draws once, then blocks on `screen.read_event()` and reacts. Keys parse
straight from strings, and `==` compares the canonical chord, so matching
a shortcut is plain equality. A resize event just tells the screen its new
size, and a mouse click bumps the counter too.

```rust,ignore
impl App {
    fn run(&mut self) -> std::io::Result<()> {
        self.render()?;
        let quit: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
        let click: [Key; 2] = ["enter", "space"].map(|s| s.parse().unwrap());

        loop {
            match self.screen.read_event()? {
                Event::KeyPress(ref k) if quit.contains(k) => break,
                Event::KeyPress(ref k) if click.contains(k) => {
                    self.count += 1;
                    self.render()?;
                }
                Event::MouseClick(m) if m.button == MouseButton::Left => {
                    self.count += 1;
                    self.render()?;
                }
                Event::Resize(ws) => {
                    self.screen.resize((ws.col, ws.row));
                    self.render()?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}
```

`read` returns each event and, as a side effect, records the capability
replies to the queries `init` fired. Those replies never reach your match
arms; you just see the keys, mouse, paste, and resize events you care
about.

## Putting the terminal back

`stop` is the mirror of `start`, and it is a single call. `Screen::finish`
tears down every mode the screen turned on (alt screen, cursor, mouse),
flushes, and restores the terminal's prior state. It consumes the screen,
so `stop` takes `self` by value. Run it even when the loop returns an
error, so a crash never leaves the terminal in a wrecked state.

```rust,ignore
impl App {
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
```

## Where to go next

- `examples/keylog.rs` shows every decoded event, plus suspend and resume.
- `examples/screen_toggle.rs` flips between inline and alternate screen.
- `examples/inline_input.rs` grows a multi-line prompt in place and commits
  it into the scrollback.
- `examples/file_explorer.rs` reads input asynchronously with the `async`
  feature and a small runtime.
- [How terminals actually work](terminals.md) is the mental model behind
  everything you just did: byte streams, raw mode, and VT modes.
- The [uncurses README](../uncurses/README.md) maps the rest of the API,
  including the low-level `Canvas`, terminal queries, and styling.

[`Screen`]: https://docs.rs/uncurses/latest/uncurses/screen/struct.Screen.html
[`TextSurface`]: https://docs.rs/uncurses/latest/uncurses/text/trait.TextSurface.html
[`buffer`]: https://docs.rs/uncurses/latest/uncurses/buffer/index.html
