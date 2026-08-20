# Agent instructions

## Website documentation

The site has three tiers, and detail belongs to exactly one of them.

- **Concepts** (`website/content/docs/concepts/`) stay high level. A concept
  page explains what a type is for, why it exists, and how it relates to the
  others. It is the page someone reads to decide whether they need the thing at
  all.
- **Guides** (`website/content/docs/guides/`) are task-shaped. They carry the
  procedure, the wiring, and the caveats for one job.
- **Rustdoc** carries the exhaustive detail: every method, parameter, escape
  sequence, and edge case.

Keep out of concept pages: escape sequences and control-code spellings,
exhaustive method or variant tables, protocol negotiation rules, per-terminal
behavior notes, and reply-format specifics. Link to the guide or the rustdoc
instead of inlining them. If a concept page needs a code sample, it should be
short enough to read at a glance and illustrate the idea rather than a
procedure.

Write concept pages in plain language. Refer to prominent things by the name a
user would use ("the alt screen"), not by the name the implementation uses. If a
sentence needs insider vocabulary to parse, it belongs in a guide or the
rustdoc. A whole section that only explains a design detail is not worth a place
on a concept page at all; delete it.

A concept page running long is the signal that reference material has leaked
into it. Move that material rather than trimming prose around it.

## Documentation writing

- Never define an API by what it lacks ("has no `X`", "there is no `Y`") or by
  what changed ("no longer", "used to", "the old `Z` was split"). A new reader
  has no memory of a shape that never shipped. State the positive behavior.
  Statements about the world (no terminfo database) and positive design
  boundaries are fine.
- No em dashes in Markdown. Rustdoc may use them.
- Website code fences take a bare language tag (` ```rust `). Rustdoc attribute
  suffixes like `rust,no_run` and `rust,ignore` mean nothing to Hugo and stop
  the block being syntax highlighted.
- Code samples in Markdown are not compiled by the test suite, including the
  `rust,no_run` blocks in `README.md`. Compile them by hand when you change
  them.
