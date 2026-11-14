# Contribute to uncurses

This guide covers changes to the library, examples, and website.
For application development, start with the [website](https://uncurses.org).

## Set up the checkout

Install the current stable Rust toolchain with Cargo, rustfmt, and Clippy.
The workspace records its Rust requirement in [Cargo.toml](Cargo.toml).
CI uses current stable; the benchmark build uses nightly.

Clone the repository and run:

```sh
cargo build --workspace --locked
cargo run -p examples --example hello_world
```

The example needs an interactive terminal. Press a key to exit.
For an output-only example, run:

```sh
cargo run -p examples --example offscreen
```

Cargo resolves dependencies from the workspace manifests and lockfile.
Use a checkout-local build directory when you work in multiple worktrees.

## Find the change path

`Program` owns session lifecycle, input observation, and terminal mode state.
`Screen` owns its desired cells, renderer, and output buffer.
`TextBuffer` supports independent frame construction and encoding.

Use this map to find implementation details and nearby tests:

| Change | Implementation and contracts | Verification and examples |
| --- | --- | --- |
| Input sequence | [ANSI helpers](uncurses/src/ansi/), [decoder](uncurses/src/event/decode/mod.rs), [event types](uncurses/src/event/mod.rs) | [Split-input tests](uncurses/src/event/decode_safety_tests.rs), `keylog`, `paste` |
| Terminal mode or query | [Program](uncurses/src/program/mod.rs), [mode state](uncurses/src/program/state.rs), [terminal handle](uncurses/src/terminal/handle.rs) | [Program tests](uncurses/src/program/tests.rs), `query`, `terminal` |
| Input wait or timeout | [Event source](uncurses/src/event/source.rs), [poller contract](uncurses/src/event/poll/mod.rs), [async stream](uncurses/src/event/stream.rs) | Event source tests, `legacy_keys`, `async_input` |
| Width or cell behavior | [Surface contract](uncurses/src/buffer/surface.rs), [Unicode](uncurses/src/unicode/), [text](uncurses/src/text/) | Buffer and text tests, `styles`, `offscreen` |
| Renderer behavior | [Pipeline](uncurses/src/renderer/mod.rs), [frames](uncurses/src/renderer/frame/mod.rs), [screen output](uncurses/src/screen/mod.rs) | [Golden tests](uncurses/src/renderer/tests/), `tour`, `gradient` |
| Ratatui integration | [Backend](uncurses-ratatui/src/backend.rs), [cell conversion](uncurses-ratatui/src/convert.rs) | Adapter tests, `ratatui_hello_world`, `ratatui_popup` |
| Website content | [Website rules](website/AGENTS.md), [content](website/content/docs/) | Hugo build and manual compilation of changed samples |

Read source-local contracts before editing these paths. They explain details
such as cell continuations, partial output writes, shared event readers, and
platform-specific readiness rules.

For terminal sequences, verify the relevant specification before implementation.
Use [xterm control sequences](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)
or the extension's own specification, as appropriate.

## Verify the change

Start with the affected package and test filter. Examples:

```sh
cargo test -p uncurses program::tests
cargo test -p uncurses screen::tests
cargo test -p uncurses renderer::
cargo test -p uncurses event::decode_safety_tests
cargo test -p uncurses-ratatui
```

Before a pull request, run the common workspace checks:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
cargo test --workspace --all-features --locked
```

These commands match the main [CI workflow](.github/workflows/ci.yml).
CI also builds the workspace and runs platform-specific jobs.

### Feature coverage

The core and adapter crates each require a Unicode backend.
`unicode-rs` is the default. `icu` takes precedence when both are enabled.
The `async` feature adds an executor-independent event stream.

Workspace feature unification can enable features through the examples package.
Use isolated package checks when a change affects feature boundaries:

```sh
cargo test -p uncurses --locked
cargo test -p uncurses --locked --no-default-features --features icu
cargo test -p uncurses-ratatui --locked
cargo test -p uncurses-ratatui --locked --no-default-features --features icu
```

Check synchronous examples separately:

```sh
cargo check -p examples --examples --locked --no-default-features --features unicode-rs
cargo check -p examples --examples --locked --no-default-features --features icu
```

Cargo skips the three async examples when `async` is disabled.
The Linux feature-isolation job covers both library and example configurations.

Use the [manifests](uncurses/Cargo.toml) for exact dependency and feature choices.
The [installation guide](website/content/docs/getting-started/installation.md)
explains those choices for application authors.

### Terminal and platform checks

An in-memory test, a PTY test, and a native terminal check prove different things.
State which environment and behavior you verify in the pull request.

Some Unix tests skip their body when a PTY is unavailable. See the shared
[test helper](uncurses/src/testutil.rs). A passing total alone does not establish
that these paths ran.

The scripts under [examples/tests](examples/tests/) use Kitty's Python
environment. From the workspace root, replay sized-text output through its
native parser:

```sh
cargo run -p examples --example sized_text_overlap -- --dump |
  kitty +runpy 'import runpy; runpy.run_path("examples/tests/sized_text_overlap_native.py", run_name="__main__")'
```

The drag-and-drop check needs a Kitty build with its drag-and-drop test hooks.
Its [module comment](examples/tests/drag_drop_native.py) gives the build and run
commands. That check simulates desktop gestures; it does not prove actual
desktop gestures or behavior on a remote host.

CI covers native and virtualized platforms. A local macOS result establishes
only the local configuration. Preserve the platform exceptions documented in
the [poller module](uncurses/src/event/poll/mod.rs).

### Benchmarks

The benchmark modules use nightly Rust and a separate configuration flag:

```sh
RUSTFLAGS="--cfg uncurses_bench" cargo +nightly bench --workspace --no-run --locked
```

This command assumes a rustup-managed Cargo and an installed nightly toolchain.
The CI job compiles benchmarks to catch errors. Measure performance separately
in a controlled environment.

## Work on documentation

Keep each detail at the appropriate level:

| Reader's question | Home |
| --- | --- |
| What does this library do? | README |
| How do I build a first app? | Website tutorials |
| What is this concept for? | Website concepts |
| How do I complete this task? | Website guides |
| What exactly does this method do? | Rustdoc |
| How do I change this repository? | This guide and source module docs |

Keep crate READMEs useful on their own. Retain a short introduction and example,
then link to the full guide or API reference.

Read [website/AGENTS.md](website/AGENTS.md) before changing website content.
Compile changed Rust samples in Markdown manually against the dependency they
name. Cargo doctests cover rustdoc, not the Markdown files in this repository.
Combine tutorial fragments in their stated order before compilation.

### Build the website

Install Go and Hugo extended for website work.
[website/go.mod](website/go.mod) records the Go requirement and theme version.
The [website workflow](.github/workflows/deploy-website.yml) pins the CI Hugo version.

Build the API reference from the workspace root:

```sh
cargo doc --no-deps --all-features -p uncurses -p uncurses-ratatui
```

Build the site into the ignored workspace target directory:

```sh
cd website
hugo --destination ../target/website --cacheDir "$PWD/../target/hugo-cache"
```

For an interactive preview, run `hugo server` from `website/`.
The deployment workflow combines the site with rustdoc under `/api/`.
A Hugo preview alone does not include that API output.

### Update recordings

[VHS tapes](assets/tapes/) produce the example GIFs.
Read a tape's commands and install its required tools before recording it.
The repository stores GIFs with Git LFS, as defined in [.gitattributes](.gitattributes).
For README images, use an image-serving URL rather than a raw LFS pointer.

## Commits and pull requests

Use Conventional Commit subjects, following the recent history.
Use a scope when it identifies the affected area, for example:

```text
fix(event): preserve fragmented input
docs(website): explain inline rendering
```

[cliff.toml](cliff.toml) filters nonconventional commits from release notes.
Sign off commits with `git commit -s`. Use the identity in your Git configuration.
Keep each commit focused on one logical change.

Explain the problem, the change, and its verification in the pull request.
Name any platform or native checks that you did not run.
For dependent pull requests, describe each layer's changes and its base.

## Prepare a release

Release preparation requires maintainer authorization.
The [publish workflow](.github/workflows/publish.yml) runs for `v*` tags.
It publishes workspace crates in dependency order and creates the GitHub release.

Before a release, check the workspace version, internal dependency versions,
installation text, and examples against the intended artifact.
Optional release-specific notes belong in `.github/releases/<tag>.md`.
The workflow combines them with the introduction and git-cliff output.
Only push a release tag when the release is explicitly authorized.
