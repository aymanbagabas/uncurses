---
title: "Text and width"
weight: 5
---

uncurses stores and renders text in terminal cells, not bytes, scalar values, or
Rust `char`s. The text layer segments strings into extended grapheme clusters,
measures each cluster, and writes one `Cell` per grid column.

See the [text API](../api/uncurses/text/) and [cell API](../api/uncurses/cell/)
for exact rustdoc, and [Geometry]({{< relref "geometry.md" >}}) for how those
cells are addressed.

## From string to cells

`grapheme_cells(s, mode, eaw_wide)` always walks the input by extended grapheme
cluster. `WidthMode` controls only how each cluster is measured:

| Mode                  | Measurement                                                                                                                       |
| --------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `WidthMode::Wc`       | Use the width of the cluster's first code point.                                                                                  |
| `WidthMode::Grapheme` | Measure the whole cluster, including variation selectors, regional indicators, zero-width joiners, and pictographic presentation. |

Cell width then maps to `Cell` structure:

| Width | Cell representation                                                                                    |
| ----- | ------------------------------------------------------------------------------------------------------ |
| `0`   | No standalone cell. While painting, zero-width clusters attach to the previous pending cluster.        |
| `1`   | A narrow primary cell, `Cell::narrow(...)`, whose `width()` is `1`.                                    |
| `2`   | A wide primary cell, `Cell::wide(...)`, followed by a continuation placeholder whose `width()` is `0`. |

```text
bytes/scalars             grapheme cluster              terminal cells
┌───────────────┐         ┌───────────────┐             ┌────┬────┐
│ "e" + U+0301  │ ──────▶ │ "e\u{0301}"   │ ─ width 1 ▶ │ é  │    │
└───────────────┘         └───────────────┘             └────┴────┘

┌───────────────┐         ┌───────────────┐             ┌────┬────┐
│ "中"          │ ──────▶ │ "中"          │ ─ width 2 ▶ │ 中 │ ▶  │
└───────────────┘         └───────────────┘             └────┴────┘
```

Continuation cells are structural placeholders. They carry no content of their
own, are considered blank, and exist so every row still has one `Cell` value per
column.

## East-Asian width and emoji presentation

`char_width(c, eaw_wide)` and `grapheme_width(g, eaw_wide)` return terminal-cell
widths. The `eaw_wide` flag controls East-Asian Ambiguous code points: when it
is `true`, ambiguous characters such as many box-drawing characters measure as
two cells; otherwise they measure as one.

`grapheme_width` also handles cases that only make sense at cluster level:

- regional indicator clusters such as flags are width `2`;
- default-ignorable lone code points are width `0`;
- extended pictographic clusters honor VS16 (`U+FE0F`) as emoji presentation,
  width `2`;
- extended pictographic clusters honor VS15 (`U+FE0E`) as text presentation,
  width `1`.

## Drawing strings

`TextSurface` is an extension trait for any mutable surface. Implementors supply
`width_mode()` and `eaw_wide()`; the trait provides the string-painting helpers:

```rust
fn set_str(&mut self, pos: impl Into<Position>, s: &str, style: Style) -> Position;
fn set_str_wrap(
    &mut self,
    pos: impl Into<Position>,
    s: &str,
    wrap: WrapMode,
    style: Style,
) -> Position;
fn set_str_rect(&mut self, rect: impl Into<Rect>, s: &str, style: Style) -> Position;
fn set_str_rect_wrap(
    &mut self,
    rect: impl Into<Rect>,
    s: &str,
    wrap: WrapMode,
    style: Style,
) -> Position;
fn str_width(&self, s: &str) -> u16;
```

`set_str` paints from a position and stops if a non-zero-width cluster would
cross the right edge. `set_str_wrap` adds an explicit `WrapMode`: truncate or
continue on the next row. `set_str_rect` and `set_str_rect_wrap` do the same
inside an explicit clipping rectangle whose left edge is used for newline and
carriage-return handling.

The painter also interprets inline SGR (`CSI ... m`) styling and OSC 8
hyperlinks while measuring those escape sequences as zero width.

## Unicode backends

The default `unicode-rs` feature uses compact pure-Rust tables for width and a
conservative subset of pictographic and default-ignorable properties. Enabling
the `icu` feature switches width property lookups to ICU4X data with broader,
more correct Unicode coverage. The public API is the same, and the `icu`
implementation takes precedence when that feature is enabled.
