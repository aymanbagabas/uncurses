# Work on uncurses

uncurses is a Rust library for terminal user interfaces. Applications control
their event loop and draw cells inline, on the alt screen, or into memory.
The library provides typed input, terminal queries, and a diff renderer.

## Understand the pieces

- `Program` owns the terminal session, input, capabilities, and a `Screen`.
- `Screen<W>` owns a cell grid, renderer, and writer. `render()` emits changes.
- `TextBuffer` holds a frame that the caller can encode and write independently.

Start with the [public layer map](website/content/docs/getting-started/the-layers.md).
For implementation work, use the [change map](CONTRIBUTING.md#find-the-change-path)
and follow its links to source documentation.

## Repository map

| Path | Purpose |
| --- | --- |
| `uncurses/` | Core library: sessions, cells, input, text, and terminal access |
| `uncurses-ratatui/` | Ratatui backend built around `Program` |
| `examples/examples/` | Runnable examples, including `hello_world` |
| `examples/tests/` | Native terminal checks with separate requirements |
| `website/` | Hugo site: tutorials, concepts, guides, and API links |
| `skills/uncurses/` | Consumer skill for applications that use uncurses |
| `.github/workflows/` | CI, website builds, and release procedures |

Use the crates in this checkout as the source of truth for available APIs.
Published releases and the current source can differ.

## Find the right information

| Need | Start here |
| --- | --- |
| Understand the project | [README](README.md) |
| Build a first application | [Hello, terminal](website/content/docs/getting-started/hello-world.md) |
| Understand a public concept | [Concepts](website/content/docs/concepts/) |
| Complete an application task | [Guides](website/content/docs/guides/) |
| Find exact API behavior | [Rustdoc](https://uncurses.org/api/uncurses/) and source docs |
| Change the library | [Contributor guide](CONTRIBUTING.md) |
| Find a nearby example | [Examples](examples/examples/) |

## Development rules

- Read the relevant module docs and tests before a change.
- Preserve ownership boundaries and the invariants beside the implementation.
- Keep caller-supplied configuration unchanged.
- Reproduce a bug before its fix. Add a focused regression test.
- Demonstrate each new feature in a runnable example.
- Keep terminal restoration reachable after setup and application errors.
- Compile changed Rust samples in Markdown. Cargo does not check them as doctests.
- Describe APIs by their positive behavior. Use American English.
- Use Conventional Commit subjects and sign off commits, as explained in the
  [contributor guide](CONTRIBUTING.md#commits-and-pull-requests).

## Common checks

Run commands from the workspace root:

```sh
cargo test -p uncurses
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

The [verification guide](CONTRIBUTING.md#verify-the-change) explains feature
coverage, native checks, benchmarks, and the website build.

## Scoped guidance

Before any change under `website/`, read [website/AGENTS.md](website/AGENTS.md).
Apply its documentation tiers and writing rules even when the host only loads
the root instruction file.

The [consumer skill](skills/uncurses/SKILL.md) teaches application development.
Use `CONTRIBUTING.md` for repository procedures.
