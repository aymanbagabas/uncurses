---
title: "Width"
weight: 4
---

A terminal lays text out in cells, so the make-or-break question for any string
is simple to ask and surprisingly hard to answer: how many cells does it take?
Guess wrong by one, and every character after it slides over, and the row
smears.

## Not every character is one cell

Text comes in three widths. Most characters are *narrow* and take one cell. A
few are *wide* and take two, like CJK characters. And some take *zero*: a
combining accent stacks onto the glyph before it rather than claiming a column
of its own.

<figure class="term-fig"><div class="term-grid" style="grid-template-columns: auto repeat(4, 2.2rem);"><span class="lbl">col:</span><span class="lbl">0</span><span class="lbl">1</span><span class="lbl">2</span><span class="lbl">3</span><span class="lbl">row 0</span><span>a</span><span>世</span><span class="cont">cont</span><span>é</span></div><figcaption>One row, four columns: narrow <code>a</code> (width 1), wide <code>世</code> (width 2, with its continuation cell), and <code>é</code> (the letter <code>e</code> plus a combining accent, still one cell).</figcaption></figure>

## Graphemes, not bytes or code points

That last one is the catch. The `é` above might be a single code point, or it
might be an `e` followed by a separate combining accent. Either way a human sees
one character, and it fills one cell. uncurses measures the way a human counts,
by *extended grapheme cluster*, so a cluster built from several code points
still lands in the right number of cells. Counting bytes or code points would
overcount and shove the rest of the row sideways.

## Two ways to measure

How a cluster is measured is a policy, captured by `WidthMode`:

- **`Wc`** is wcwidth-style: it measures each cluster by its first code point
  and ignores the rest. Simple, and it matches how older or plainer terminals
  behave. This is the default.
- **`Grapheme`** measures the whole cluster, accounting for variation
  selectors, regional-indicator flags, and zero-width-joiner emoji sequences.
  The cluster boundaries follow the Unicode text-segmentation rules in
  [UAX #29](https://unicode.org/reports/tr29/), and this mode is what terminals
  advertise as [Unicode Core](https://contour-terminal.org/vt-extensions/unicode-core/)
  mode (DEC mode 2027). Reach for it when the terminal draws a joined cluster as
  one unit.

## East Asian ambiguous width

A handful of code points are genuinely *ambiguous*
([UAX #11](https://unicode.org/reports/tr11/)): one cell or two depending on the
terminal and font. The `eaw_wide` flag decides which way to count them. uncurses
does not probe your terminal to find out, because that is the host's call to
make, not the library's. There is no reliable, platform-independent way to know
a terminal's choice in advance; a host that needs certainty can probe at runtime
(print the character, then read the cursor position back) and set the flag from
what it learns. You set the flag; uncurses honors it.

## Why a wrong guess hurts

Every cell declares its width, and the renderer and cursor planner trust that
declaration completely. If a string claims one cell but the terminal paints two,
everything after it is off by a column: the cursor lands in the wrong place, the
next write lands on top of the wrong cell, and the careful diff falls apart.
Measuring right is what keeps the grid honest.

## Where width lives

You rarely call the measurement functions yourself. Any
[surface]({{< relref "surfaces.md" >}}) that paints text carries a width mode
and an `eaw_wide` flag, and its string-painting methods use them, laying down
wide primaries and their continuations for you. When you do want the raw answer:

```rust
use uncurses::text::grapheme_width;

fn main() {
    assert_eq!(grapheme_width("a", false), 1);          // narrow Latin letter
    assert_eq!(grapheme_width("世", false), 2);          // wide CJK character
    assert_eq!(grapheme_width("e\u{0301}", false), 1);   // "é" = e + combining accent
}
```
