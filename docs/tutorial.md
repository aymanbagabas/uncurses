# A short uncurses tutorial

Let's build a small interactive program from scratch: a centered button
you click (or press a key) to bump a counter. By the end you will have
touched raw mode, the alternate screen, drawing, the event loop, and a
clean teardown. The full source lives in `examples/counter.rs`.

## The three pieces

uncurses hands you three types and then stays out of the way:

- `Terminal` owns the input and output halves and the raw-mode lifecycle.
- `Screen` is a cell grid with a diffing renderer. You draw into it.
- `EventSource` decodes input bytes into typed `Event` values.

We will wrap them in an `App` struct with three methods: `start` sets
everything up, `run` is the loop, and `stop` puts the terminal back.

## Setting up

`start` enters raw mode, builds a `Screen` and an `EventSource` from the
terminal's halves, and switches on the alternate screen, a hidden cursor,
and mouse reporting.

```rust,ignore
use uncurses::terminal::Terminal;
use uncurses::ansi::mode::{MouseEncoding, MouseMode};
use uncurses::event::EventSource;
use uncurses::screen::Screen;
use uncurses::terminal::{Stdin, Stdout};

struct App {
    term: Terminal<Stdin, Stdout>,
    screen: Screen<Stdout>,
    events: EventSource<Stdin>,
    count: u32,
}

impl App {
    fn start() -> std::io::Result<Self> {
        let mut term = Terminal::stdio();
        term.make_raw()?;

        let mut screen = Screen::new(term.output(), term.window_size().unwrap_or_default());
        screen.set_alt_screen(true);
        screen.set_cursor_visible(false);
        screen.set_mouse_mode(MouseMode::Normal, MouseEncoding::Sgr);

        let events = EventSource::new(term.input())?;
        Ok(Self { term, screen, events, count: 0 })
    }
}
```

Notice the mode setters return nothing. They just stage bytes into the
screen's buffer, so they can't fail. Those bytes reach the terminal on the
next flush.

## Drawing a frame

Drawing writes cells into the screen, then commits a frame. `render`
stages the diff and `flush` sends it; `present` does both in one go. A
frame only emits the cells that actually changed since the last one.

```rust,ignore
use uncurses::buffer::SurfaceMut;
use uncurses::color::BasicColor;
use uncurses::style::Style;
use uncurses::text::WrapMode;

impl App {
    fn render(&mut self) -> std::io::Result<()> {
        self.screen.clear();
        let label = format!("[ Clicks: {} ]", self.count);
        let w = self.screen.width();
        let h = self.screen.height();
        let x = w.saturating_sub(label.len() as u16) / 2;

        let style = Style::default()
            .fg(BasicColor::BrightWhite.into())
            .bg(BasicColor::Blue.into())
            .bold();
        self.screen.set_str_with((x, h / 2), &label, WrapMode::Truncate, style);

        self.screen.present()
    }
}
```

## The event loop

`run` draws once, then blocks on `events.read()` and reacts. Keys parse
straight from strings, and `==` compares the canonical chord, so matching
a shortcut is plain equality. A resize event just tells the screen its new
size.

```rust,ignore
use uncurses::event::{Event, Key};

impl App {
    fn run(&mut self) -> std::io::Result<()> {
        self.render()?;
        let quit: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
        let click: [Key; 2] = ["enter", "space"].map(|s| s.parse().unwrap());

        loop {
            match self.events.read()? {
                Event::KeyPress(ref k) if quit.contains(k) => break,
                Event::KeyPress(ref k) if click.contains(k) => {
                    self.count += 1;
                    self.render()?;
                }
                Event::Resize(ws) => {
                    self.screen.resize(ws.col, ws.row);
                    self.render()?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}
```

## Putting the terminal back

`stop` is the mirror of `start`. `Screen::reset` emits the teardown for
every mode the screen turned on (alt screen, cursor, mouse), a flush sends
it, and `Terminal::restore` drops raw mode. Run it even when the loop
returns an error, so a crash never leaves the terminal in a wrecked state.

```rust,ignore
impl App {
    fn stop(&mut self) -> std::io::Result<()> {
        self.screen.reset();
        self.screen.flush()?;
        self.term.restore()
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
- `examples/file_explorer.rs` reads input asynchronously with the `async`
  feature and a small runtime.
- [How terminals actually work](terminals.md) is the mental model behind
  everything you just did: byte streams, raw mode, and VT modes.
- The [uncurses README](../uncurses/README.md) maps the rest of the API,
  including terminal queries and styling.
