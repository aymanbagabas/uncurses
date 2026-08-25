# How other terminals store and reclaim cells

Surveyed from source while designing the packed cell. Every claim below was
read out of the repository named, not recalled.

## Summary

| | ghostty | rio | kitty | alacritty |
|---|---|---|---|---|
| cell size | 8 B (`packed struct(u64)`) | 8 B (`repr(transparent) u64`) | 32 B (12 CPU + 20 GPU) | 24 B |
| grapheme | per page bitmap allocator | per grid `ExtrasTable` | per screen `TextCache` | per cell `Arc<CellExtra>` |
| style | id into per page `RefCountedSet` | id into per grid `StyleSet` | inline | inline |
| hyperlink | id into per page `RefCountedSet` | via `extras_id` | id into per screen pool | in `Arc<CellExtra>` |
| reclamation | reference counting | mark and sweep, threshold | epoch GC, renumbers | `Arc` drop |
| per write cost | one inc, one dec | none | none | usually none |
| ids stable after GC | mostly | yes, free list | no, all renumbered | n/a |

## ghostty (Zig)

`Cell` is a `packed struct(u64)`: a 21 bit codepoint, a 16 bit `style_id`, two
bits of `Wide`, and flags. A page is one contiguous mmap holding rows, cells,
a `StyleSet`, a grapheme allocator, a string allocator, and a hyperlink set.
Every side table is per page. There is no process wide table.

Reclamation is reference counting inside `RefCountedSet`. `Page.clearCells`
releases the style of every cell it overwrites, and writes acquire. Dead
entries are trimmed from the tail of the item array on insert, and when more
than ten percent of the table is dead an insert returns `NeedsRehash`, which
makes `PageList` allocate a fresh page and `cloneFrom` the live entries.

Two details worth stealing: `style_id == 0` always means the default style and
needs no lookup, and cells that carry only a background colour encode it in
the cell bits and never touch the style table at all.

`Page.verifyIntegrity` walks every cell and checks refcount consistency in
debug builds. It exists because refcount drift is otherwise invisible.

## rio (Rust)

`Square` is a `repr(transparent)` `u64` with the same shape: 21 bit codepoint,
a 16 bit `style_id`, a 16 bit `extras_id`, and tag bits. `StyleSet` and
`ExtrasTable` are fields on `Grid`, so a grid owns both its cells and its
tables. Primary and alternate screens are separate `Grid` values with separate
tables.

The comment in `square.rs` is explicit that this replaced an alacritty derived
design of a 24 byte cell plus per cell heap allocation.

Reclamation is mark and sweep on a threshold, and it never renumbers. Freed
ids go on a free list and are handed out to new values, so a live id keeps its
slot and no cell ever needs rewriting. `intern` checks a 1024 slot direct
mapped memo cache first, so a repeated style costs an array index. A sweep
fires when the free list is empty, enough novel values have been interned, and
the table is over a high water mark.

The trigger lives inside `Grid::intern_style`. A caller never orchestrates
reclamation and never sees it.

## kitty (C)

Splits per cell data into a 20 byte `GPUCell` and a 12 byte `CPUCell`. Style
is inline, so there is no style table and no style reclamation at all.
Graphemes go in a per screen `TextCache`, hyperlinks in a per screen pool, and
both are shared between the main and alternate line buffers.

Both tables garbage collect on a count threshold, checked on cell write.
The GC walks every cell in history, main, and alternate buffers and
**renumbers every id in place**. That renumbering is the source of a real bug
class: kitty has to explicitly reset memoized tab stop indices after a text
cache GC, because they held stale indices.

## alacritty (Rust)

No side tables. `Cell` is 24 bytes with `c`, `fg`, `bg`, `flags`, and an
`Option<Arc<CellExtra>>` for zero width characters and hyperlinks. The common
cell has `extra == None` and costs nothing. Reclamation is `Drop`.

## What we took

The id layout is closest to rio, and deliberately so: scalars are identity
mapped and only multi scalar clusters reach a table, which is a stronger
version of ghostty's "id 0 is free" trick.

The reclamation model we planned is also rio's, for a reason the benchmarks
later confirmed: ghostty pays an increment and a decrement on every styled
cell write, and we measured that class of per cell work costing roughly two
times on our emit path. A sweep keeps writes free.

We did not take kitty's renumbering. Stable ids are worth more than the
simpler sweep, and their stale index bug is the argument.
