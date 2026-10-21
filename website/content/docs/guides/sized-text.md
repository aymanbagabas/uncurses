---
title: "Sized text and regions"
weight: 15
---

Use `Screen::set_sized_text` to draw a heading or small text inside a fixed
rectangle. The screen derives the rectangle from `TextSizing`. It draws this
content with the ordinary text in the same frame.

## Select a supported terminal

This feature works on inline and fullscreen screens with Kitty text sizing support.
Width support and scale support can differ. Confirm the features your application
uses before you draw sized text.

The [protocol specification](https://sw.kovidgoyal.net/kitty/text-sizing-protocol/)
describes an explicit cursor-report probe. This probe draws visible cells.
Use a temporary area for it. Ordinary `Program` initialization leaves that
choice to the application.

## Draw a heading

```rust
use uncurses::screen::{Screen, TextSizing};
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::new(Vec::new(), (40, 8));
    screen.set_str((2, 2), "underlying cells", Style::default());
    screen.set_sized_text(
        "heading",
        (2, 2),
        "Build",
        Style::default().bold(),
        TextSizing {
            scale: 2,
            width: 5,
            ..Default::default()
        },
    )?;
    screen.render()?;
    assert!(screen.writer().windows(4).any(|bytes| bytes == b"]66;"));
    Ok(())
}
```

The heading occupies ten columns and two rows. Fractional scale changes the
font inside the same footprint. Use `scale: 1` for an explicit single-row width.
Rustdoc lists the [metadata limits](/api/uncurses/screen/struct.TextSizing.html).

The sample stores bytes in memory and uses an inline screen.
For an interactive application, draw through the program's screen.
Use `Program::enter_alt_screen` when the application needs the whole window.

## Keep stable keys across frames

A key identifies one placement on one screen. Reuse it when the content moves
or changes. Different keys remain distinct, even at the same position.
Prefixes have application-defined meaning.

The common frame loop is `clear`, draw cells and regions, then `render`.
`clear()` resets the desired frame. The screen retains the displayed frame,
so an identical rebuild produces no extra output.
`fill()` also removes region declarations and replaces the desired cells.

For incremental updates, retain the desired frame and change only the necessary
cells or regions. Both approaches use the same methods.

The screen keeps one authoritative grid of desired cells. Regions cover those
cells while visible. The renderer projects coverage into its existing staging
buffer and retains its displayed buffer for frame comparison.
If a region covers part of a wide character, the renderer replaces that whole
character with spaces that keep its style. This preserves the background and
other attributes at the region edges.

## Respect drawing order

A later text write appears above the affected part of a region.
The renderer draws native content first, then the later text.
`fill_rect` and `clear_rect` follow the same rule within their rectangles.
Full-surface operations replace the whole frame.

String helpers and window presentation use the same rule. For example, a string
written into a block's lower row appears at the requested position, with all
its characters intact. A view write keeps its requested position too.

Further text edits update the overlay. An identical rebuilt frame produces
no output, including when it contains overlays.
Call `set_sized_text` with the same key and content to cover earlier overlays again.

The renderer erases target cells before text replaces native content.
This keeps the text at its requested coordinates.
A terminal can erase a whole multi-cell glyph when one part is replaced.
For example, Kitty can erase a sized glyph that an overlay touches.
Other content in the region remains subject to its own cell boundaries.

A later region placement also retires intersecting blocks and keeps its
requested position.

## Move, remove, and hand off

Calling `set_sized_text` with the same key and a new position moves the block.
Calling `remove_region` removes it. Both restore the current underlying desired
text at the old footprint.

`Program` removes displayed regions before a screen switch or terminal handoff.
A bare `Screen` requires `release_regions()` before that transition.
This method preserves the desired content for a later render.

After a resize, move or remove blocks that exceed the new bounds.
`render()` reports invalid geometry before frame output.
It also rejects a region that intersects a text tail with uncertain width.
Use the terminal's matching grapheme mode or explicit sized text for that case.

Mutable cell access counts as an ordered write, even when the value stays equal.
Its glyph becomes an overlay at the time of access. Use `set_cell` when the
display width changes.
`insert_above` preserves displayed regions and leaves pending frame edits staged.
After a size change, insertion replays the displayed frame if its regions fit
the new viewport. It clips ordinary text at the viewport edges.
If a displayed block exceeds those bounds, insertion returns an error before
output. Move or remove that block and render the resized frame first.

## Implement other region content

`SizedText` implements the public [Region trait](/api/uncurses/screen/trait.Region.html).
`set_sized_text` derives its rectangle and calls `set_region`.
An external crate can implement the same trait and provide a rectangle directly.

The contract describes immutable, opaque native blocks with later text overlays.
The implementation supplies content equality and a `prepare` method.
The renderer owns composition, frame comparison, output, and exposed text
restoration.

`prepare` receives the block size and an immutable
[RegionContext](/api/uncurses/screen/struct.RegionContext.html).
The context supplies the screen size and color profile.
Return a [PreparedRegion](/api/uncurses/screen/struct.PreparedRegion.html)
with separate draw and cleanup operations.
The renderer reuses these operations when equal content moves within the same
screen environment. Replay and cleanup use retained operations.

Use [RegionOp](/api/uncurses/screen/enum.RegionOp.html) for local cursor
positions, styles, native bytes, and rectangular erasure.
Each operation list starts at the placement origin with the default style.
Encode native bytes with the ANSI helpers.
Raw bytes make the tracked cursor unknown. Use `SetCursor` only when the
protocol guarantees that position and clears pending wrap.
Inline output preserves an anchor through the terminal's DEC saved cursor.
Native payloads must preserve that save slot. Applications must not retain
their own saved cursor across region output.
Prefer `Erase` for cleanup; the renderer can clip it after a viewport shrink.
Position-independent `Command` operations can also run after a viewport shrink.
Other cursor-dependent cleanup requires the old block to fit.

The renderer checks local coordinates before it changes frame state.
The implementation must ensure that native bytes stay inside the block,
preserve terminal modes and the pen, and avoid scrolling or wrapping.
The protocol must support cell erasure where text replaces native content.
Alternatively, implement `prepare_visible` to clip native output around later
text. This method receives the remaining local row spans.
Coordinate checks cannot establish the effects of an arbitrary protocol.

Use `RegionResource` for shared, position-independent upload and release recipes.
Resources are created after frame preparation and before ordinary cells.
Their dependencies are created first and released last.
`set_region_resource` retains a resource for ordinary cells.
The image crate uses these bindings for Unicode placeholders.

`Region::coverage` can describe disjoint occupied rectangles within a block.
Its holes preserve ordinary cells. `AnchoredRegion` resolves a native group from
the current desired cells. Anchor movement updates its footprint before later
text edits. See the [image guide]({{< relref "images.md" >}}) for a complete example.

A preparation error leaves the displayed frame, queued output, and desired
edits intact. `insert_above` can still replay the displayed frame.
The renderer prepares all fallible extension work before it encodes the frame.

An output failure returns the writer's I/O error and discards queued bytes.
Partial output can leave the terminal and renderer in different states.
The caller handles terminal recovery before further output.
After recovery, call `Screen::invalidate()` to request a full repaint.

## Run the example

For an interactive example, run:

```sh
cargo run -p examples --example sized_text_drag
```

The screen shows a dim Chinese character pattern behind a large heading.
Drag the heading with the left mouse button. The text beneath its old position
appears again. Press `l` to insert a log line above the managed screen.
In fullscreen mode, the log goes to the alt screen's hidden scrollback.
Press `q`, `Esc`, or `Ctrl-C` to quit.
This example requires Kitty text sizing support and at least 18 columns and
four rows. It checks width and scale support before it draws the pattern.
The cursor-report probe uses explicit spaces because Kitty ignores empty
text payloads.
A terminal multiplexer must support the protocol too. If the probe fails,
run the example directly in a supporting terminal.

With Kitty installed, compare incremental drag, insertion, and resize output
against fresh frames. The resize checks keep one terminal instance and preserve
pending text, placement, and clear operations:

```sh
cargo test -p examples --example sized_text_drag native_drag_matches_fresh_frames -- --ignored --nocapture
```

For a sequence of region operations, run:

```sh
cargo run -p examples --example sized_text
```

The visual example labels each stage and advances every 1.5 seconds.
It shows equal frames, coordinate-preserving overwrites, movement, removal,
and fractional scale. Its `CaptionedText` implementation combines sized text
and a caption through the generic prepared-region API.
Separate text edits preserve the caption between them.
Later text, clear, and fill operations replace parts of the caption.
Insertion replays the displayed overlays while pending edits stay staged.
Removal restores the underlying text with those edits.
It requires a supporting terminal with at least
40 columns and eight rows.

Add `-- --inline` to run in the normal terminal buffer.
This version also demonstrates insertion after the managed area grows or shrinks.
It requires ten terminal rows and keeps pending edits for the next render.

Use `-- --preview` for escaped bytes or `-- --dump` for hexadecimal frame data.
With Kitty installed, verify the frame data through its native parser:

```sh
cargo run -p examples --example sized_text -- --dump |
  kitty +runpy 'import runpy; runpy.run_path("examples/tests/sized_text_native.py", run_name="__main__")'
```

Check inline output from three initial rows, including the terminal's bottom row:

```sh
cargo run -p examples --example sized_text -- --inline --dump |
  kitty +runpy 'import sys, runpy; sys.argv = ["sized_text_native.py", "--inline"]; runpy.run_path("examples/tests/sized_text_native.py", run_name="__main__")'
```
