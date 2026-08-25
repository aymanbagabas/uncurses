# What was built

## Two cell types

```
cell::Cell    48 B   owns its content and style. What applications see.
Ref            8 B   three interned ids. What the renderer stores.
```

`Cell` is arena free, so it can be built, compared, and passed around with no
reference to any grid. `Ref` is `pub(crate)` and lives in `renderer::packed`,
so an id can never escape the crate.

```rust
pub struct Cell {
    pub content: Content,   // Char(char) | Cluster(Box<str>)
    pub style: Style,       // SGR + Option<Arc<Link>>
    pub kind: Kind,         // Narrow | Wide | Continuation
}

pub(crate) struct Ref {
    content: GraphemeId,    // u32, Kind packed into the top two bits
    style: StyleId,         // u16
    link: LinkId,           // u16
}
```

## The id layout is the load bearing part

```
grapheme id 0 .. 0x110000     the Unicode scalar with that value
grapheme id >= 0x110000       index into the cluster table
```

Every single scalar grapheme is its own id. It needs no table, no lock, and
no arena, which means a scalar cell is valid against *every* arena and
`Ref::BLANK` can be a `const`. Only clusters of two or more scalars are
interned. A corpus spanning Latin, Cyrillic, Greek, CJK, and Hangul interns
zero entries.

Style id 0 and link id 0 are pre seeded in every arena as the empty style and
no link, so a default styled unlinked cell is also arena agnostic.

## Where the boundary is

```
Buffer, Window, View, TextBuffer      Grid<Cell>     fat, no arena
RenderBuffer                          Grid<Ref>      packed, owns the arena
```

`Surface` and `SurfaceMut` speak only `Cell`. `RenderBuffer` interns on
`set_cell` and resolves on `cell`, and that is the only place the two forms
meet. The renderer works on `Ref` throughout, so a frame diff is an integer
comparison.

`Grid<T: GridCell>` is the shared row major storage plus wide cell
bookkeeping, generic so the same 660 lines serve both cell types. `GridCell`
is six methods: `width`, `is_wide`, `is_continuation`, `blank`,
`continuation`, `blank_like`, `continuation_like`.

## Fast paths that make the boundary cheap

Three of these, added after measuring the boundary at 2.1 times slower than
fat storage. Together they took it to slightly faster.

1. `intern_style` returns id 0 for the empty style with no lock, since it is
   pre seeded.
2. `Cell::intern` skips the arena entirely for a scalar in the default style
   with no link. Every id in that cell is identity mapped, so packing is a
   pure function.
3. `RenderBuffer` remembers the last style and the last link it interned.
   Writes hold `&mut self`, so these need no synchronisation. Adjacent cells
   almost always share both.

The link memo matters most. Interning a link builds an owned `Link` to probe
the table with, so a miss costs two allocations on top of the lock. Without
the memo, drawing a frame of hyperlinked text is 5.8 times slower than plain
text. With it, the difference is two percent.

## What is not built

Reclamation. The arena grows until `MAX_ENTRIES` (65535 per table), then
degrades: unknown graphemes render as `?`, unknown styles as the default.
That bound is real and prevents memory exhaustion from untrusted input, but
nothing is ever freed.

Three earlier designs were built and removed, each unsound for the ownership
model of the time. See [findings.md](findings.md).

The remaining design is rio's, and it is now sound because `Screen` owns
front, back, and tracked buffers and can name a complete root set. It needs
`RenderBuffer::new_in` so a `Screen` gives its three buffers one arena rather
than the process default, a threshold check, and a free list.
