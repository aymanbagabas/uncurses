---
title: "Geometry"
weight: 7
---

uncurses uses three small geometry types for the terminal cell grid:

| Type | Meaning |
| --- | --- |
| `Position` | An `(x, y)` point: column, row. |
| `Size` | A `width x height` extent. |
| `Rect` | An axis-aligned rectangle: `(x, y, width, height)`. |

See the [layout API](/api/uncurses/layout/) for exact rustdoc and
[Text and width]({{< relref "text-and-width.md" >}}) for how grapheme widths
occupy this grid.

## Coordinate system

The origin is the top-left corner. `x` increases to the right and `y` increases
downward. A `Rect` covers half-open ranges: `x..x + width` for columns and
`y..y + height` for rows.

<div style="overflow-x:auto; margin:1rem 0;">
<svg viewBox="0 0 360 230" width="360" role="img" aria-label="Coordinate grid: origin top-left, x increases right, y increases down, with Rect(1,1,2,2) highlighted" xmlns="http://www.w3.org/2000/svg" style="max-width:100%; color:inherit;">
  <rect x="80" y="80" width="80" height="80" fill="#7c3aed" fill-opacity="0.25" stroke="#7c3aed" stroke-width="2"/>
  <g stroke="currentColor" stroke-opacity="0.5" stroke-width="1" fill="none">
    <line x1="40" y1="40" x2="40" y2="200"/><line x1="80" y1="40" x2="80" y2="200"/><line x1="120" y1="40" x2="120" y2="200"/><line x1="160" y1="40" x2="160" y2="200"/><line x1="200" y1="40" x2="200" y2="200"/><line x1="240" y1="40" x2="240" y2="200"/>
    <line x1="40" y1="40" x2="240" y2="40"/><line x1="40" y1="80" x2="240" y2="80"/><line x1="40" y1="120" x2="240" y2="120"/><line x1="40" y1="160" x2="240" y2="160"/><line x1="40" y1="200" x2="240" y2="200"/>
  </g>
  <g fill="currentColor" font-family="monospace" font-size="13" text-anchor="middle">
    <text x="60" y="30">0</text><text x="100" y="30">1</text><text x="140" y="30">2</text><text x="180" y="30">3</text><text x="220" y="30">4</text>
    <text x="28" y="64">0</text><text x="28" y="104">1</text><text x="28" y="144">2</text><text x="28" y="184">3</text>
  </g>
  <g fill="currentColor" font-family="monospace" font-size="13">
    <text x="248" y="30">x &#8594;</text>
    <text x="6" y="218">y &#8595;</text>
    <text x="250" y="112" font-size="12">Rect::new(1, 1, 2, 2)</text>
    <text x="250" y="130" font-size="12">x in 1..3, y in 1..3</text>
  </g>
</svg>
</div>

The right and bottom edges are exclusive. For `Rect::new(1, 1, 2, 2)`, cells
`(1, 1)`, `(2, 1)`, `(1, 2)`, and `(2, 2)` are inside; `(3, 1)` and `(1, 3)` are
outside.

## Constructors and fields

```rust
use uncurses::layout::{Position, Rect, Size};

let p = Position::new(3, 5);
assert_eq!((p.x, p.y), (3, 5));

let s = Size::new(80, 24);
assert_eq!((s.width, s.height), (80, 24));

let r = Rect::new(3, 5, 10, 2);
assert_eq!((r.x, r.y, r.width, r.height), (3, 5, 10, 2));
assert_eq!(r.left(), 3);
assert_eq!(r.right(), 13);
assert_eq!(r.top(), 5);
assert_eq!(r.bottom(), 7);
```

`Position::ORIGIN` is `(0, 0)`. `Size::ZERO` and `Rect::ZERO` represent empty
extents.

## Tuple shorthand

Most APIs that take geometry accept `impl Into<Position>`, `impl Into<Size>`, or
`impl Into<Rect>`, so tuples are the common shorthand:

```rust
use uncurses::layout::{Position, Rect, Size};

let p: Position = (3, 5).into();
let s: Size = (80, 24).into();
let r: Rect = (3, 5, 10, 2).into();
```

This is why drawing calls can use compact coordinates:

```rust
screen.set_str((0, 0), "hello", uncurses::style::Style::default());
screen.resize((80, 24));
```
