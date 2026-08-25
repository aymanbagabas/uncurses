# Verdict

Interning is a bet that values repeat. The measurements say the bet pays for
text shaped frames and loses badly for frames where every cell is unique.

## Where it wins

| | fat | packed | |
|---|---:|---:|---|
| diffing an unchanged screen | 6,368 ns | 1,071 | 5.95x |
| one line changed, whole frame | 3,435 ns | 2,083 | 1.65x |
| full repaint, whole frame | 70,581 ns | 48,268 | 1.46x |
| renderer memory, three 80x24 grids | 322 KB | 46 KB | 7x |

Scan dominated work wins most, because an eight byte cell means seven times
less memory walked and equality is one integer comparison rather than a
string plus an `Arc` comparison.

## Where it loses

| | fat | packed | |
|---|---:|---:|---|
| drawing a distinct style per cell | 10,542 ns | 99,742 | **0.11x** |

That is a gradient, an image, or a star field. The `space`, `space_unlimited`,
and `gradient` examples in this repository are all this shape. Every cell
mints an id that is never reused, so the arena pays a hash to store a value it
will never see again, and the style memo never hits.

The same workload renders 3.62 times faster packed. Draw is the larger half,
so the whole frame is a net loss.

## The unpaid obligation

Reclamation does not exist. The arena grows until `MAX_ENTRIES` and then
degrades gracefully, which is safe but never frees. On a style churning
application that ceiling is about 34 frames of `space_unlimited`.

This only exists because we intern. Fat storage has no such obligation:
values are dropped when the cell holding them is overwritten.

## The open decision

Reverting is cheap and contained, which is the one clearly good outcome of the
restructure. Packing lives only in `RenderBuffer`; `Buffer`, `Window`, `View`,
and `TextBuffer` are already plain fat cells, and the public API is already
clean and arena free. A revert is: delete `renderer/packed/`, make
`RenderBuffer` a `Grid<Cell>`, drop the two memo caches, and optionally
collapse `Grid<T>` back to a concrete `Grid<Cell>`.

So the choice is between:

- **keep**: 1.4 to 1.6 times on text shaped frames, 5.9 times on diffing, 7
  times renderer memory, against 9.5 times slower on churn shaped drawing and
  a garbage collector still to write.
- **revert**: lose those wins, lose the obligation, and delete roughly a
  thousand lines of machinery.

The work that is worth keeping either way is independent of the answer: the
`Cell` and `Ref` split gave the library a fat, arena free public API with
`Surface` and `SurfaceMut` implementable outside the crate again, and the bug
fixes in [findings.md](findings.md) apply to both storage models.
