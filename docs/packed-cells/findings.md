# Findings

Bugs the work surfaced, mistakes made measuring it, and what each one taught.

## Bugs found in existing code

**A lock per scalar on the emit path.** `LocalArena::grapheme` took an
`RwLock` read and a hash lookup for every scalar, because non ASCII scalars
were stored in a map. ASCII now resolves from a static string and the emitter
encodes scalars inline without consulting the arena at all. This was worth
roughly two times on full frame benchmarks.

**An `Arc` clone per cell.** `emit_cell` cloned the arena's `Arc<dyn Arena>`
purely to end a borrow before calling `&mut self`, which is two atomic
read modify writes for every cell emitted. Replaced with a stack scratch
buffer.

**Resolving through the arena to ask a question the id answers.**
`predicates.rs` called `arena.grapheme` up to six times per cell to test
whether a cell was a space, and compared resolved `Style` values where equal
ids already mean equal styles. Both are integer comparisons.

**A discarded width measurement per cell.** The painter measured a cluster,
then called `Cell::new`, which measured it again through the Unicode width
tables, and then overrode the result with the measurement it already had.
Removing the redundant call took 13.7 percent off the draw path.

**Two spellings of blank.** `Content::Empty` and `Content::Char(' ')`
rendered identically but compared unequal, so a blank that arrived as `""`
and one that arrived as `' '` would diff against each other forever and
re emit. Collapsing them removed the redundant emit and took `Cell` from 56
bytes to 48, because the third variant was what forced a discriminant word.

## Reclamation, three times wrong

The design that survives is rio's. Getting there took three failures, each
sound looking until tested.

**`Buffer::compact()`** swept using one buffer's cells as roots. Arenas are
shared, so it freed entries other buffers were still displaying. A test
proved sibling corruption: a cluster in a second buffer resolved to `""`
after an unrelated `compact()`.

**`arena::sweep(roots: &mut [&mut Buffer])`** took every root explicitly.
Correct in principle, and still wrong in practice: the test written to prove
it corrupted three unrelated tests running in parallel on the shared process
arena. No caller can name a complete root set for a process wide arena.

**A non reclaiming process arena** fixed that by giving up on reclaiming the
default. Then the restructure removed `Buffer`'s arena entirely and
`sweep(&mut [&mut Buffer])` stopped type checking, because a `Buffer` no
longer has an arena to sweep.

The lesson is that reclamation is not an algorithm problem, it is an
ownership problem. A sweep needs a complete root set, and only a type that
owns every grid on an arena can produce one. Every emulator surveyed reaches
the same conclusion by construction: the table and the cells that reference
it always have one owner, whether that is a page, a grid, or a screen.

## Measurement mistakes

**A shared target directory.** Two checkouts of the same package share build
artifacts through a global `CARGO_TARGET_DIR`. A benchmark comparison
reported the working tree at 30,975 ns for a frame it renders in 12,919.

**Setup inside the timed loop.** A draw benchmark built a `String` per row
inside `b.iter`, so it partly measured string formatting.

**Setup outside the timed loop.** A hyperlink benchmark built its buffers
before timing, so the interning it existed to measure never ran inside the
loop. It reported the link memo as worth nothing. A benchmark that times the
writes reported the same change as worth 5.8 times.

**Benchmarking the wrong layer.** `render` was measured in isolation and
declared free of regressions after the fat cell restructure. It was. The cost
had moved to the draw path, which nothing measured until later, and the whole
frame was 18 percent slower.

The pattern in all four: a number that looks fine is not evidence that the
thing you care about is fine.

## Corrections to earlier conclusions in this work

- `full_frame_no_changes` was described as a diff benchmark. It never diffed:
  both buffers had their touch flags cleared, so `render` returned at the
  early out. 1,920 cells in 8 ns would have been 1.7 TB/s.
- Scattered updates were explained as the renderer re printing intervening
  cells. It does not. It emits 181 cursor moves and 244 glyph bytes, so it
  jumps. The cost is the moves.
- The `set_str` regression was blamed on the ANSI tokenizer. It was the
  discarded width measurement described above.
