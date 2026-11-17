# Website documentation

Before editing the website, read this file even when the agent host only loads
the root `AGENTS.md`.

## Documentation tiers

The site has three tiers. Give each detail one primary home.

- **Concepts** (`content/docs/concepts/`) explain what a type is for, why it
  exists, and how it relates to other types.
- **Guides** (`content/docs/guides/`) explain the procedure, connections, and
  caveats for one task.
- **Rustdoc** defines each method, parameter, escape sequence, and edge case.

Keep escape sequences, exhaustive method tables, negotiation rules, terminal
differences, and reply formats in guides or rustdoc. Link to those details from
concept pages.

Write concepts in plain language. Use the user's name for a thing, such as
"the alt screen". Keep concept samples short and focused on the idea.
Move procedural or reference material to its appropriate home.

## Writing rules

- Describe positive API behavior rather than absent APIs or migration history.
- Use American English and short, direct sentences.
- Use commas, colons, or parentheses instead of em dashes in Markdown.
- Use descriptive link text and useful image descriptions.
- Let page front matter supply the title. Start content headings at level two.
- Use bare language tags in website fences, such as `rust`.
- Compile changed Rust samples manually. Markdown samples are separate from
  Cargo doctests.
- Assemble tutorial fragments before compilation.
- Keep cleanup reachable after setup and application errors in complete samples.
- Identify whether samples target a release or the current source.

Read the [contributor guide](../CONTRIBUTING.md#build-the-website) for build and
preview commands. Keep public API details in rustdoc and link to `/api/`.
