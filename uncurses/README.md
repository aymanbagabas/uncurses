# uncurses

The core of [uncurses](https://github.com/aymanbagabas/uncurses): a
low-level toolkit for building terminal UIs in Rust. Grab the parts you
need, wire them together, and keep your hands on the loop.

New here? The [workspace README](../README.md) has the overview and a
step-by-step [tutorial](../docs/tutorial.md). This page is the map of the
core crate, module by module.

## The model

Three types do most of the heavy lifting, and they stay out of each
other's way:

- `Terminal` is the device handle. It owns the input and output halves
  plus a snapshot of the environment, flips raw mode on and off, and
  reports the window size. It is not `Copy`: it stashes the prior state on
  `make_raw` so `restore` can put things back with no arguments. Build
  `Screen` and `EventSource` from its `Copy` `output`/`input` halves and
  keep the `Terminal` around for the raw-mode lifecycle.
- `Screen` is a cell grid wrapped around a diffing renderer. You draw into
  it, then render a frame, and it emits only the bytes that changed. It
  remembers every terminal mode it switched on so it can put them all back
  on exit, and it implements `Write` (staging into its buffer) so raw
  escapes and queries ride along on the same flush.
- `EventSource` reads the input half and decodes raw bytes into typed
  `Event` values. It is wakeable: a cheaply-clonable `Waker` can knock a
  blocked read loose from another thread.

A typical app builds all three from a `Terminal`, draws into the `Screen`,
reads from the `EventSource`, and restores the terminal on the way out.
Flip on the `async` feature and an `EventStream` wraps that same
`EventSource`, handing you events through a `futures_core::Stream`.

## Buffering, in one paragraph

Drawing never touches the terminal. Every method that emits bytes, whether
it sets a cell, changes a mode, or renders a frame, stages those bytes
into an in-memory buffer. Nothing leaves until you `flush`. That is why
draw calls are infallible and return `()`: the only place I/O can fail is
`flush` (and `present`, which renders then flushes). When you want a frame
on screen, reach for `present`.

## Module map

| Module | What lives there |
| --- | --- |
| `terminal` | `Terminal` handle, raw mode, window size, stdio helpers. |
| `screen` | `Screen`: cell grid, frame diffing, terminal modes, lifecycle. |
| `event` | Input decoding, `EventSource`, terminal `query` helpers, async `stream`. |
| `style` | `Style`, attributes, colors, underline, hyperlinks, SGR emission. |
| `color` | Color types and the terminal color `Profile`. |
| `cell` | The `Cell`: one styled grapheme on the grid. |
| `buffer` | `Surface` traits and the cell buffer behind a `Screen`. |
| `layout` | `Position` and `Rect`. |
| `text` | Width measurement and wrapping. |
| `ansi` | Escape sequence builders and parsers for the protocols above. |

## Queries

Terminals will answer a surprising number of questions, if you know how
to ask. The `event::query` module models each request as a `Query`: a
value that pairs the request bytes with a matcher for the reply. The
predefined queries are consts (say `query::BACKGROUND_COLOR`);
parameterised ones are constructors (say `query::mode(...)`). Either way a
query never eats your input. Anything the user types in the meantime stays
queued, in order, for a later `read`, and the matched reply event rides
through a later `read` too.

Two methods run a query, on either an `EventSource` or (with the `async`
feature) an `EventStream`:

- **`query_blocking`** fires the request and **blocks** until the reply
  lands or `timeout` runs out, handing back the decoded value
  (`Option<T>`, `None` on timeout). On an `EventSource` it drives the
  source inline; on an `EventStream` it parks until the reader thread
  delivers. Great for a one-shot probe at startup, but in an event loop it
  stalls everything else.
- **`query`** fires the request and returns right away with an in-flight
  handle, **without blocking**. Collect it whenever: drive the source
  yourself and call `try_take` on an `EventSource`, or `.await` the handle
  on an `EventStream` (see [async queries](#async-queries)). You can keep
  several in flight at once.

The request goes out through a writer of your choosing, paired with the
`EventSource` reading the same terminal. Already rendering with a
`Screen`? Write the request through it: the bytes stage into the screen's
buffer, ship on the next flush in order with everything else, and in debug
builds land in the output trace. Just probing, with no `Screen` in play?
Write straight to the `Terminal` output (or stdout). Simpler, and every
bit as correct.

```rust,no_run
use std::time::Duration;
use uncurses::terminal::Terminal;
use uncurses::event::{EventSource, query};

fn main() -> std::io::Result<()> {
    let mut term = Terminal::open()?;
    term.make_raw()?;
    let mut out = term.output();
    let mut source = EventSource::new(term.input())?;

    // One-shot probe, no Screen, so write straight to the output.
    // Blocks this thread until the reply or the timeout.
    let bg = source.query_blocking(&mut out, query::BACKGROUND_COLOR, Duration::from_millis(100))?;

    term.restore()?; // leave raw mode before returning
    println!("background: {bg:?}");
    Ok(())
}
```

### Querying by hand

The helpers are not magic, and you do not have to use them. A `Query`
exposes the two pieces it is built from: `request()` gives you the bytes to
send, and `matches(&event)` tests whether some event is the reply (and
decodes it). So you can skip the registry entirely, write the request
yourself, and let one arm of your normal event loop catch the reply:

```rust,no_run
use std::io::Write;
use uncurses::terminal::Terminal;
use uncurses::event::{Event, EventSource, query};

fn main() -> std::io::Result<()> {
    let mut term = Terminal::open()?;
    term.make_raw()?;
    let mut out = term.output();
    let mut source = EventSource::new(term.input())?;

    out.write_all(query::BACKGROUND_COLOR.request())?; // fire the request
    out.flush()?;

    loop {
        let ev = source.read()?;
        // The reply rides in as an ordinary event; the query's own matcher
        // recognises and decodes it. Everything else is normal input.
        if let Some(color) = query::BACKGROUND_COLOR.matches(&ev) {
            println!("background: {color:?}");
        }
        if let Event::KeyPress(_) = ev {
            break; // any key quits, for the sake of the example
        }
    }

    term.restore()?;
    Ok(())
}
```

The reply rides in as just another event, so keystrokes and resizes keep
flowing while you wait, same as with the helpers, and catching it is one
extra branch in a loop you already run. A blocking `read` waits forever, so
the loop above never gives up on a silent terminal. If you want a deadline,
you can add one by hand: swap `read` for `poll(Some(remaining))` against a
deadline you track yourself, and break when the budget runs out. That is
real bookkeeping though, recomputed every iteration as input trickles in,
and it is per query, so a shared budget across a batch becomes a small
state machine you maintain.

This inline match works the same whether you hold an `EventSource` or an
`EventStream`. The stream's reader thread blocks on the terminal and fills
a shared queue; its `read` and `try_read` just pop from that queue, so the
match arm is identical. What the helpers add is that bookkeeping done for
you: a deadline, a typed result, and a whole batch resolved on one
round-trip. Under async they pull the most weight, because
`EventStream::query` hands back a `Future` that resolves on its own through
the observer registry. You can `.await` it, or race it against a timeout,
without weaving reply detection into your `Stream` loop or parking an
executor thread on a blocking `read`. The `query_inline` and
`capabilities` examples show both sides.

## Async input

Flip on the `async` feature and an `EventSource` becomes an `EventStream`,
a `futures_core::Stream` of events. It stays runtime agnostic: it leans on
`futures-core` alone and runs on any executor. A dedicated reader thread
handles the blocking readiness waits, since there is no reactor to hand
the terminal handle to.

The stream shares the `EventSource` (it holds an `Arc<Mutex<…>>`) instead
of swallowing it, so you can still run a `query` against the same source
while the stream is live. The reply gets plucked out and the rest of the
events keep flowing to the stream.

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses", features = ["async"] }
```

### Async queries

`EventStream::query` fires the request without blocking and hands back an
in-flight handle that is a `Future`: `.await` it to get the reply
(`Option<T>`, `None` on timeout). The task yields while it waits, so the
executor keeps doing other work; the reader thread delivers the reply when
it lands. The same writer choice applies: write through your `Screen` if
you have one, or straight to the terminal output if you are only probing.
(Prefer to collect it synchronously? Drive it with `try_take`, or call
`EventStream::query_blocking` to park until it arrives.)

```rust,ignore
use std::time::Duration;
use uncurses::color::Color;
use uncurses::event::{EventStream, Input, query};
use uncurses::screen::Screen;

// Runtime-agnostic: drive this with whatever executor you use.
async fn background_color<I: Input + 'static, W: std::io::Write>(
    stream: &mut EventStream<I>,
    screen: &mut Screen<W>,
) -> std::io::Result<Option<Color>> {
    // Issue the query (non-blocking), then await the reply: the task
    // yields until the answer arrives or the timeout elapses.
    Ok(stream
        .query(screen, query::BACKGROUND_COLOR, Duration::from_millis(100))?
        .await)
}
```

Build the stream from a source with `EventSource::into_stream()`. To keep
a synchronous query handle to the same source alongside the stream, share
it with `Arc<Mutex<EventSource>>` and `EventStream::from_shared(...)`.

## Unicode features

### Character widths

Every cell on the grid is one or two columns wide. uncurses decides how
many columns a character or cluster occupies with two functions in the
`text` module:

- `char_width(c, eaw_wide)` measures a single code point, wcwidth-style.
  Controls, combining marks, format characters, and default-ignorable
  code points are 0; code points whose East-Asian-Width property is
  `Wide` or `Fullwidth` (most CJK) are 2; everything else is 1.
- `grapheme_width(g, eaw_wide)` measures one extended grapheme cluster as
  a whole. It honours variation selectors (VS15 text, VS16 emoji),
  Regional Indicator pairs (flag emoji), ZWJ sequences, and
  `Extended_Pictographic` default presentation per UTS #51. Combining
  marks and joiners in the tail of a cluster add no columns.

### East-Asian Ambiguous policy (`eaw_wide`)

A block of code points carry East-Asian-Width `Ambiguous`: characters
that existed in both legacy CJK encodings (drawn double-width) and
Western text (drawn single-width), so their column count genuinely
depends on context. Examples are box-drawing glyphs, the horizontal
ellipsis, and many Greek and Cyrillic letters. The `eaw_wide` flag picks
the policy:

- `false` (default): Ambiguous code points are 1 column.
- `true`: Ambiguous code points are 2 columns.

Terminals configured for CJK locales usually render these double-wide
and want `true`; most others want `false`. The policy is orthogonal to
clustering and applies in both width modes.

There is no universally correct choice. The only hard rule is that your
measurement and the terminal's must use the same policy: if the library
counts an Ambiguous character as 1 column while the terminal draws it as
2 (or the reverse), every following cell on the line is misaligned, which
shows up as overlapping glyphs, gaps, or garbled tables. The terminal's
policy is usually tied to its locale or a config setting, and the font in
use can pull the glyph's visual width the other way again. uncurses does
not probe any of this; you set the policy to match your target with
`Screen::with_eaw_wide`, and it then flows into `str_width`,
`grapheme_width`, `set_str`, and the rest.

### Grapheme-cluster mode (Unicode core, DEC 2027)

How a string is split into cells is a separate choice from `eaw_wide`,
captured by `WidthMode`:

- `Wc` (default): each cluster's width is the width of its first code
  point alone. Cluster-blind, so VS16, ZWJ joins, and emoji-presentation
  overrides do not change the result.
- `Grapheme`: the full cluster is measured via `grapheme_width`, so a
  `✋` + VS16 sequence is 2 columns and a `✋` + VS15 sequence is 1.

This mirrors the terminal's Unicode core mode (DEC private mode 2027).
When the terminal advertises that it measures whole clusters, enabling
the mode keeps the library's accounting in step with what the terminal
actually draws; otherwise the wcwidth-style `Wc` mode matches the more
common per-code-point behaviour.

### How `Screen` measures strings

A `Screen` combines both choices when it paints. `set_str`,
`set_str_rect`, and `insert_above` all measure through the screen's
current `width_mode()` and `eaw_wide()`:

- `width_mode()` is derived from grapheme-cluster mode: `Grapheme` once
  `set_grapheme_clusters(true)` has emitted DEC 2027, `Wc` otherwise.
- `eaw_wide()` is fixed at construction with `Screen::with_eaw_wide(true)`
  and defaults to `false`.

```rust,ignore
let mut screen = Screen::new(out, (80, 24)).with_eaw_wide(true);
screen.set_grapheme_clusters(true); // measure whole clusters

// Advances the cursor by the measured column count, here 2.
let end = screen.set_str((0, 0), "中", WrapMode::Truncate);
```

To measure without painting, the screen exposes helpers that already
carry its current mode and policy: `screen.str_width(s)` for a whole
string's column count (inline SGR and OSC 8 are ignored, as in
`set_str`), `screen.grapheme_width(g)` for one cluster, and
`screen.grapheme_cells(s)` to iterate `(cluster, width)` pairs. The
underlying `text::char_width`, `text::grapheme_width`, and
`text::grapheme_cells` functions are also available if you want to pass a
mode and policy explicitly.

### Backends

Width and grapheme segmentation come from one of two backends:

- `unicode-rs` (default): pure-Rust tables, small and fast.
- `icu`: ICU4X-backed, larger but more correct on emoji and grapheme
  edge cases. Takes precedence when both are enabled.

## License

MIT. See [LICENSE](../LICENSE).
