# Use uncurses with an AI assistant

The [uncurses skill](uncurses/SKILL.md) helps an assistant write applications
that depend on uncurses. It explains the ownership model, safe teardown,
common API mistakes, and where to find examples.

The skill complements the [website](https://uncurses.org) and API reference.
It targets the repository source. Check your dependency revision before using
its samples with a published release.

## Install with Copilot CLI

From an uncurses checkout, copy the skill into your personal skill directory:

```sh
copilot skill add ./skills/uncurses/SKILL.md
```

For a project-specific copy, run this command from your application directory.
Replace the example path with the path to your uncurses checkout:

```sh
copilot skill add --project /path/to/uncurses/skills/uncurses/SKILL.md
```

Copilot CLI copies file-based skills. Use `copilot skill list` to check discovery.
To update an installed copy, first remove it with `copilot skill remove uncurses`.
Then repeat the appropriate `add` command.

The repository keeps the distributable skill in `skills/uncurses/`.
Copilot CLI discovers project copies in `.github/skills/`.
Other assistants can use the same `SKILL.md` format through their supported
installation mechanism.

## Work on the repository

For uncurses development, start with [AGENTS.md](../AGENTS.md) and
[CONTRIBUTING.md](../CONTRIBUTING.md). They contain repository rules, check
commands, and routes to implementation contracts.
