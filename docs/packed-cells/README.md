# Packed cells: a spike

An experiment in replacing uncurses' fat `Cell` with an eight byte packed
form backed by interning arenas, and a record of what it cost and bought.

The work is on `spike/packed-cells`. It is complete enough to measure and to
decide on, and the decision is not yet made. See
[verdict.md](verdict.md) first if you want the answer rather than the route.

| document | what is in it |
|---|---|
| [verdict.md](verdict.md) | Where the design wins, where it loses, and the open decision |
| [design.md](design.md) | What was built and why each piece is shaped the way it is |
| [research.md](research.md) | How ghostty, rio, kitty, and alacritty solve the same problem |
| [benchmarks.md](benchmarks.md) | Every measurement, with methodology |
| [findings.md](findings.md) | Bugs found, mistakes made, and what they taught |

## The idea in one paragraph

A terminal grid stores the same handful of values over and over: a few
hundred distinct graphemes, a dozen styles, the odd hyperlink. Storing them
inline means a 56 byte cell and a string comparison per diff. Storing them as
interned ids means an eight byte cell and an integer comparison, at the cost
of a table lookup whenever a value crosses between the two forms. Whether
that trade pays depends entirely on how much the values repeat.

## Status

The library builds clean, 1148 library tests and 21 doctests pass, clippy is
silent. Reclamation is designed but not built: the arena grows until
`MAX_ENTRIES`, then degrades gracefully. That is the largest outstanding
obligation and it is discussed in [verdict.md](verdict.md).
